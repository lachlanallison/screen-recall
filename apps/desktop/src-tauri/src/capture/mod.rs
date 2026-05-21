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

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{Datelike, Timelike, Utc};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{sleep, Instant};
use tracing::{debug, info, warn};
use xcap::Monitor;

use crate::config::CaptureImageFormat;
use crate::state::SharedState;
use crate::util::active_window::active_window_info;
use crate::util::monitor_names::friendly_monitor_name;

use fast_image_resize as fr;

/// Rolling window of capture outcomes for a single monitor.
/// Used to auto-detect noisy sources (CCTV feeds etc.).
#[derive(Clone, Debug, Default)]
struct AdaptiveState {
    /// Last 20 ticks: Some(distance) if saved, None if skipped.
    ticks: VecDeque<Option<u32>>,
    /// True when this monitor is classified as a noisy source.
    is_noisy: bool,
}

impl AdaptiveState {
    fn push(&mut self, saved: bool, distance: u32) {
        self.ticks
            .push_back(if saved { Some(distance) } else { None });
        if self.ticks.len() > 20 {
            self.ticks.pop_front();
        }
    }

    fn evaluate(&mut self) {
        if self.ticks.len() < 15 {
            self.is_noisy = false;
            return;
        }
        let saved: Vec<u32> = self.ticks.iter().filter_map(|&t| t).collect();
        self.is_noisy = saved.len() >= 15 && saved.iter().all(|&d| d < 13);
    }
}

pub async fn run_scheduler(state: SharedState, tx: UnboundedSender<i64>) -> Result<()> {
    std::fs::create_dir_all(state.frames_dir()).ok();

    info!("capture scheduler started");
    let mut fingerprint_cache: HashMap<i32, (i64, u64)> = HashMap::new();
    let mut adaptive_state: HashMap<i32, AdaptiveState> = HashMap::new();
    loop {
        let interval_secs = state.config().capture_interval_secs.max(1);
        let tick_start = Instant::now();

        if state.is_recording() && !skip_capture_for_session_lock(&state) {
            match capture_once(&state, &tx, &mut fingerprint_cache, &mut adaptive_state).await {
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

type MonitorCaptureOutcome = Option<(Option<i64>, i32, i64, u64, AdaptiveState)>;

async fn capture_once(
    state: &SharedState,
    tx: &UnboundedSender<i64>,
    fingerprint_cache: &mut HashMap<i32, (i64, u64)>,
    adaptive_state: &mut HashMap<i32, AdaptiveState>,
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
    let dedupe_threshold = config.capture_dedupe_threshold;
    let adaptive_enabled = config.capture_adaptive_dedupe_enabled;
    let noisy_threshold = config.capture_noisy_monitor_threshold;
    let cache_snapshot = fingerprint_cache.clone();
    let adaptive_snapshot = adaptive_state.clone();
    let downscale_max = max_edge;

    #[derive(Debug)]
    struct CaptureResult {
        new_ids: Vec<i64>,
        cache_updates: Vec<(i32, i64, u64)>,
        adaptive_updates: Vec<(i32, AdaptiveState)>,
    }

    let result = tokio::task::spawn_blocking(move || -> Result<CaptureResult> {
        let start = std::time::Instant::now();
        let monitors = Monitor::all().context("enumerate monitors via xcap")?;
        let enum_elapsed = start.elapsed();
        if monitors.is_empty() {
            return Ok(CaptureResult {
                new_ids: vec![],
                cache_updates: vec![],
                adaptive_updates: vec![],
            });
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
                ImageBuffer::<Rgba<u8>, _>::from_raw(raw.width(), raw.height(), raw.into_raw())
                    .ok_or_else(|| anyhow::anyhow!("invalid raw image"))?,
            );
            captures.push(Capture {
                monitor_id,
                monitor_name,
                image: dyn_img,
                capture_ms,
            });
        }

        // Phase 2: process each captured image in parallel (CPU-bound).
        let mut new_ids = Vec::new();
        let mut cache_updates = Vec::new();
        let mut adaptive_updates = Vec::new();
        {
            let store = &state_inner.store;
            let perf = &state_inner.capture_perf;
            let win_info = win_info.as_ref();

            std::thread::scope(|s| {
                let mut handles = Vec::with_capacity(captures.len());
                for cap in &captures {
                    let handle = s.spawn(|| -> Result<MonitorCaptureOutcome> {
                        let mut local_adaptive = adaptive_snapshot
                            .get(&cap.monitor_id)
                            .cloned()
                            .unwrap_or_default();

                        let t0 = std::time::Instant::now();

                        // Downscale first so dhash + save work on the smaller image.
                        let (work_img, downscale_ms) = if downscale_max > 0
                            && cap.image.width().max(cap.image.height()) > downscale_max
                        {
                            let t2 = std::time::Instant::now();
                            let result = downscale_fast(&cap.image, downscale_max);
                            let ms = t2.elapsed().as_millis() as u64;
                            (result, ms)
                        } else {
                            (cap.image.clone(), 0)
                        };

                        let t_dhash = std::time::Instant::now();
                        let phash = dhash_u64(&work_img);
                        let dhash_ms = t_dhash.elapsed().as_millis() as u64;

                        let now = Utc::now();
                        let now_ms = now.timestamp_millis();

                        let db_start = std::time::Instant::now();
                        let (fingerprint, from_cache) = if let Some(fingerprint) =
                            cache_snapshot.get(&cap.monitor_id).copied()
                        {
                            (Some(fingerprint), true)
                        } else {
                            (store.last_frame_fingerprint(cap.monitor_id)?, false)
                        };
                        let db_lookup_ms = if from_cache {
                            0
                        } else {
                            db_start.elapsed().as_millis() as u64
                        };

                        let distance = fingerprint
                            .map(|(_, prev)| hamming(prev, phash))
                            .unwrap_or(0);
                        let effective = if adaptive_enabled
                            && dedupe_threshold > 0
                            && local_adaptive.is_noisy
                        {
                            noisy_threshold
                        } else {
                            dedupe_threshold
                        };

                        if let Some((last_id, prev)) = fingerprint {
                            if dedupe_threshold > 0
                                && distance <= effective
                                && store.extend_frame_static_until(last_id, now_ms)?
                            {
                                let db_ms = if from_cache {
                                    db_start.elapsed().as_millis() as u64
                                } else {
                                    db_lookup_ms
                                };
                                let total_ms = t0.elapsed().as_millis() as u64;
                                if db_ms > 500 {
                                    info!(
                                        monitor_id = cap.monitor_id,
                                        db_lookup_ms,
                                        db_update_ms = db_ms.saturating_sub(db_lookup_ms),
                                        db_ms,
                                        "slow capture DB phase (static extend)"
                                    );
                                }
                                perf.record(
                                    cap.monitor_id,
                                    &cap.monitor_name,
                                    cap.capture_ms,
                                    dhash_ms,
                                    downscale_ms,
                                    0,
                                    db_ms,
                                    total_ms,
                                );
                                if total_ms > 2500 {
                                    info!(
                                        monitor_id = cap.monitor_id,
                                        capture_ms = cap.capture_ms,
                                        total_ms,
                                        "slow capture (skipped — unchanged screen)"
                                    );
                                }
                                local_adaptive.push(false, distance);
                                if adaptive_enabled && dedupe_threshold > 0 {
                                    local_adaptive.evaluate();
                                }
                                return Ok(Some((
                                    None,
                                    cap.monitor_id,
                                    last_id,
                                    prev,
                                    local_adaptive,
                                )));
                            }
                        }

                        let ext = match img_format {
                            CaptureImageFormat::Jpeg => "jpg",
                            CaptureImageFormat::Webp => "webp",
                        };
                        let rel = format!(
                            "{:04}/{:02}/{:02}/{:02}{:02}{:02}{:03}_m{}.{}",
                            now.year(),
                            now.month(),
                            now.day(),
                            now.hour(),
                            now.minute(),
                            now.second(),
                            now.timestamp_subsec_millis(),
                            cap.monitor_id,
                            ext,
                        );
                        let out_path = frames_dir.join(&rel);
                        if let Some(parent) = out_path.parent() {
                            std::fs::create_dir_all(parent).ok();
                        }

                        let t3 = std::time::Instant::now();
                        save_frame(&work_img, &out_path, img_format, jpeg_quality)
                            .with_context(|| format!("save frame to {}", out_path.display()))?;
                        let save_ms = t3.elapsed().as_millis() as u64;

                        let db_insert_start = std::time::Instant::now();
                        let id = store.insert_frame(
                            now.timestamp_millis(),
                            out_path.to_string_lossy().as_ref(),
                            phash,
                            win_info.map(|w| w.process.as_str()),
                            win_info.map(|w| w.title.as_str()),
                            cap.monitor_id,
                            Some(&cap.monitor_name),
                        )?;
                        let db_insert_ms = db_insert_start.elapsed().as_millis() as u64;
                        let db_ms = db_lookup_ms + db_insert_ms;
                        let total_ms = t0.elapsed().as_millis() as u64;
                        if db_ms > 500 {
                            info!(
                                monitor_id = cap.monitor_id,
                                db_lookup_ms, db_insert_ms, db_ms, "slow capture DB phase"
                            );
                        }
                        perf.record(
                            cap.monitor_id,
                            &cap.monitor_name,
                            cap.capture_ms,
                            dhash_ms,
                            downscale_ms,
                            save_ms,
                            db_ms,
                            total_ms,
                        );
                        perf.record_saved(
                            cap.monitor_id,
                            cap.capture_ms,
                            dhash_ms,
                            downscale_ms,
                            save_ms,
                            db_ms,
                            total_ms,
                        );
                        if total_ms > 2500 {
                            info!(
                                monitor_id = cap.monitor_id,
                                capture_ms = cap.capture_ms,
                                downscale_ms,
                                save_ms,
                                total_ms,
                                "slow capture (saved)"
                            );
                        }
                        local_adaptive.push(true, distance);
                        if adaptive_enabled && dedupe_threshold > 0 {
                            local_adaptive.evaluate();
                        }
                        Ok(Some((Some(id), cap.monitor_id, id, phash, local_adaptive)))
                    });
                    handles.push(handle);
                }
                for handle in handles {
                    match handle.join().unwrap() {
                        Ok(Some((new_id, monitor_id, last_id, phash, local_adaptive))) => {
                            if let Some(id) = new_id {
                                new_ids.push(id);
                            }
                            cache_updates.push((monitor_id, last_id, phash));
                            adaptive_updates.push((monitor_id, local_adaptive));
                        }
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
            info!(
                count = new_ids.len(),
                elapsed_ms = total_elapsed.as_millis() as u64,
                enum_ms = enum_elapsed.as_millis() as u64,
                "slow capture tick"
            );
        }
        Ok(CaptureResult {
            new_ids,
            cache_updates,
            adaptive_updates,
        })
    })
    .await??;

    for (monitor_id, id, phash) in &result.cache_updates {
        fingerprint_cache.insert(*monitor_id, (*id, *phash));
    }

    for (monitor_id, local_adaptive) in result.adaptive_updates {
        let old_noisy = adaptive_state
            .get(&monitor_id)
            .map(|s| s.is_noisy)
            .unwrap_or(false);
        let new_noisy = local_adaptive.is_noisy;
        if !old_noisy && new_noisy {
            info!(
                monitor_id,
                "monitor classified as noisy — running retroactive cleanup"
            );
            match retroactive_cleanup(state, monitor_id, noisy_threshold) {
                Ok(merged) => {
                    if merged > 0 {
                        info!(monitor_id, merged, "retroactive cleanup merged frames");
                    }
                    if let Ok(mut diag) = state.adaptive_diagnostics.write() {
                        let entry = diag.entry(monitor_id).or_default();
                        entry.merged_frames_count += merged as u64;
                    }
                }
                Err(e) => {
                    warn!(monitor_id, error = %e, "retroactive cleanup failed");
                }
            }
        } else if old_noisy && !new_noisy {
            info!(monitor_id, "monitor cleared noisy flag");
        }
        if let Ok(mut diag) = state.adaptive_diagnostics.write() {
            let entry = diag.entry(monitor_id).or_default();
            entry.is_noisy = local_adaptive.is_noisy;
            entry.tick_count = local_adaptive.ticks.len();
            entry.saved_tick_count = local_adaptive.ticks.iter().filter(|&&t| t.is_some()).count();
            entry.last_distance = local_adaptive.ticks.back().copied().flatten().unwrap_or(0);
        }
        adaptive_state.insert(monitor_id, local_adaptive);
    }

    for id in &result.new_ids {
        let _ = tx.send(*id);
    }
    Ok(result.new_ids.len())
}

/// Merge recently-saved noisy frames on a monitor into the earliest frame of
/// each contiguous group, deleting redundant rows and files.
fn retroactive_cleanup(
    state: &SharedState,
    monitor_id: i32,
    noisy_threshold: u32,
) -> Result<usize> {
    let frames = state
        .store
        .recent_unarchived_fingerprints(monitor_id, i64::MAX, 120)?;
    if frames.len() < 2 {
        return Ok(0);
    }

    let mut merged = 0usize;
    let mut i = 0;
    while i < frames.len() {
        let mut group_end = i;
        let mut j = i + 1;
        while j < frames.len() {
            let dist = hamming(frames[j - 1].3, frames[j].3);
            if dist <= noisy_threshold {
                group_end = j;
                j += 1;
            } else {
                break;
            }
        }

        if group_end > i {
            let keeper_id = frames[i].0;
            let last_ts = frames[group_end].1;
            let delete_ids: Vec<i64> = frames[i + 1..=group_end].iter().map(|f| f.0).collect();

            state
                .store
                .merge_noisy_frames(keeper_id, &delete_ids, last_ts)?;

            for frame in &frames[i + 1..=group_end] {
                if let Err(e) = std::fs::remove_file(&frame.2) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        warn!(path = %frame.2, error = %e, "failed to delete merged frame file");
                    }
                }
            }

            merged += delete_ids.len();
        }

        i = group_end + 1;
    }

    Ok(merged)
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
                        .encode(
                            rgba,
                            rgba.width() as u16,
                            rgba.height() as u16,
                            jpeg_encoder::ColorType::Rgba,
                        )
                        .with_context(|| format!("encode JPEG: {}", path.display()))?;
                }
                _ => {
                    let rgb = img.to_rgb8();
                    encoder
                        .encode(
                            &rgb,
                            rgb.width() as u16,
                            rgb.height() as u16,
                            jpeg_encoder::ColorType::Rgb,
                        )
                        .with_context(|| format!("encode JPEG: {}", path.display()))?;
                }
            }
            buf.flush()
                .with_context(|| format!("flush JPEG: {}", path.display()))?;
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

fn downscale_fast(img: &DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= max_edge {
        return img.clone();
    }
    let ratio = max_edge as f32 / long as f32;
    let new_w = (w as f32 * ratio).round().max(1.0) as u32;
    let new_h = (h as f32 * ratio).round().max(1.0) as u32;

    let rgba = img.to_rgba8();
    let src = fr::images::Image::from_vec_u8(w, h, rgba.into_raw(), fr::PixelType::U8x4)
        .expect("valid src image for resize");
    let mut dst = fr::images::Image::new(new_w, new_h, fr::PixelType::U8x4);
    let mut resizer = fr::Resizer::new();
    resizer.resize(&src, &mut dst, None).expect("resize succeeded");

    let buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(new_w, new_h, dst.into_vec())
        .expect("valid dst buffer");
    DynamicImage::ImageRgba8(buf)
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::fs;

    #[test]
    fn downscale_preserves_aspect_ratio() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 2, Rgba([120, 80, 40, 255])));
        let downscaled = downscale_fast(&img, 2);
        assert_eq!(downscaled.dimensions(), (2, 1));
    }

    #[test]
    fn save_frame_jpeg_writes_decodable_file() {
        let dir = std::env::temp_dir().join(format!(
            "screenrecall-capture-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.jpg");

        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 1, Rgba([1, 2, 3, 255])));
        save_frame(&img, &path, CaptureImageFormat::Jpeg, 80).unwrap();

        assert_eq!(image::image_dimensions(&path).unwrap(), (2, 1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn adaptive_state_flags_noisy_camera() {
        let mut s = AdaptiveState::default();
        for _ in 0..15 {
            s.push(true, 5);
        }
        s.evaluate();
        assert!(s.is_noisy);
    }

    #[test]
    fn adaptive_state_not_flagged_with_low_save_rate() {
        let mut s = AdaptiveState::default();
        for _ in 0..10 {
            s.push(true, 5);
            s.push(false, 0);
        }
        s.evaluate();
        assert!(!s.is_noisy);
    }

    #[test]
    fn adaptive_state_not_flagged_when_real_change_occurs() {
        let mut s = AdaptiveState::default();
        for _ in 0..14 {
            s.push(true, 5);
        }
        s.push(true, 14); // real scene change
        s.evaluate();
        assert!(!s.is_noisy);
    }

    #[test]
    fn adaptive_state_clears_on_real_change() {
        let mut s = AdaptiveState::default();
        for _ in 0..15 {
            s.push(true, 5);
        }
        s.evaluate();
        assert!(s.is_noisy);

        s.push(true, 14);
        s.evaluate();
        assert!(!s.is_noisy);
    }
}
