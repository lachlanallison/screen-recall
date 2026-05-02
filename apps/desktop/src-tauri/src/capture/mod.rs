//! Periodic screen capture.
//!
//! The scheduler loops on a configurable interval. For every tick it:
//!   1. Checks the pause flag and the exclude list (active window).
//!   2. Captures every connected monitor via `xcap`.
//!   3. Computes a 64-bit dHash and compares to the last frame we kept
//!      for that monitor. If the Hamming distance is below the threshold
//!      we skip a new file/row and extend the previous row's
//!      `static_until_ms` so the UI can show how long the screen stayed static.
//!   4. Writes the frame as WebP under `<data>/frames/YYYY/MM/DD/`.
//!   5. Inserts a row into SQLite and forwards the new id to the OCR /
//!      embedding workers.
//!
//! This module is intentionally simple (one frame at a time, one writer)
//! to keep resource use low.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{Datelike, Timelike, Utc};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{sleep, Instant};
use tracing::{debug, info, warn};
use xcap::Monitor;

use crate::state::SharedState;
use crate::store::Store;
use crate::util::active_window::{active_window_info, WindowInfo};

/// Hamming distance below which two consecutive frames are treated
/// as "unchanged" and the newer one is dropped.
const PHASH_NOOP_THRESHOLD: u32 = 3;

/// Cap on the long edge for stored frames. Saves disk space.
const MAX_LONG_EDGE: u32 = 1920;

pub async fn run_scheduler(
    state: SharedState,
    tx: UnboundedSender<i64>,
) -> Result<()> {
    std::fs::create_dir_all(state.frames_dir()).ok();

    info!("capture scheduler started");
    loop {
        let interval_secs = state.config().capture_interval_secs.max(1);
        let tick_start = Instant::now();

        if state.is_recording() && !skip_capture_for_session_lock(&state) {
            match capture_once(&state, &tx).await {
                Ok(n) => {
                    if n > 0 {
                        debug!("captured {n} new frame(s)");
                    }
                }
                Err(err) => warn!(?err, "capture tick failed"),
            }
        }

        // Sleep the rest of the interval.
        let elapsed = tick_start.elapsed();
        let target = Duration::from_secs(interval_secs);
        if elapsed < target {
            sleep(target - elapsed).await;
        } else {
            // Already over budget; yield for a moment so we don't spin hot.
            sleep(Duration::from_millis(50)).await;
        }
    }
}

fn skip_capture_for_session_lock(state: &SharedState) -> bool {
    let cfg = state.config();
    cfg.pause_when_workstation_locked && state.is_workstation_locked()
}

async fn capture_once(
    state: &SharedState,
    tx: &UnboundedSender<i64>,
) -> Result<usize> {
    let config = state.config();

    // Active window (used for both exclude list + metadata).
    let win = active_window_info();

    if let Some(info) = &win {
        for excl in &config.excluded_processes {
            if excl.eq_ignore_ascii_case(&info.process) {
                return Ok(0);
            }
        }
        for substr in &config.excluded_window_substrings {
            if info.title.to_lowercase().contains(&substr.to_lowercase()) {
                return Ok(0);
            }
        }
    }

    // `Arc<AppState>` is Send + Sync so we can move it into the blocking pool.
    let state_inner = state.clone();
    let frames_dir = state.frames_dir();
    let win_info = win.clone();

    let new_ids = tokio::task::spawn_blocking(move || -> Result<Vec<i64>> {
        let monitors =
            Monitor::all().context("enumerate monitors via xcap")?;
        if monitors.is_empty() {
            return Ok(vec![]);
        }
        let mut new_ids = Vec::new();
        for monitor in monitors {
            match capture_monitor(
                &monitor,
                &frames_dir,
                &state_inner.store,
                win_info.as_ref(),
            ) {
                Ok(Some(id)) => new_ids.push(id),
                Ok(None) => {}
                Err(err) => warn!(?err, "monitor capture failed"),
            }
        }
        Ok(new_ids)
    })
    .await??;

    for id in &new_ids {
        let _ = tx.send(*id);
    }
    Ok(new_ids.len())
}

fn capture_monitor(
    monitor: &Monitor,
    frames_dir: &PathBuf,
    store: &Store,
    win_info: Option<&WindowInfo>,
) -> Result<Option<i64>> {
    let monitor_id = monitor.id().unwrap_or(0) as i32;
    let image = monitor.capture_image().context("monitor capture_image")?;
    let dyn_img: DynamicImage =
        DynamicImage::ImageRgba8(ImageBuffer::<Rgba<u8>, _>::from_raw(
            image.width(),
            image.height(),
            image.into_raw(),
        )
        .ok_or_else(|| anyhow::anyhow!("invalid raw image"))?);

    // Compute perceptual hash and compare with last for this monitor.
    let phash = dhash_u64(&dyn_img);
    let now = Utc::now();
    let now_ms = now.timestamp_millis();
    if let Some((last_id, prev)) = store.last_frame_fingerprint(monitor_id)? {
        if hamming(prev, phash) <= PHASH_NOOP_THRESHOLD {
            store.extend_frame_static_until(last_id, now_ms)?;
            return Ok(None);
        }
    }

    // Downscale for storage.
    let scaled = downscale(&dyn_img, MAX_LONG_EDGE);

    // Build output path: frames/YYYY/MM/DD/HHMMSSmmm_<mon>.webp
    let rel = format!(
        "{:04}/{:02}/{:02}/{:02}{:02}{:02}{:03}_m{}.webp",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.timestamp_subsec_millis(),
        monitor_id,
    );
    let out_path = frames_dir.join(&rel);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // Encode as WebP. `image` crate uses lossless by default for WebP; to get
    // a smaller file we encode as JPEG-quality-equivalent lossy via the webp
    // feature if available; otherwise fall back to JPEG.
    match scaled.save(&out_path) {
        Ok(()) => {}
        Err(err) => {
            warn!(?err, "webp save failed, falling back to JPEG");
            let jpeg_path = out_path.with_extension("jpg");
            scaled
                .to_rgb8()
                .save_with_format(&jpeg_path, image::ImageFormat::Jpeg)?;
            return Ok(Some(store.insert_frame(
                now.timestamp_millis(),
                jpeg_path.to_string_lossy().as_ref(),
                phash,
                win_info.map(|w| w.process.as_str()),
                win_info.map(|w| w.title.as_str()),
                monitor_id,
            )?));
        }
    }

    let id = store.insert_frame(
        now.timestamp_millis(),
        out_path.to_string_lossy().as_ref(),
        phash,
        win_info.map(|w| w.process.as_str()),
        win_info.map(|w| w.title.as_str()),
        monitor_id,
    )?;
    Ok(Some(id))
}

fn downscale(img: &DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= max_edge {
        return img.clone();
    }
    let ratio = max_edge as f32 / long as f32;
    let new_w = (w as f32 * ratio).round().max(1.0) as u32;
    let new_h = (h as f32 * ratio).round().max(1.0) as u32;
    img.resize(new_w, new_h, image::imageops::FilterType::Triangle)
}

/// 64-bit dHash. See https://www.hackerfactor.com/blog/index.php?/archives/529-Kind-of-Like-That.html
fn dhash_u64(img: &DynamicImage) -> u64 {
    let small = img
        .grayscale()
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut bits: u64 = 0;
    for y in 0..8 {
        for x in 0..8 {
            let l = small.get_pixel(x, y).0[0];
            let r = small.get_pixel(x + 1, y).0[0];
            if l > r {
                bits |= 1 << (y * 8 + x);
            }
        }
    }
    bits
}

fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

