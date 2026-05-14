//! FFmpeg-based video archival of captured frames.
//!
//! Runs on a configurable interval, groups recent frames per monitor into
//! video segments, and optionally deletes the source frame files.

use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::{Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use tauri::Emitter;

use crate::config::AppConfig;
use crate::state::SharedState;
use crate::util::process::configure_hidden_tokio;

/// Known encoder presets with human-readable labels and ffmpeg encoder names.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub ffmpeg_encoder: &'static str,
    pub description: &'static str,
    /// Approximate compression ratio vs raw frames (lower = more compression).
    pub compression_ratio: f32,
    /// Whether this encoder requires hardware support.
    pub hardware_only: bool,
}

pub const KNOWN_ENCODERS: &[EncoderPreset] = &[
    EncoderPreset {
        id: "h264",
        label: "H.264 (software)",
        ffmpeg_encoder: "libx264",
        description: "Universal compatibility, fast encode",
        compression_ratio: 0.05,
        hardware_only: false,
    },
    EncoderPreset {
        id: "h264_qsv",
        label: "H.264 (Intel QSV)",
        ffmpeg_encoder: "h264_qsv",
        description: "Intel hardware encode, very fast",
        compression_ratio: 0.05,
        hardware_only: true,
    },
    EncoderPreset {
        id: "h264_nvenc",
        label: "H.264 (NVIDIA NVENC)",
        ffmpeg_encoder: "h264_nvenc",
        description: "NVIDIA hardware encode, very fast",
        compression_ratio: 0.05,
        hardware_only: true,
    },
    EncoderPreset {
        id: "h265",
        label: "H.265/HEVC (software)",
        ffmpeg_encoder: "libx265",
        description: "40-50% smaller than H.264, slower encode",
        compression_ratio: 0.03,
        hardware_only: false,
    },
    EncoderPreset {
        id: "h265_qsv",
        label: "H.265/HEVC (Intel QSV)",
        ffmpeg_encoder: "hevc_qsv",
        description: "Intel hardware HEVC encode",
        compression_ratio: 0.03,
        hardware_only: true,
    },
    EncoderPreset {
        id: "h265_nvenc",
        label: "H.265/HEVC (NVIDIA NVENC)",
        ffmpeg_encoder: "hevc_nvenc",
        description: "NVIDIA hardware HEVC encode",
        compression_ratio: 0.03,
        hardware_only: true,
    },
    EncoderPreset {
        id: "av1_qsv",
        label: "AV1 (Intel QSV)",
        ffmpeg_encoder: "av1_qsv",
        description: "Intel Arc hardware AV1, smallest files",
        compression_ratio: 0.02,
        hardware_only: true,
    },
    EncoderPreset {
        id: "av1_nvenc",
        label: "AV1 (NVIDIA NVENC)",
        ffmpeg_encoder: "av1_nvenc",
        description: "NVIDIA hardware AV1 encode",
        compression_ratio: 0.02,
        hardware_only: true,
    },
    EncoderPreset {
        id: "vp9",
        label: "VP9 (software)",
        ffmpeg_encoder: "libvpx-vp9",
        description: "Google's open codec, good compression",
        compression_ratio: 0.035,
        hardware_only: false,
    },
];

/// Result of probing available FFmpeg encoders.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderAvailability {
    pub ffmpeg_found: bool,
    pub ffmpeg_version: String,
    pub available_encoders: Vec<String>,
    pub recommended: String,
}

static ENCODER_CACHE: Mutex<Option<EncoderAvailability>> = Mutex::new(None);

/// Clear the cached encoder probe so the next call re-detects FFmpeg.
pub fn clear_encoder_cache() {
    if let Ok(mut g) = ENCODER_CACHE.lock() {
        *g = None;
    }
}

/// Verify a candidate ffmpeg binary actually runs.
async fn verify_ffmpeg(path: &str) -> bool {
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new(path)
            .arg("-version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let stdout = String::from_utf8_lossy(&o.stdout);
            stderr.contains("ffmpeg version") || stdout.contains("ffmpeg version")
        }
        _ => false,
    }
}

#[cfg(target_os = "windows")]
async fn find_ffmpeg_via_where() -> Option<String> {
    let output = Command::new("cmd")
        .args(["/c", "where", "ffmpeg.exe"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.ends_with("ffmpeg.exe") && verify_ffmpeg(trimmed).await {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Search PATH first, then common install locations for ffmpeg binary.
pub async fn find_ffmpeg_binary() -> Option<String> {
    if verify_ffmpeg("ffmpeg").await {
        return Some("ffmpeg".into());
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = find_ffmpeg_via_where().await {
            return Some(path);
        }

        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let data_local = directories::BaseDirs::new().map(|d| d.data_local_dir().to_path_buf());

        let candidates: Vec<std::path::PathBuf> = vec![
            data_local.as_ref().map(|d| d.join("Programs").join("Gyan.FFmpeg").join("bin").join("ffmpeg.exe")),
            data_local.as_ref().map(|d| d.join("Programs").join("FFmpeg").join("bin").join("ffmpeg.exe")),
            data_local.as_ref().map(|d| d.join("Microsoft").join("WinGet").join("Packages").join("Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe").join("ffmpeg").join("bin").join("ffmpeg.exe")),
            Some(std::path::PathBuf::from(r"C:\Program Files\ffmpeg\bin\ffmpeg.exe")),
            Some(std::path::PathBuf::from(r"C:\Program Files (x86)\ffmpeg\bin\ffmpeg.exe")),
            Some(std::path::PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe")),
            Some(std::path::PathBuf::from(r"C:\tools\ffmpeg\bin\ffmpeg.exe")),
            home.as_ref().map(|d| d.join("scoop").join("apps").join("ffmpeg").join("current").join("bin").join("ffmpeg.exe")),
            home.as_ref().map(|d| d.join("scoop").join("shims").join("ffmpeg.exe")),
        ]
        .into_iter()
        .flatten()
        .collect();

        for candidate in candidates {
            let path = candidate.to_string_lossy().to_string();
            if verify_ffmpeg(&path).await {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for p in [
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
        ] {
            if verify_ffmpeg(p).await {
                return Some(p.into());
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for p in [
            "/usr/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/snap/bin/ffmpeg",
        ] {
            if verify_ffmpeg(p).await {
                return Some(p.into());
            }
        }
    }

    None
}

/// Probe FFmpeg once and cache the result.
pub async fn probe_encoders() -> EncoderAvailability {
    if let Ok(Some(cached)) = ENCODER_CACHE.lock().map(|g| g.clone()) {
        return cached;
    }
    let result = do_probe().await;
    if let Ok(mut g) = ENCODER_CACHE.lock() {
        *g = Some(result.clone());
    }
    result
}

async fn do_probe() -> EncoderAvailability {
    let mut out = EncoderAvailability::default();

    let ffmpeg_path = find_ffmpeg_binary().await;

    if let Some(ffmpeg) = &ffmpeg_path {
        let version_out = Command::new(ffmpeg)
            .arg("-version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        if let Ok(o) = version_out {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let stdout = String::from_utf8_lossy(&o.stdout);
            if stderr.contains("ffmpeg version") || stdout.contains("ffmpeg version") {
                out.ffmpeg_found = true;
                let text = if stderr.contains("ffmpeg version") { stderr } else { stdout };
                out.ffmpeg_version = text.lines().next().unwrap_or("unknown").to_string();
            }
        }
    }
    if !out.ffmpeg_found {
        out.recommended = "h264".into();
        return out;
    }

    // List available encoders.
    let enc_out = Command::new(&ffmpeg_path.unwrap())
        .args(["-encoders"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;
    if let Ok(o) = enc_out {
        // Some ffmpeg builds write -encoders to stderr; read both.
        let stdout_text = String::from_utf8_lossy(&o.stdout);
        let stderr_text = String::from_utf8_lossy(&o.stderr);
        let text = if stdout_text.trim().is_empty() {
            stderr_text.as_ref()
        } else {
            stdout_text.as_ref()
        };
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2
                && parts[0].len() == 6
                && parts[0].starts_with('V')
                && !parts[1].starts_with('=')
            {
                out.available_encoders.push(parts[1].to_string());
            }
        }
    }

    // Pick recommended encoder: prefer hardware AV1 > hardware HEVC > hardware H264 > software H264.
    let hw_order = ["av1_qsv", "av1_nvenc", "hevc_qsv", "hevc_nvenc", "h264_qsv", "h264_nvenc"];
    for enc in &hw_order {
        if out.available_encoders.contains(&enc.to_string()) {
            out.recommended = enc.to_string();
            return out;
        }
    }
    if out.available_encoders.contains(&"libx264".to_string()) {
        out.recommended = "h264".into();
    } else {
        out.recommended = "h264".into(); // fallback, will fail gracefully
    }
    out
}

/// Find the encoder preset by id.
pub fn find_preset(id: &str) -> Option<&'static EncoderPreset> {
    KNOWN_ENCODERS.iter().find(|p| p.id == id)
}

/// Archiver state for the diagnostics page.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiverStatus {
    pub enabled: bool,
    pub running: bool,
    pub last_run_ts: Option<i64>,
    pub last_duration_ms: Option<u64>,
    pub next_run_ts: Option<i64>,
    pub total_archived: i64,
    pub total_segments: i64,
    pub total_source_deleted: i64,
    pub last_error: Option<String>,
}

static ARCHIVER_STATUS: Mutex<ArchiverStatus> = Mutex::new(ArchiverStatus {
    enabled: false,
    running: false,
    last_run_ts: None,
    last_duration_ms: None,
    next_run_ts: None,
    total_archived: 0,
    total_segments: 0,
    total_source_deleted: 0,
    last_error: None,
});

pub fn get_archiver_status() -> ArchiverStatus {
    ARCHIVER_STATUS.lock().unwrap().clone()
}

fn set_archiver_status(f: impl FnOnce(&mut ArchiverStatus)) {
    if let Ok(mut g) = ARCHIVER_STATUS.lock() {
        f(&mut g);
    }
}

/// Main archiver worker loop. Runs immediately, then sleeps for `archive_interval_secs`.
pub async fn run_worker(state: SharedState) -> Result<()> {
    info!("archiver worker started");

    let cfg = state.config();
    if cfg.archive_enabled {
        set_archiver_status(|s| {
            s.enabled = true;
            s.running = false;
        });
    }

    loop {
        let cfg = state.config();
        if !cfg.archive_enabled {
            set_archiver_status(|s| {
                s.enabled = false;
                s.running = false;
            });
            let sleep_secs = cfg.archive_interval_secs.max(60);
            set_archiver_status(|s| s.next_run_ts = Some(Utc::now().timestamp_millis() + sleep_secs as i64 * 1000));
            tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
            continue;
        }

        set_archiver_status(|s| {
            s.enabled = true;
            s.running = true;
            s.last_error = None;
        });

        let now = Utc::now().timestamp_millis();
        let status = get_archiver_status();
        let min_ts = match status.last_run_ts {
            Some(ts) => {
                if cfg.archive_max_lookback_secs > 0 {
                    ts.max(now - (cfg.archive_max_lookback_secs as i64 * 1000))
                } else {
                    ts
                }
            }
            None => now - (cfg.archive_interval_secs as i64 * 1000).max(60_000),
        };
        let cutoff = now - 1000;

        let started = std::time::Instant::now();
        match archive_segment(&state, cutoff, min_ts, now, &cfg).await {
            Ok((archived, segments, deleted)) => {
                set_archiver_status(|s| {
                    s.running = false;
                    s.last_run_ts = Some(now);
                    s.last_duration_ms = Some(started.elapsed().as_millis() as u64);
                    s.total_archived += archived as i64;
                    s.total_segments += segments as i64;
                    s.total_source_deleted += deleted as i64;
                });
                if archived > 0 {
                    info!(archived, segments, "archiver run complete");
                }
                if cfg.archive_keep_frames_days > 0 {
                    let cut = now - (cfg.archive_keep_frames_days as i64 * 24 * 3600 * 1000);
                    if let Ok(deleted) = state.store.delete_expired_source_files(cut) {
                        let n = deleted.len();
                        if n > 0 {
                            set_archiver_status(|s| s.total_source_deleted += n as i64);
                            info!(deleted = n, "deleted archived source files past keep threshold");
                        }
                    }
                }
            }
            Err(e) => {
                set_archiver_status(|s| {
                    s.running = false;
                    s.last_error = Some(e.to_string());
                });
                warn!(?e, "archiver run failed");
            }
        }

        let sleep_secs = cfg.archive_interval_secs.max(60);
        set_archiver_status(|s| s.next_run_ts = Some(Utc::now().timestamp_millis() + sleep_secs as i64 * 1000));
        tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
    }
}

/// Archive all unarchived frames (backfill / history mode), ignoring the cutoff.
pub async fn archive_history(state: &SharedState, cfg: &AppConfig) -> Result<(usize, usize, usize)> {
    set_archiver_status(|s| {
        s.enabled = cfg.archive_enabled;
        s.running = true;
        s.last_error = None;
    });
    let started = std::time::Instant::now();
    let now = Utc::now().timestamp_millis();
    let result = archive_segment(state, i64::MAX, 0, now, cfg).await;
    match &result {
        Ok((archived, segments, deleted)) => {
            set_archiver_status(|s| {
                s.running = false;
                s.last_run_ts = Some(now);
                s.last_duration_ms = Some(started.elapsed().as_millis() as u64);
                s.total_archived += *archived as i64;
                s.total_segments += *segments as i64;
                s.total_source_deleted += *deleted as i64;
            });
            info!(archived, segments, "history archive run complete");
        }
        Err(e) => {
            set_archiver_status(|s| {
                s.running = false;
                s.last_error = Some(e.to_string());
            });
            warn!(?e, "history archive run failed");
        }
    }
    result
}

/// Archive all unarchived frames older than `cutoff` into video segments per monitor.
async fn archive_segment(
    state: &SharedState,
    cutoff_ms: i64,
    min_ts: i64,
    now_ms: i64,
    cfg: &AppConfig,
) -> Result<(usize, usize, usize)> {
    // Find monitors that have unarchived frames before cutoff
    // and after min_ts (to avoid backfilling huge history automatically).
    let monitors = state.store.list_monitors_with_unarchived_range(min_ts, cutoff_ms)?;
    if monitors.is_empty() {
        return Ok((0, 0, 0));
    }

    let preset = find_preset(&cfg.archive_codec)
        .ok_or_else(|| anyhow!("unknown archive codec: {}", cfg.archive_codec))?;

    let mut total_archived = 0;
    let mut total_segments = 0;
    let total_deleted = 0;
    let video_dir = state.config().data_dir.join("videos");
    std::fs::create_dir_all(&video_dir)?;

    for monitor_id in monitors {
        let frames = state.store.list_unarchived_for_monitor_range(monitor_id, min_ts, cutoff_ms)?;
        if frames.is_empty() {
            continue;
        }

        // Group frames into segments of `archive_segment_secs`.
        let segment_ms = cfg.archive_segment_secs * 1000;
        let mut segments: Vec<Vec<(i64, i64, String)>> = Vec::new();
        let mut current: Vec<(i64, i64, String)> = Vec::new();
        let mut segment_start: Option<i64> = None;

        for f in &frames {
            let (_id, ts, _path) = f;
            let start = segment_start.get_or_insert(*ts);
            if *ts - *start >= segment_ms as i64 && !current.is_empty() {
                segments.push(std::mem::take(&mut current));
                segment_start = Some(*ts);
            }
            current.push((*f).clone());
        }
        if !current.is_empty() {
            segments.push(current);
        }

        for segment in &segments {
            if segment.len() < 2 {
                continue;
            }
            let first_ts = segment[0].1;
            let last_ts = segment.last().unwrap().1;
            let duration_s = ((last_ts - first_ts) as f64 / 1000.0).max(0.5);

            // Build video path: videos/YYYY/MM/DD/HHMMSS_m{monitor}.mp4
            let first_frame_ts = segment[0].1;
            let dt = chrono::DateTime::<Utc>::from_timestamp_millis(first_frame_ts)
                .unwrap_or_else(|| chrono::Utc::now());
            let rel = format!(
                "{:04}/{:02}/{:02}/{:02}{:02}{:02}_m{}.mp4",
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second(),
                monitor_id,
            );
            let video_path = video_dir.join(&rel);
            if let Some(parent) = video_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Create a temp file list for ffmpeg concat demuxer.
            // Images (webp) have no inherent duration, so we must explicitly
            // specify a duration for EVERY frame including the last one.
            // Otherwise the last frame has duration 0 and seeking past it hits EOF.
            // Cap per-frame duration at 1.0s so the video never plays slower than 1 fps
            // (avoids 2 frames over 15 min => 7.5 min per frame).
            let list_path = video_path.with_extension("txt");
            let mut list_content = String::new();
            let avg_frame_duration = (duration_s / segment.len() as f64).min(1.0);
            for i in 0..segment.len() {
                let (_, _, frame_path) = &segment[i];
                let escaped = frame_path.replace("'", "'\\''");
                list_content.push_str(&format!("file '{}
", escaped));
                list_content.push_str(&format!("duration {:.3}
", avg_frame_duration));
            }
            std::fs::write(&list_path, &list_content)?;

            // Run ffmpeg.
            // WebP captures may have alpha (ARGB); force conversion to yuv420p via
            // filter so the output is not black in standard players.
            // JPEG frames are already RGB — skip the filter to avoid unnecessary CPU work.
            let has_webp = segment.iter().any(|(_, _, path)| {
                path.to_ascii_lowercase().ends_with(".webp")
            });
            let encoder = &preset.ffmpeg_encoder;
            let ffmpeg_bin = find_ffmpeg_binary().await
                .ok_or_else(|| anyhow!("ffmpeg not found in PATH or common install locations"))?;
            let list_path_str = list_path.to_string_lossy().to_string();
            let video_path_str = video_path.to_string_lossy().to_string();
            let mut cmd = Command::new(&ffmpeg_bin);
            let mut args: Vec<&str> = vec![
                "-y",
                "-f", "concat",
                "-safe", "0",
                "-i", &list_path_str,
            ];
            if has_webp {
                args.push("-vf");
                args.push("format=yuv420p");
            }
            args.extend_from_slice(&[
                "-c:v", encoder,
                "-preset", "veryfast",
                "-vsync", "vfr",
                "-pix_fmt", "yuv420p",
            ]);
            args.push(&video_path_str);
            cmd.args(&args);
            configure_hidden_tokio(&mut cmd);
            cmd.stdin(Stdio::null());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            let output = cmd.output().await?;
            let _ = std::fs::remove_file(&list_path);

            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                let lines: Vec<&str> = err.lines().collect();
                // FFmpeg prints a multi-line version banner at the start of stderr.
                // The actual error is near the end, so show the last lines.
                let tail: Vec<&str> = lines.into_iter().rev().take(8).rev().collect();
                return Err(anyhow!("ffmpeg failed: {}", tail.join(" | ")));
            }

            // Update DB: set video_path, video_offset_ms, and archived_at for each frame.
            let offsets: Vec<(i64, i64)> = segment
                .iter()
                .map(|(id, offset_ms, _)| (*id, *offset_ms))
                .collect();
            state.store.set_video_archive(&video_path.to_string_lossy(), &offsets, now_ms)?;

            total_archived += segment.len();
            total_segments += 1;
            debug!(
                monitor = monitor_id,
                frames = segment.len(),
                video = %video_path.display(),
                "archived segment"
            );
        }
    }

    Ok((total_archived, total_segments, total_deleted))
}

/// Re-encode all existing archived video files to a new codec.
/// Returns (success_count, fail_count).
pub async fn transcode_all_videos(
    state: &SharedState,
    target_codec: &str,
    app: tauri::AppHandle,
) -> Result<(usize, usize)> {
    let preset = find_preset(target_codec)
        .ok_or_else(|| anyhow!("unknown archive codec: {}", target_codec))?;

    let video_paths = state.store.list_archived_video_paths()?;
    let total = video_paths.len();
    if total == 0 {
        return Ok((0, 0));
    }

    let ffmpeg_bin = find_ffmpeg_binary().await
        .ok_or_else(|| anyhow!("ffmpeg not found in PATH or common install locations"))?;

    let mut success = 0usize;
    let mut failed = 0usize;

    for (i, video_path_str) in video_paths.iter().enumerate() {
        let video_path = std::path::PathBuf::from(video_path_str);
        if !video_path.exists() {
            failed += 1;
            continue;
        }

        let _ = app.emit(
            "transcode:progress",
            serde_json::json!({
                "current": i + 1,
                "total": total,
                "file": video_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                "status": "running",
            }),
        );

        let temp_path = video_path.with_extension("tmp.mp4");
        let encoder = &preset.ffmpeg_encoder;

        let mut cmd = Command::new(&ffmpeg_bin);
        cmd.args([
            "-y",
            "-i", &video_path.to_string_lossy(),
            "-c:v", encoder,
            "-preset", "veryfast",
            "-pix_fmt", "yuv420p",
            "-an", // no audio
            &temp_path.to_string_lossy(),
        ]);
        configure_hidden_tokio(&mut cmd);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        match cmd.output().await {
            Ok(output) if output.status.success() => {
                // Replace original with transcoded file.
                match std::fs::rename(&temp_path, &video_path) {
                    Ok(_) => {
                        success += 1;
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&temp_path);
                        warn!(?e, path = %video_path.display(), "failed to replace original with transcoded file");
                        failed += 1;
                    }
                }
            }
            Ok(output) => {
                let _ = std::fs::remove_file(&temp_path);
                let err = String::from_utf8_lossy(&output.stderr);
                let lines: Vec<&str> = err.lines().collect();
                let tail: Vec<&str> = lines.into_iter().rev().take(5).rev().collect();
                warn!(path = %video_path.display(), error = %tail.join(" | "), "transcode failed");
                failed += 1;
            }
            Err(e) => {
                let _ = std::fs::remove_file(&temp_path);
                warn!(?e, path = %video_path.display(), "transcode command failed");
                failed += 1;
            }
        }
    }

    let _ = app.emit(
        "transcode:progress",
        serde_json::json!({
            "current": total,
            "total": total,
            "file": "",
            "status": "done",
            "success": success,
            "failed": failed,
        }),
    );

    Ok((success, failed))
}
