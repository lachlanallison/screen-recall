//! OCR pipeline.
//!
//! An `OcrEngine` trait lets us swap Tesseract for a platform-native
//! engine or a vision-LLM pass later without touching the worker loop.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::config::OcrEngineKind;
use crate::state::SharedState;
use crate::util::perf_log;
use crate::util::process::configure_hidden_std;
use crate::util::tesseract;
use crate::worker_activity::OcrActiveGuard;

pub trait OcrEngine: Send + Sync {
    fn ocr_image(&self, path: &Path) -> Result<String>;
}

pub struct TesseractEngine;

/// Tesseract is usually built to read PNG/JPEG/TIFF. Captures are often stored
/// as WebP, which `image` can decode but many Windows Tesseract+Leptonica
/// builds cannot—leading to exit 0 and empty text.
fn tesseract_read_path(src: &Path) -> Result<(PathBuf, bool, (u32, u32))> {
    let use_direct = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            e == "png" || e == "jpg" || e == "jpeg" || e == "tif" || e == "tiff" || e == "bmp"
        })
        .unwrap_or(false);
    if use_direct {
        let (w, h) = image::image_dimensions(src).unwrap_or((0, 0));
        return Ok((src.to_path_buf(), false, (w, h)));
    }
    let img = image::open(src).with_context(|| {
        format!(
            "OCR: decode image for Tesseract ({}). File missing or corrupt?",
            src.display()
        )
    })?;
    let w = img.width();
    let h = img.height();
    // Grayscale + 8-bit PNG: better for Tesseract on UI / WebP; avoids some Leptonica/WebP quirk
    // paths that still yield exit 0 and empty text.
    let luma = img.to_luma8();
    let tmp = std::env::temp_dir().join(format!("screenrecall-ocr-{}.png", uuid::Uuid::new_v4()));
    luma.save(&tmp)
        .with_context(|| format!("OCR: write temp PNG for Tesseract ({})", tmp.display()))?;
    Ok((tmp, true, (w, h)))
}

impl OcrEngine for TesseractEngine {
    fn ocr_image(&self, path: &Path) -> Result<String> {
        let bin = tesseract::find_binary().ok_or_else(|| {
            anyhow::anyhow!(
                "tesseract executable not found. Install Tesseract and ensure it is on PATH."
            )
        })?;

        let (input, temp, (src_w, src_h)) = tesseract_read_path(path)?;

        let _cleanup = if temp {
            struct DropTmp(PathBuf);
            impl Drop for DropTmp {
                fn drop(&mut self) {
                    let _ = fs::remove_file(&self.0);
                }
            }
            Some(DropTmp(input.clone()))
        } else {
            None
        };

        // PSM 6: single text block; often better for full-screen app/web captures than 3.
        let mut cmd = Command::new(&bin);
        configure_hidden_std(&mut cmd);
        let output = cmd
            .arg(&input)
            .arg("stdout")
            .arg("-l")
            .arg("eng")
            .arg("--dpi")
            .arg("220")
            .arg("--psm")
            .arg("6")
            .arg("--oem")
            .arg("3")
            .output()
            .map_err(|e| anyhow::anyhow!("failed to run {}: {e}", bin.display()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(anyhow::anyhow!("tesseract run failed: {stderr}"));
        }

        let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if out.is_empty() {
            warn!(
                path = %path.display(),
                tesseract = %bin.display(),
                temp_png = temp,
                src_w,
                src_h,
                tesseract_status = %output.status,
                tesseract_stderr = %stderr.trim(),
                "OCR: tesseract returned empty stdout (Leptonica/Tesseract may still log to stderr above)"
            );
        }

        Ok(out)
    }
}

fn build_engine(kind: OcrEngineKind) -> Box<dyn OcrEngine> {
    match kind {
        OcrEngineKind::Tesseract => Box::new(TesseractEngine),
    }
}

pub async fn run_worker(state: SharedState, mut rx: UnboundedReceiver<i64>) -> Result<()> {
    info!("OCR worker started");

    // Process any frames that were already queued from a previous run.
    flush_pending(&state).await;

    loop {
        // Either the scheduler pushed a new frame, or we wake every so often
        // to handle any backlog (e.g. if OCR was configured off and re-enabled).
        let maybe_id = tokio::select! {
            biased;
            id = rx.recv() => id,
            _ = sleep(Duration::from_secs(30)) => None,
        };

        if let Some(id) = maybe_id {
            process_one(&state, id).await;
        } else {
            flush_pending(&state).await;
        }
    }
}

async fn flush_pending(state: &SharedState) {
    let pending = match state.store.pending_ocr(32) {
        Ok(p) => p,
        Err(err) => {
            warn!(?err, "failed to load pending OCR queue");
            return;
        }
    };
    for id in pending {
        process_one(state, id).await;
    }
}

async fn process_one(state: &SharedState, id: i64) {
    let started = std::time::Instant::now();
    let frame = match state.store.get_frame(id) {
        Ok(Some(f)) => f,
        Ok(None) => return,
        Err(err) => {
            warn!(frame_id = id, ?err, "get_frame failed");
            return;
        }
    };

    let _active = OcrActiveGuard::new(&state.worker_activity, id);

    let cfg = state.config();
    let engine = build_engine(cfg.ocr_engine);
    let path = frame.path.clone();
    let text = tokio::task::spawn_blocking(move || engine.ocr_image(Path::new(&path)))
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("join error: {e}")));

    match text {
        Ok(text) => {
            if let Err(err) = state.store.set_ocr_text(id, &text) {
                warn!(frame_id = id, ?err, "save OCR text failed");
                let ms = started.elapsed().as_millis() as u64;
                perf_log::record(
                    state,
                    "ocr_error",
                    serde_json::json!({
                        "frame_id": id,
                        "ms": ms,
                        "error": err.to_string(),
                    }),
                );
                state.worker_activity.record_ocr_ms(ms);
            } else {
                if text.trim().is_empty() {
                    warn!(frame_id = id, "OCR: empty result stored (Tesseract line above includes path, dimensions, stderr)");
                }
                debug!(frame_id = id, chars = text.len(), "OCR indexed");
                let ms = started.elapsed().as_millis() as u64;
                perf_log::record(
                    state,
                    "ocr_ok",
                    serde_json::json!({
                        "frame_id": id,
                        "chars": text.len(),
                        "ms": ms,
                    }),
                );
                state.worker_activity.record_ocr_ms(ms);
            }
        }
        Err(err) => {
            warn!(frame_id = id, ?err, "OCR failed; marking as empty");
            let ms = started.elapsed().as_millis() as u64;
            perf_log::record(
                state,
                "ocr_error",
                serde_json::json!({
                    "frame_id": id,
                    "ms": ms,
                    "error": err.to_string(),
                }),
            );
            // Record an empty result so we don't retry forever.
            let _ = state.store.set_ocr_text(id, "");
            state.worker_activity.record_ocr_ms(ms);
        }
    }
}
