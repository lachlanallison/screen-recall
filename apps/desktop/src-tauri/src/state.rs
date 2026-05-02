use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use anyhow::{Context, Result};
use tauri::AppHandle;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::AppConfig;
use crate::embed;
use crate::store::Store;
use crate::util::paths;
use crate::util::process::configure_hidden_tokio;
use crate::worker_activity::WorkerActivity;

/// Handle to the OCR worker queue (re-queue frame ids for reprocessing).
#[derive(Clone)]
pub struct OcrQueue(pub mpsc::UnboundedSender<i64>);

pub struct ManagedLlamaProcess {
    pub command: String,
    pub cwd: Option<String>,
    pub started_at: Instant,
    pub stdout_path: String,
    pub stderr_path: String,
    pub child: tokio::process::Child,
}

#[derive(Clone, Debug, Default)]
pub struct ManagedLlamaLast {
    pub exit_code: Option<i32>,
    pub stderr_tail: Option<String>,
}

/// Singleton app state. Cloned cheaply via `Arc`.
pub struct AppState {
    pub app: AppHandle,
    pub store: Store,
    /// Current config. RW-locked for hot updates from Settings.
    config: RwLock<AppConfig>,
    config_path: PathBuf,
    /// Recording on/off toggle.
    recording: AtomicBool,
    /// Windows: session locked (Win+L) — capture skipped when `pause_when_workstation_locked` is on.
    workstation_locked: AtomicBool,
    /// Registry of cancelled chat sessions.
    pub cancelled_sessions: AsyncMutex<std::collections::HashSet<String>>,
    /// Which frame the OCR / embed workers are processing, for Diagnostics UI.
    pub worker_activity: std::sync::Arc<WorkerActivity>,
    /// Optional managed local LLM server processes (chat/embed) launched from Settings.
    pub managed_llama: AsyncMutex<std::collections::HashMap<String, ManagedLlamaProcess>>,
    /// Last known exit details for managed servers.
    pub managed_llama_last: AsyncMutex<std::collections::HashMap<String, ManagedLlamaLast>>,
    /// Cached on-disk frames size in MiB; maintained by a background refresher.
    cached_disk_mb: std::sync::Mutex<f64>,
    /// Cached counts for cheap `get_stats` reads.
    cached_frame_count: std::sync::Mutex<i64>,
    cached_indexed_count: std::sync::Mutex<i64>,
}

impl AppState {
    pub async fn new(app: AppHandle) -> Result<Self> {
        let (default_data_dir, config_path) = default_paths()?;
        std::fs::create_dir_all(&default_data_dir).context("create default data_dir")?;

        let config = AppConfig::load_or_default(&config_path, &default_data_dir)?;
        std::fs::create_dir_all(&config.data_dir).context("create configured data_dir")?;

        let db_path = config.data_dir.join(paths::SQLITE_DB_FILE);
        let store = Store::open(&db_path)?;

        Ok(Self {
            app,
            store,
            config: RwLock::new(config),
            config_path,
            recording: AtomicBool::new(true),
            workstation_locked: AtomicBool::new(false),
            cancelled_sessions: AsyncMutex::new(Default::default()),
            worker_activity: std::sync::Arc::new(WorkerActivity::default()),
            managed_llama: AsyncMutex::new(Default::default()),
            managed_llama_last: AsyncMutex::new(Default::default()),
            cached_disk_mb: std::sync::Mutex::new(0.0),
            cached_frame_count: std::sync::Mutex::new(0),
            cached_indexed_count: std::sync::Mutex::new(0),
        })
    }

    pub fn config(&self) -> AppConfig {
        self.config.read().expect("config lock poisoned").clone()
    }

    pub fn set_config(&self, new_config: AppConfig) -> Result<()> {
        new_config.save(&self.config_path)?;
        *self.config.write().expect("config lock poisoned") = new_config;
        embed::reset_ollama_fallback_log_signature();
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    pub fn set_recording(&self, on: bool) {
        self.recording.store(on, Ordering::Relaxed);
    }

    /// True when the OS reports the user session is locked (e.g. Win+L on Windows).
    pub fn is_workstation_locked(&self) -> bool {
        self.workstation_locked.load(Ordering::Relaxed)
    }

    /// Called from the platform lock monitor when lock/unlock is detected.
    pub(crate) fn set_workstation_locked(&self, locked: bool) {
        let before = self.workstation_locked.swap(locked, Ordering::Relaxed);
        if before != locked {
            info!(locked, "session lock state changed (skips capture when setting enabled)");
        }
    }

    pub fn frames_dir(&self) -> PathBuf {
        self.config().data_dir.join("frames")
    }

    pub fn set_cached_disk_mb(&self, mb: f64) {
        if let Ok(mut g) = self.cached_disk_mb.lock() {
            *g = mb.max(0.0);
        }
    }

    pub fn cached_disk_mb(&self) -> f64 {
        self.cached_disk_mb.lock().map(|g| *g).unwrap_or(0.0)
    }

    pub fn set_cached_counts(&self, frame_count: i64, indexed_count: i64) {
        if let Ok(mut g) = self.cached_frame_count.lock() {
            *g = frame_count.max(0);
        }
        if let Ok(mut g) = self.cached_indexed_count.lock() {
            *g = indexed_count.max(0);
        }
    }

    pub fn cached_counts(&self) -> (i64, i64) {
        let frames = self.cached_frame_count.lock().map(|g| *g).unwrap_or(0);
        let indexed = self.cached_indexed_count.lock().map(|g| *g).unwrap_or(0);
        (frames, indexed)
    }

    /// Best-effort stop of all managed llama.cpp child processes.
    pub async fn stop_all_managed_llama(&self) -> usize {
        let mut map = self.managed_llama.lock().await;
        let mut stopped = 0usize;
        for (_kind, mut proc) in map.drain() {
            let _ = proc.child.start_kill();
            let _ = proc.child.wait().await;
            stopped += 1;
        }
        stopped
    }

    /// Synchronous wrapper for shutdown hooks that can't be `async`.
    pub fn stop_all_managed_llama_blocking(&self) -> usize {
        tauri::async_runtime::block_on(self.stop_all_managed_llama())
    }
}

pub fn shell_spawn(
    command: &str,
    cwd: Option<&str>,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<tokio::process::Child> {
    fn split_command_line(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut escaped = false;
        for ch in s.chars() {
            if escaped {
                cur.push(ch);
                escaped = false;
                continue;
            }
            // On Windows, backslashes are path separators and should not be consumed as escapes.
            if !cfg!(target_os = "windows") && ch == '\\' && !in_single {
                escaped = true;
                continue;
            }
            if ch == '\'' && !in_double {
                in_single = !in_single;
                continue;
            }
            if ch == '"' && !in_single {
                in_double = !in_double;
                continue;
            }
            if ch.is_whitespace() && !in_single && !in_double {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                continue;
            }
            cur.push(ch);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    fn strip_weird_wrap_quotes(mut s: String) -> String {
        loop {
            let t = s.trim().to_string();
            let bytes = t.as_bytes();
            if bytes.len() >= 2
                && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
                    || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
            {
                s = t[1..t.len() - 1].to_string();
                continue;
            }
            return t;
        }
    }

    let mut parts = split_command_line(command);
    if parts.is_empty() {
        return Err(anyhow::anyhow!("empty command"));
    }
    for i in 0..parts.len() {
        if parts[i] == "-m" || parts[i] == "--model" {
            if i + 1 < parts.len() {
                parts[i + 1] = strip_weird_wrap_quotes(parts[i + 1].clone());
            }
            continue;
        }
        if let Some(rest) = parts[i].strip_prefix("--model=") {
            parts[i] = format!("--model={}", strip_weird_wrap_quotes(rest.to_string()));
        }
    }
    let exe = parts[0].clone();
    let mut cmd = tokio::process::Command::new(&exe);
    if parts.len() > 1 {
        cmd.args(&parts[1..]);
    }
    if let Some(wd) = cwd {
        if !wd.trim().is_empty() {
            cmd.current_dir(wd.trim());
        }
    }
    if let Some(parent) = stdout_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Some(parent) = stderr_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stdout_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(stdout_path)
        .context("open managed stdout log")?;
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_path)
        .context("open managed stderr log")?;
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(stdout_file));
    cmd.stderr(Stdio::from(stderr_file));
    configure_hidden_tokio(&mut cmd);
    cmd.kill_on_drop(true);
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn managed shell command: exe={exe} cwd={:?}", cwd))?;
    Ok(child)
}

/// Default data directory and config file location (`~/.screenrecall/`).
fn default_paths() -> Result<(PathBuf, PathBuf)> {
    let home = directories::BaseDirs::new()
        .context("could not find home directory")?
        .home_dir()
        .to_path_buf();
    let data_dir = home.join(paths::DATA_DIR_NAME);
    let config_path = data_dir.join("config.json");
    Ok((data_dir, config_path))
}

/// Convenience alias for command handlers and background tasks.
pub type SharedState = Arc<AppState>;
