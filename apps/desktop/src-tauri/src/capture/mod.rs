//! Periodic screen capture.
//!
//! The scheduler loops on a configurable interval. For every tick it:
//!   1. Checks the pause flag and the exclude list (active window).
//!   2. Captures every connected monitor via `xcap` (serial capture, parallel processing).
//!   3. Computes a 64-bit dHash and compares to the last frame we kept
//!      for that monitor. If the Hamming distance is below the threshold
//!      we skip a new file/row and extend the previous row's
//!      `static_until_ms` so the UI can show how long the screen stayed static.
//!   4. Writes the frame as JPEG/WebP under `<data>/frames/YYYY/MM/DD/`.
//!   5. Inserts a row into SQLite and forwards the new id to the OCR /
//!      embedding workers.

use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{Datelike, Timelike, Utc};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{sleep, Instant};
use tracing::{debug, info, warn};
use xcap::Monitor;

use crate::config::{CaptureImageFormat, CaptureResizeFilter};
use crate::state::SharedState;
use crate::util::active_window::active_window_info;
use crate::util::monitor_names::friendly_monitor_name;

/// Hamming distance below which two consecutive frames are treated
/// as "unchanged" and the newer one is dropped.
const PHASH_NOOP_THRESHOLD: u32 = 3;

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

        let elapsed = tick_start.elapsed();
        let target = Duration::from_secs(interval_secs);
        if elapsed < target {
            sleep(target - elapsed).await;
        } else {
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

    let state_inner = state.clone();
    let frames_dir = state.frames_dir();
    let win_info = win.clone();
    let max_edge = config.capture_downscale_max_edge;
    let img_format = config.capture_image_format;
    let jpeg_quality = config.capture_jpeg_quality.clamp(1, 100);
    let resize_filter = config.capture_resize_filter;

    let new_ids = tokio::task::spawn_blocking(move || -> Result<Vec<i64>> {
        let start = std::time::Instant::now();
        let monitors =
            Monitor::all().context("enumerate monitors via xcap")?;
        let enum_elapsed = start.elapsed();
        if monitors.is_empty() {
            return Ok(vec![]);
        }

        // Phase 1: capture all images sequentially (fast I/O).
        struct Capture {
            monitor_id: i32,
            monitor_name: String,
            image: DynamicImage,
            capture_ms: u64,
        }
        let mut captures = Vec::with_capacity(monitors.len());
        for monitor in &monitors {
            let monitor_id = monitor.id().unwrap_or(0) as i32;
            let monitor_name = friendly_monitor_name(&monitor.name().unwrap_or_default())
                .unwrap_or_else(|| monitor.name().unwrap_or_default());
            let t0 = std::time::Instant::now();
            let raw = monitor.capture_image().context("monitor capture_image")?;
            let capture_ms = t0.elapsed().as_millis() as u64;
            let dyn_img = DynamicImage::ImageRgba8(
                ImageBuffer::<Rgba<u8>, _>::from_raw(
                    raw.width(),
                    raw.height(),
                    raw.into_raw(),
                )
                .ok_or_else(|| anyhow::anyhow!("invalid raw image"))?,
            );
            captures.push(Capture { monitor_id, monitor_name, image: dyn_img, capture_ms });
        }

        // Phase 2: process each captured image in parallel (CPU-bound).
        let mut new_ids = Vec::new();
        {
            let store = &state_inner.store;
            let perf = &state_inner.capture_perf;
            let win_info = win_info.as_ref();

            std::thread::scope(|s| {
                let mut handles = Vec::with_capacity(captures.len());
                for cap in &captures {
                    let handle = s.spawn(|| -> Result<Option<i64>> {
                        let t0 = std::time::Instant::now();
                        let phash = dhash_u64(&cap.image);
                        let now = Utc::now();
                        let now_ms = now.timestamp_millis();

                        if let Some((last_id, prev)) = store.last_frame_fingerprint(cap.monitor_id)? {
                            if hamming(prev, phash) <= PHASH_NOOP_THRESHOLD {
                                store.extend_frame_static_until(last_id, now_ms)?;
                                let total_ms = t0.elapsed().as_millis() as u64;
                                perf.record(cap.monitor_id, &cap.monitor_name, cap.capture_ms, 0, 0, total_ms);
                                if total_ms > 2500 {
                                    info!(monitor_id = cap.monitor_id, capture_ms = cap.capture_ms, total_ms, "slow capture (skipped — unchanged screen)");
                                }
                                return Ok(None);
                            }
                        }

                        let scaled = if max_edge > 0 && cap.image.width().max(cap.image.height()) > max_edge {
                            let t2 = std::time::Instant::now();
                            let result = downscale(&cap.image, max_edge, resize_filter);
                            let downscale_ms = t2.elapsed().as_millis() as u64;
                            (result, downscale_ms)
                        } else {
                            (cap.image.clone(), 0)
                        };
                        let (scaled_img, downscale_ms) = scaled;

                        let ext = match img_format {
                            CaptureImageFormat::Jpeg => "jpg",
                            CaptureImageFormat::Webp => "webp",
                        };
                        let rel = format!(
                            "{:04}/{:02}/{:02}/{:02}{:02}{:02}{:03}_m{}.{}",
                            now.year(), now.month(), now.day(),
                            now.hour(), now.minute(), now.second(),
                            now.timestamp_subsec_millis(),
                            cap.monitor_id, ext,
                        );
                        let out_path = frames_dir.join(&rel);
                        if let Some(parent) = out_path.parent() {
                            std::fs::create_dir_all(parent).ok();
                        }

                        let t3 = std::time::Instant::now();
                        save_frame(&scaled_img, &out_path, img_format, jpeg_quality)
                            .with_context(|| format!("save frame to {}", out_path.display()))?;
                        let save_ms = t3.elapsed().as_millis() as u64;

                        let id = store.insert_frame(
                            now.timestamp_millis(),
                            out_path.to_string_lossy().as_ref(),
                            phash,
                            win_info.map(|w| w.process.as_str()),
                            win_info.map(|w| w.title.as_str()),
                            cap.monitor_id,
                        )?;

                        let total_ms = t0.elapsed().as_millis() as u64;
                        perf.record(cap.monitor_id, &cap.monitor_name, cap.capture_ms, downscale_ms, save_ms, total_ms);
                        perf.record_saved(cap.monitor_id, cap.capture_ms, downscale_ms, save_ms, total_ms);
                        if total_ms > 2500 {
                            info!(monitor_id = cap.monitor_id, capture_ms = cap.capture_ms, downscale_ms, save_ms, total_ms, "slow capture (saved)");
                        }
                        Ok(Some(id))
                    });
                    handles.push(handle);
                }
                for handle in handles {
                    match handle.join().unwrap() {
                        Ok(Some(id)) => new_ids.push(id),
                        Ok(None) => {}
                        Err(err) => warn!(?err, "monitor capture failed"),
                    }
                }
            });
        }

        let total_elapsed = start.elapsed();
        state_inner.capture_perf.set_last_tick(
            total_elapsed.as_millis() as u64,
            enum_elapsed.as_millis() as u64,
        );
        if total_elapsed.as_secs_f64() > 3.0 {
            info!(count = new_ids.len(), elapsed_ms = total_elapsed.as_millis() as u64, enum_ms = enum_elapsed.as_millis() as u64, "slow capture tick");
        }
        Ok(new_ids)
    })
    .await??;

    for id in &new_ids {
        let _ = tx.send(*id);
    }
    Ok(new_ids.len())
}

fn save_frame(
    img: &DynamicImage,
    path: &std::path::Path,
    format: CaptureImageFormat,
    jpeg_quality: u8,
) -> Result<()> {
    match format {
        CaptureImageFormat::Jpeg => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("create JPEG file: {}", path.display()))?;
            let mut buf = std::io::BufWriter::new(file);
            let encoder = jpeg_encoder::Encoder::new(&mut buf, jpeg_quality);
            match img {
                DynamicImage::ImageRgba8(rgba) => {
                    encoder
                        .encode(&rgba, rgba.width() as u16, rgba.height() as u16, jpeg_encoder::ColorType::Rgba)
                        .with_context(|| format!("encode JPEG: {}", path.display()))?;
                }
                _ => {
                    let rgb = img.to_rgb8();
                    encoder
                        .encode(&rgb, rgb.width() as u16, rgb.height() as u16, jpeg_encoder::ColorType::Rgb)
                        .with_context(|| format!("encode JPEG: {}", path.display()))?;
                }
            }
            buf.flush().with_context(|| format!("flush JPEG: {}", path.display()))?;
            Ok(())
        }
        CaptureImageFormat::Webp => {
            let rgb = img.to_rgb8();
            rgb.save_with_format(path, image::ImageFormat::WebP)
                .with_context(|| format!("encode WebP: {}", path.display()))?;
            Ok(())
        }
    }
}

fn downscale(img: &DynamicImage, max_edge: u32, filter: CaptureResizeFilter) -> DynamicImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= max_edge {
        return img.clone();
    }
    let ratio = max_edge as f32 / long as f32;
    let new_w = (w as f32 * ratio).round().max(1.0) as u32;
    let new_h = (h as f32 * ratio).round().max(1.0) as u32;
    let f = match filter {
        CaptureResizeFilter::Nearest => image::imageops::FilterType::Nearest,
        CaptureResizeFilter::Lanczos3 => image::imageops::FilterType::Lanczos3,
    };
    img.resize(new_w, new_h, f)
}

fn dhash_u64(img: &DynamicImage) -> u64 {
    let (w, h) = img.dimensions();
    let mut small = [0u8; 72];

    for y in 0..8 {
        for x in 0..9 {
            let sx = ((x as u32 * w) / 9).min(w - 1);
            let sy = ((y as u32 * h) / 8).min(h - 1);
            let p = img.get_pixel(sx, sy);
            small[y * 9 + x] = ((p[0] as u16 + p[1] as u16 * 2 + p[2] as u16) / 4) as u8;
        }
    }

    let mut bits: u64 = 0;
    for y in 0..8 {
        for x in 0..8 {
            if small[y * 9 + x] > small[y * 9 + x + 1] {
                bits |= 1 << (y * 8 + x);
            }
        }
    }
    bits
}

fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}
