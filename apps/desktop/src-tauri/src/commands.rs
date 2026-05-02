//! Tauri IPC surface.
//!
//! Every command here is invoked from the React frontend via `invoke(...)`.
//! Errors are serialized as strings so they can be `.catch()`ed on the JS
//! side without needing a custom deserialization path.

use std::sync::Arc;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;

use crate::config::{AppConfig, LlmBackend};
use crate::llm::{self, prompts, ChatMessage};
use crate::search;
use crate::state::{shell_spawn, AppState, ManagedLlamaLast, ManagedLlamaProcess, OcrQueue};
use crate::store::Frame;
use crate::util::paths;
use crate::util::perf_log;
use crate::util::process::configure_hidden_std;
use crate::util::process_log;
use crate::util::reveal_in_folder;
use crate::worker_activity::{OneWorkerQueue, WorkerQueuesSnapshot};

type AppStateHandle<'a> = State<'a, Arc<AppState>>;

pub type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn read_tail_lines(path: &Path, max_lines: usize, max_bytes: u64) -> std::io::Result<Vec<String>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max_bytes.max(1));
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(|x| x.to_string()).collect();
    // If we started from the middle of the file, the first line may be partial/truncated.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > max_lines {
        lines.drain(0..(lines.len() - max_lines));
    }
    Ok(lines)
}

fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    configure_hidden_std(&mut cmd);
    cmd
}

fn resolve_tesseract_for_check() -> Option<std::path::PathBuf> {
    let candidates = [
        std::path::PathBuf::from("tesseract.exe"),
        std::path::PathBuf::from("tesseract"),
        std::path::PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe"),
        std::path::PathBuf::from(r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe"),
    ];

    for c in candidates {
        let ok = hidden_command(&c)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(c);
        }
    }
    None
}

#[derive(Serialize, Clone)]
pub struct DependencyStatus {
    pub key: String,
    pub label: String,
    pub status: String,
    pub detail: String,
}

#[derive(Serialize, Clone)]
pub struct DependencyReport {
    pub ok: bool,
    pub items: Vec<DependencyStatus>,
}

/* ------------------------------------------------------------------ */
/* Config / status                                                    */
/* ------------------------------------------------------------------ */

#[derive(Serialize)]
pub struct Status {
    pub recording: bool,
}

#[tauri::command]
pub fn get_status(state: AppStateHandle<'_>) -> CmdResult<Status> {
    Ok(Status {
        recording: state.is_recording(),
    })
}

#[tauri::command]
pub fn set_recording(state: AppStateHandle<'_>, on: bool) -> CmdResult<()> {
    state.set_recording(on);
    Ok(())
}

#[tauri::command]
pub fn get_config(state: AppStateHandle<'_>) -> CmdResult<AppConfig> {
    Ok(state.config())
}

#[tauri::command]
pub fn set_config(state: AppStateHandle<'_>, config: AppConfig) -> CmdResult<()> {
    state.set_config(config).map_err(err)
}

#[derive(Serialize)]
pub struct LlmConnectionTest {
    pub ok: bool,
    pub detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedLlamaStatus {
    pub kind: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub started_ms_ago: Option<u64>,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub last_exit_code: Option<i32>,
    pub last_stderr_tail: Option<String>,
}

const MANAGED_KINDS: [&str; 2] = ["chat", "embed"];

fn normalize_managed_kind(kind: &str) -> CmdResult<&'static str> {
    let k = kind.trim().to_ascii_lowercase();
    match k.as_str() {
        "chat" => Ok("chat"),
        "embed" | "embeddings" => Ok("embed"),
        _ => Err("kind must be 'chat' or 'embed'".into()),
    }
}

async fn managed_status_for_kind(state: &Arc<AppState>, kind: &'static str) -> ManagedLlamaStatus {
    fn tail_chars(s: &str, n: usize) -> String {
        let mut v: Vec<char> = s.chars().collect();
        if v.len() > n {
            v.drain(0..(v.len() - n));
        }
        v.into_iter().collect()
    }
    let mut map = state.managed_llama.lock().await;
    let mut last_map = state.managed_llama_last.lock().await;
    if let Some(proc) = map.get_mut(kind) {
        let wait_res = proc.child.try_wait();
        let running = match wait_res {
            Ok(None) => true,
            Ok(Some(_)) | Err(_) => false,
        };
        if running {
            let last = last_map.get(kind).cloned().unwrap_or_default();
            return ManagedLlamaStatus {
                kind: kind.to_string(),
                running: true,
                pid: proc.child.id(),
                command: Some(proc.command.clone()),
                cwd: proc.cwd.clone(),
                started_ms_ago: Some(proc.started_at.elapsed().as_millis() as u64),
                stdout_path: Some(proc.stdout_path.clone()),
                stderr_path: Some(proc.stderr_path.clone()),
                last_exit_code: last.exit_code,
                last_stderr_tail: last.stderr_tail,
            };
        }
        if let Ok(Some(status)) = wait_res {
            let stderr_tail = std::fs::read_to_string(&proc.stderr_path)
                .ok()
                .map(|s| tail_chars(&s, 1200));
            last_map.insert(
                kind.to_string(),
                ManagedLlamaLast {
                    exit_code: status.code(),
                    stderr_tail,
                },
            );
        }
        map.remove(kind);
    }
    let last = last_map.get(kind).cloned().unwrap_or_default();
    ManagedLlamaStatus {
        kind: kind.to_string(),
        running: false,
        pid: None,
        command: None,
        cwd: None,
        started_ms_ago: None,
        stdout_path: None,
        stderr_path: None,
        last_exit_code: last.exit_code,
        last_stderr_tail: last.stderr_tail,
    }
}

#[tauri::command]
pub async fn get_managed_llama_status(state: AppStateHandle<'_>) -> CmdResult<Vec<ManagedLlamaStatus>> {
    let shared: Arc<AppState> = state.inner().clone();
    let mut out = Vec::with_capacity(MANAGED_KINDS.len());
    for kind in MANAGED_KINDS {
        out.push(managed_status_for_kind(&shared, kind).await);
    }
    Ok(out)
}

#[tauri::command]
pub async fn start_managed_llama(
    state: AppStateHandle<'_>,
    kind: String,
    command: String,
    cwd: Option<String>,
) -> CmdResult<ManagedLlamaStatus> {
    let kind = normalize_managed_kind(&kind)?;
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("Command is empty".into());
    }
    let shared: Arc<AppState> = state.inner().clone();
    let mut map = shared.managed_llama.lock().await;
    let mut last_map = shared.managed_llama_last.lock().await;
    if let Some(existing) = map.get_mut(kind) {
        if matches!(existing.child.try_wait(), Ok(None)) {
            return Err(format!("Managed {} server is already running (pid {:?})", kind, existing.child.id()));
        }
        map.remove(kind);
    }
    let log_dir = state.config().data_dir.join("managed-llama");
    let stdout_path = log_dir.join(format!("{kind}.stdout.log"));
    let stderr_path = log_dir.join(format!("{kind}.stderr.log"));
    process_log::record(
        &shared,
        "managed_llama_start",
        "custom_command",
        &[command.clone()],
        cwd.as_deref(),
    );
    let child = shell_spawn(&command, cwd.as_deref(), &stdout_path, &stderr_path).map_err(err)?;
    let status = ManagedLlamaStatus {
        kind: kind.to_string(),
        running: true,
        pid: child.id(),
        command: Some(command.clone()),
        cwd: cwd.clone(),
        started_ms_ago: Some(0),
        stdout_path: Some(stdout_path.to_string_lossy().to_string()),
        stderr_path: Some(stderr_path.to_string_lossy().to_string()),
        last_exit_code: None,
        last_stderr_tail: None,
    };
    map.insert(
        kind.to_string(),
        ManagedLlamaProcess {
            command,
            cwd,
            started_at: std::time::Instant::now(),
            stdout_path: stdout_path.to_string_lossy().to_string(),
            stderr_path: stderr_path.to_string_lossy().to_string(),
            child,
        },
    );
    last_map.remove(kind);
    drop(last_map);
    drop(map);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let current = managed_status_for_kind(&shared, kind).await;
    if !current.running {
        let detail = current
            .last_stderr_tail
            .clone()
            .unwrap_or_else(|| "process exited immediately (no stderr captured)".into());
        return Err(format!(
            "Managed {} server exited immediately (code {:?}). {}",
            kind, current.last_exit_code, detail
        ));
    }
    Ok(status)
}

#[tauri::command]
pub async fn stop_managed_llama(
    state: AppStateHandle<'_>,
    kind: String,
) -> CmdResult<ManagedLlamaStatus> {
    let kind = normalize_managed_kind(&kind)?;
    let shared: Arc<AppState> = state.inner().clone();
    let mut map = shared.managed_llama.lock().await;
    if let Some(mut proc) = map.remove(kind) {
        let _ = proc.child.start_kill();
        let _ = proc.child.wait().await;
    }
    Ok(ManagedLlamaStatus {
        kind: kind.to_string(),
        running: false,
        pid: None,
        command: None,
        cwd: None,
        started_ms_ago: None,
        stdout_path: None,
        stderr_path: None,
        last_exit_code: None,
        last_stderr_tail: None,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedLlamaStartBothResult {
    pub started: Vec<String>,
    pub skipped: Vec<String>,
}

#[tauri::command]
pub async fn start_managed_llama_both(state: AppStateHandle<'_>) -> CmdResult<ManagedLlamaStartBothResult> {
    let cfg = state.config();
    let cwd = cfg.managed_server_working_dir.clone();
    let mut started = Vec::new();
    let mut skipped = Vec::new();
    if let Some(cmd) = cfg.managed_chat_server_command.clone().filter(|s| !s.trim().is_empty()) {
        start_managed_llama(state.clone(), "chat".into(), cmd, cwd.clone()).await?;
        started.push("chat".into());
    } else {
        skipped.push("chat".into());
    }
    if let Some(cmd) = cfg.managed_embed_server_command.clone().filter(|s| !s.trim().is_empty()) {
        start_managed_llama(state.clone(), "embed".into(), cmd, cwd.clone()).await?;
        started.push("embed".into());
    } else {
        skipped.push("embed".into());
    }
    Ok(ManagedLlamaStartBothResult { started, skipped })
}

#[tauri::command]
pub fn get_managed_llama_log_tail(
    state: AppStateHandle<'_>,
    kind: String,
    stream: Option<String>,
    limit: Option<usize>,
) -> CmdResult<Vec<String>> {
    let kind = normalize_managed_kind(&kind)?;
    let stream = stream
        .as_deref()
        .unwrap_or("stderr")
        .trim()
        .to_ascii_lowercase();
    let suffix = if stream == "stdout" { "stdout" } else { "stderr" };
    let max = limit.unwrap_or(200).clamp(1, 2000);
    let path = state
        .config()
        .data_dir
        .join("managed-llama")
        .join(format!("{kind}.{suffix}.log"));
    match read_tail_lines(&path, max, 512 * 1024) {
        Ok(lines) => Ok(lines),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(err(e)),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    /// On-disk `screenrecall.db` size (approximate RSS impact if fully mmapped without limits).
    pub sqlite_main_mb: f64,
    pub sqlite_wal_mb: f64,
    pub sqlite_shm_mb: f64,
    /// Quick probe of common install locations only (PATH-only installs not detected here).
    pub tesseract_known_path: Option<String>,
    pub managed_servers: Vec<ManagedLlamaStatus>,
}

fn file_metadata_len_mb(path: &std::path::Path) -> f64 {
    std::fs::metadata(path)
        .map(|m| m.len() as f64 / 1_048_576.0)
        .unwrap_or(0.0)
}

fn probe_tesseract_known_path() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        for p in [
            r"C:\Program Files\Tesseract-OCR\tesseract.exe",
            r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe",
        ] {
            let q = std::path::Path::new(p);
            if q.is_file() {
                return Some(p.to_string());
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        for p in [
            "/usr/bin/tesseract",
            "/usr/local/bin/tesseract",
            "/opt/homebrew/bin/tesseract",
        ] {
            let q = std::path::Path::new(p);
            if q.is_file() {
                return Some(p.to_string());
            }
        }
    }
    None
}

/// Local DB sizes + managed server health + Tesseract install hint (no subprocess spawns).
#[tauri::command]
pub async fn get_health_snapshot(state: AppStateHandle<'_>) -> CmdResult<HealthSnapshot> {
    let shared: Arc<AppState> = state.inner().clone();
    let data_dir = shared.config().data_dir.clone();
    let main = data_dir.join(paths::SQLITE_DB_FILE);
    let wal = data_dir.join(format!("{}-wal", paths::SQLITE_DB_FILE));
    let shm = data_dir.join(format!("{}-shm", paths::SQLITE_DB_FILE));

    let mut managed = Vec::with_capacity(MANAGED_KINDS.len());
    for kind in MANAGED_KINDS {
        managed.push(managed_status_for_kind(&shared, kind).await);
    }

    Ok(HealthSnapshot {
        sqlite_main_mb: file_metadata_len_mb(&main),
        sqlite_wal_mb: file_metadata_len_mb(&wal),
        sqlite_shm_mb: file_metadata_len_mb(&shm),
        tesseract_known_path: probe_tesseract_known_path(),
        managed_servers: managed,
    })
}

fn trim_base_url(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

/// GET Ollama `/api/tags` — validates the Ollama URL used for chat/pull when backend is Ollama.
#[tauri::command]
pub async fn test_ollama_connection(ollama_url: String) -> CmdResult<LlmConnectionTest> {
    let base = trim_base_url(&ollama_url);
    if base.is_empty() {
        return Ok(LlmConnectionTest {
            ok: false,
            detail: "Ollama URL is empty".into(),
        });
    }
    let url = format!("{}/api/tags", base);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(err)?;
    let resp = client.get(&url).send().await.map_err(err)?;
    let st = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if st.is_success() {
        return Ok(LlmConnectionTest {
            ok: true,
            detail: format!("GET {} → {}", url, st),
        });
    }
    let snippet: String = body.chars().take(200).collect();
    Ok(LlmConnectionTest {
        ok: false,
        detail: format!("GET {} → {}: {}", url, st, snippet),
    })
}

/// GET OpenAI-compatible `/v1/models` (llama.cpp and most proxies expose this).
#[tauri::command]
pub async fn test_openai_chat_connection(
    base_url: String,
    api_key: String,
) -> CmdResult<LlmConnectionTest> {
    let base = trim_base_url(&base_url);
    if base.is_empty() {
        return Ok(LlmConnectionTest {
            ok: false,
            detail: "Base URL is empty".into(),
        });
    }
    let url = format!("{}/models", base);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(err)?;
    let mut req = client.get(&url);
    if !api_key.trim().is_empty() {
        req = req.bearer_auth(api_key.trim());
    }
    let resp = req.send().await.map_err(err)?;
    let st = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if st.is_success() {
        return Ok(LlmConnectionTest {
            ok: true,
            detail: format!("GET {} → {}", url, st),
        });
    }
    let snippet: String = body.chars().take(200).collect();
    Ok(LlmConnectionTest {
        ok: false,
        detail: format!("GET {} → {}: {}", url, st, snippet),
    })
}

/// POST OpenAI-compatible `/v1/embeddings` using the same shape as the indexer.
#[tauri::command]
pub async fn test_openai_embed_connection(
    base_url: String,
    api_key: String,
    model: String,
) -> CmdResult<LlmConnectionTest> {
    let base = trim_base_url(&base_url);
    if base.is_empty() {
        return Ok(LlmConnectionTest {
            ok: false,
            detail: "Base URL is empty (set the embeddings or chat base URL)".into(),
        });
    }
    if model.trim().is_empty() {
        return Ok(LlmConnectionTest {
            ok: false,
            detail: "Embedding model name is empty".into(),
        });
    }
    let url = format!("{}/embeddings", base);
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(err)?;
    let body = serde_json::json!({
        "model": model.trim(),
        "input": "screenrecall connection test",
    });
    let mut req = client.post(&url).json(&body);
    if !api_key.trim().is_empty() {
        req = req.bearer_auth(api_key.trim());
    }
    let resp = req.send().await.map_err(err)?;
    let st = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    if !st.is_success() {
        let snippet: String = body_text.chars().take(200).collect();
        return Ok(LlmConnectionTest {
            ok: false,
            detail: format!("POST {} → {}: {}", url, st, snippet),
        });
    }
    let dim = serde_json::from_str::<serde_json::Value>(&body_text)
        .ok()
        .as_ref()
        .and_then(|v| v.pointer("/data/0/embedding")?.as_array())
        .map(|a| a.len());
    Ok(LlmConnectionTest {
        ok: true,
        detail: match dim {
            Some(n) if n > 0 => format!("POST {} → 200, embedding dim {}", url, n),
            _ => format!(
                "POST {} → 200 (body did not look like OpenAI embeddings JSON)",
                url
            ),
        },
    })
}

/// Persisted chat UI (threads, session id) under the configured data directory.
#[tauri::command]
pub fn load_chat_ui_state(state: AppStateHandle<'_>) -> CmdResult<Option<String>> {
    let path = state.config().data_dir.join("chat_ui.json");
    match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => Ok(Some(s)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
pub fn save_chat_ui_state(state: AppStateHandle<'_>, json: String) -> CmdResult<()> {
    let path = state.config().data_dir.join("chat_ui.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    std::fs::write(&path, json).map_err(err)
}

#[tauri::command]
pub async fn check_dependencies(state: AppStateHandle<'_>) -> CmdResult<DependencyReport> {
    let cfg = state.config();
    let shared: Arc<AppState> = state.inner().clone();
    let mut items = Vec::new();

    let tesseract = tauri::async_runtime::spawn_blocking(resolve_tesseract_for_check)
        .await
        .map_err(err)?;
    match tesseract {
        Some(bin) => {
            process_log::record(
                &shared,
                "check_dependencies",
                &bin.to_string_lossy(),
                &["--version".to_string()],
                None,
            );
            let out = hidden_command(&bin)
                .arg("--version")
                .output()
                .map_err(err)?;
            items.push(DependencyStatus {
                key: "tesseract".into(),
                label: "Tesseract".into(),
                status: "ok".into(),
                detail: String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .map(|l| format!("{} ({})", l, bin.display()))
                    .unwrap_or_else(|| format!("tesseract found at {}", bin.display())),
            });
        }
        None => items.push(DependencyStatus {
            key: "tesseract".into(),
            label: "Tesseract".into(),
            status: "missing".into(),
            detail: "program not found".into(),
        }),
    }

    let http = Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(err)?;

    match cfg.llm_backend {
        LlmBackend::Openai => {
            let chat_base = cfg.openai_base_url.trim_end_matches('/');
            let detail = match &cfg.openai_embedding_base_url {
                Some(emb) if !emb.trim().is_empty() => format!(
                    "OpenAI-compatible — chat {}, embeddings {} (Ollama not required for chat)",
                    chat_base,
                    emb.trim().trim_end_matches('/')
                ),
                _ => format!(
                    "OpenAI-compatible — chat/embed use {} (separate embed URL in Settings, or Ollama fallback for embeds)",
                    chat_base
                ),
            };
            items.push(DependencyStatus {
                key: "llm_backend".into(),
                label: "LLM backend".into(),
                status: "ok".into(),
                detail,
            });
        }
        LlmBackend::Ollama => {
            let tags_url = format!("{}/api/tags", cfg.ollama_url.trim_end_matches('/'));
            let tags_resp = http.get(&tags_url).send().await;
            match tags_resp {
                Ok(resp) if resp.status().is_success() => {
                    #[derive(Deserialize)]
                    struct OllamaModel {
                        name: String,
                    }
                    #[derive(Deserialize)]
                    struct OllamaTags {
                        models: Vec<OllamaModel>,
                    }

                    let tags: OllamaTags = resp.json().await.map_err(err)?;
                    let names: std::collections::HashSet<String> = tags
                        .models
                        .into_iter()
                        .map(|m| m.name.split(':').next().unwrap_or(&m.name).to_string())
                        .collect();

                    items.push(DependencyStatus {
                        key: "ollama".into(),
                        label: "Ollama endpoint".into(),
                        status: "ok".into(),
                        detail: format!("reachable at {}", cfg.ollama_url),
                    });

                    // Informational only: wrong names do not block first-run / setup gate.
                    for (key, label, model) in [
                        ("chat_model", "Chat model (Ollama)", cfg.chat_model.clone()),
                        ("embed_model", "Embedding model (Ollama)", cfg.embed_model.clone()),
                    ] {
                        let base = model.split(':').next().unwrap_or(&model).to_string();
                        let present =
                            names.contains(&model) || names.contains(&base);
                        items.push(DependencyStatus {
                            key: key.into(),
                            label: label.into(),
                            status: if present { "ok" } else { "optional" }.into(),
                            detail: if present {
                                format!("{} is pulled in Ollama", model)
                            } else {
                                format!(
                                    "Not found as \"{}\" — pull it or change model name in Settings (e.g. llama3.3)",
                                    model
                                )
                            },
                        });
                    }
                }
                Ok(resp) => {
                    items.push(DependencyStatus {
                        key: "ollama".into(),
                        label: "Ollama endpoint".into(),
                        status: "missing".into(),
                        detail: format!("{} returned {}", cfg.ollama_url, resp.status()),
                    });
                }
                Err(e) => {
                    items.push(DependencyStatus {
                        key: "ollama".into(),
                        label: "Ollama endpoint".into(),
                        status: "missing".into(),
                        detail: e.to_string(),
                    });
                }
            }
        }
    }

    /// Only these rows block setup completion (first-run gate).
    fn blocks_setup(item: &DependencyStatus) -> bool {
        match item.key.as_str() {
            // Chat/embed model names are advisory when using Ollama; user may use llama.cpp, remote API, etc.
            "chat_model" | "embed_model" => false,
            _ => item.status != "ok",
        }
    }

    let ok = items.iter().all(|i| !blocks_setup(i));
    Ok(DependencyReport { ok, items })
}

#[tauri::command]
pub async fn pull_model(
    app: tauri::AppHandle,
    state: AppStateHandle<'_>,
    model: String,
) -> CmdResult<()> {
    let cfg = state.config();
    let url = format!("{}/api/pull", cfg.ollama_url.trim_end_matches('/'));
    let http = Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(err)?;

    let resp = http
        .post(url)
        .json(&serde_json::json!({
            "name": model,
            "stream": true
        }))
        .send()
        .await
        .map_err(err)?;

    if !resp.status().is_success() {
        return Err(format!("pull failed: {}", resp.status()));
    }

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(err)?;
        for line in String::from_utf8_lossy(&chunk).lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                let status = v
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pulling")
                    .to_string();
                let completed = v.get("completed").and_then(|n| n.as_u64());
                let total = v.get("total").and_then(|n| n.as_u64());
                let progress = match (completed, total) {
                    (Some(c), Some(t)) if t > 0 => Some((c as f64 / t as f64).min(1.0)),
                    _ => None,
                };
                let _ = app.emit(
                    "setup:pull-progress",
                    serde_json::json!({
                        "model": v.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                        "status": status,
                        "progress": progress,
                    }),
                );
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn install_tesseract(state: AppStateHandle<'_>) -> CmdResult<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        process_log::record(
            state.inner(),
            "install_tesseract",
            "winget",
            &[
                "install".into(),
                "--id".into(),
                "UB-Mannheim.TesseractOCR".into(),
                "-e".into(),
                "--accept-package-agreements".into(),
                "--accept-source-agreements".into(),
            ],
            None,
        );
        let mut c = hidden_command("winget");
        c.args([
            "install",
            "--id",
            "UB-Mannheim.TesseractOCR",
            "-e",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = hidden_command("brew");
        c.args(["install", "tesseract"]);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = hidden_command("pkexec");
        c.args(["apt-get", "install", "-y", "tesseract-ocr"]);
        c
    };
    let status = cmd.status().map_err(err)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("installer exited with {}", status))
    }
}

#[tauri::command]
pub fn install_ollama(state: AppStateHandle<'_>) -> CmdResult<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        process_log::record(
            state.inner(),
            "install_ollama",
            "winget",
            &[
                "install".into(),
                "--id".into(),
                "Ollama.Ollama".into(),
                "-e".into(),
                "--accept-package-agreements".into(),
                "--accept-source-agreements".into(),
            ],
            None,
        );
        let mut c = hidden_command("winget");
        c.args([
            "install",
            "--id",
            "Ollama.Ollama",
            "-e",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = hidden_command("brew");
        c.args(["install", "--cask", "ollama"]);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = hidden_command("sh");
        c.args(["-c", "curl -fsSL https://ollama.com/install.sh | sh"]);
        c
    };
    let status = cmd.status().map_err(err)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("installer exited with {}", status))
    }
}

#[tauri::command]
pub fn complete_setup(state: AppStateHandle<'_>) -> CmdResult<()> {
    let mut cfg = state.config();
    cfg.setup_complete = true;
    state.set_config(cfg).map_err(err)
}

#[derive(Serialize)]
pub struct Stats {
    pub frame_count: i64,
    pub disk_mb: f64,
    pub indexed_count: i64,
}

#[tauri::command]
pub fn get_stats(state: AppStateHandle<'_>) -> CmdResult<Stats> {
    let (frames, indexed) = state.cached_counts();
    let disk_mb = state.cached_disk_mb();
    Ok(Stats {
        frame_count: frames,
        disk_mb,
        indexed_count: indexed,
    })
}

/// Live indexer status: DB backlog + current frame (in-memory) for OCR and embed workers.
#[tauri::command]
pub fn get_worker_queue_snapshot(state: AppStateHandle<'_>) -> CmdResult<WorkerQueuesSnapshot> {
    let ocr_p = state.store.count_pending_ocr().map_err(err)?;
    let emb_p = state.store.count_pending_embed().map_err(err)?;
    let (o_id, o_ms) = state.worker_activity.snapshot_ocr();
    let (e_id, e_ms) = state.worker_activity.snapshot_embed();
    let (ocr_timing, embed_timing) = state.worker_activity.timing_stats();
    let (frame_total, _indexed) = state.store.stats().map_err(err)?;
    Ok(WorkerQueuesSnapshot {
        ocr: OneWorkerQueue {
            pending_in_db: ocr_p,
            active_frame_id: o_id,
            active_elapsed_ms: o_ms,
        },
        embed: OneWorkerQueue {
            pending_in_db: emb_p,
            active_frame_id: e_id,
            active_elapsed_ms: e_ms,
        },
        ocr_timing,
        embed_timing,
        frame_total,
    })
}

/* ------------------------------------------------------------------ */
/* Frames / search                                                    */
/* ------------------------------------------------------------------ */

#[tauri::command]
pub fn list_frames(
    state: AppStateHandle<'_>,
    limit: i64,
    before_ts: Option<i64>,
) -> CmdResult<Vec<Frame>> {
    state.store.list_frames(limit, before_ts).map_err(err)
}

#[tauri::command]
pub fn get_frame_ocr(
    state: AppStateHandle<'_>,
    frame_id: i64,
) -> CmdResult<Option<String>> {
    state.store.get_ocr_text(frame_id).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingPreview {
    pub dim: usize,
    pub values: Vec<f32>,
}

/// Inspect one frame embedding (preview only; values are capped for UI readability).
#[tauri::command]
pub fn get_frame_embedding_preview(
    state: AppStateHandle<'_>,
    frame_id: i64,
) -> CmdResult<Option<EmbeddingPreview>> {
    let preview = state
        .store
        .get_embedding_preview(frame_id, 128)
        .map_err(err)?;
    Ok(preview.map(|(dim, values)| EmbeddingPreview { dim, values }))
}

/// Reset OCR (and related embed state) for frames that are still pending OCR or have no stored
/// text, then queue them for the OCR worker. Returns how many frames were reset.
#[tauri::command]
pub fn requeue_ocr_rerun(
    state: AppStateHandle<'_>,
    ocr: State<'_, OcrQueue>,
) -> CmdResult<usize> {
    let ids = state.store.requeue_ocr_rerun().map_err(err)?;
    let n = ids.len();
    for id in ids {
        let _ = ocr.0.send(id);
    }
    Ok(n)
}

/// Reset embed state for frames that have OCR text but no vector, so embed worker retries.
#[tauri::command]
pub fn requeue_embed_rerun(state: AppStateHandle<'_>) -> CmdResult<usize> {
    let ids = state.store.requeue_embed_rerun().map_err(err)?;
    Ok(ids.len())
}

#[tauri::command]
pub async fn search(
    state: AppStateHandle<'_>,
    query: String,
    limit: usize,
    semantic: bool,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
) -> CmdResult<Vec<search::SearchHit>> {
    let _ = (start_ts, end_ts); // reserved for date filtering in a follow-up
    let http = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(err)?;
    let state_arc: Arc<AppState> = state.inner().clone();
    search::search(&state_arc, &http, &query, limit, semantic)
        .await
        .map_err(err)
}

/* ------------------------------------------------------------------ */
/* Chat (RAG, streaming over events)                                  */
/* ------------------------------------------------------------------ */

#[tauri::command]
pub async fn chat(
    app: tauri::AppHandle,
    state: AppStateHandle<'_>,
    prompt: String,
    session_id: String,
    k: usize,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
) -> CmdResult<()> {
    let _ = (start_ts, end_ts);
    let state: Arc<AppState> = state.inner().clone();
    let session = session_id.clone();

    tauri::async_runtime::spawn(async move {
        if let Err(err) = run_chat(app.clone(), state, &session, prompt, k).await {
            let _ = app.emit(
                "chat:error",
                serde_json::json!({
                    "session_id": session,
                    "error": err.to_string(),
                }),
            );
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn chat_cancel(state: AppStateHandle<'_>, session_id: String) -> CmdResult<()> {
    let mut set = state.cancelled_sessions.lock().await;
    set.insert(session_id);
    Ok(())
}

async fn run_chat(
    app: tauri::AppHandle,
    state: Arc<AppState>,
    session_id: &str,
    prompt: String,
    k: usize,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let http = Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    // 1. Retrieve top-k relevant frames via hybrid search.
    let hits = search::search(&state, &http, &prompt, k.max(1), true).await?;
    let mut ctx: Vec<(Frame, Option<String>)> = Vec::new();
    for h in &hits {
        let text = state.store.get_ocr_text(h.frame.id).ok().flatten();
        ctx.push((h.frame.clone(), text));
    }
    perf_log::record(
        &state,
        "chat_context_built",
        serde_json::json!({
            "session_id": session_id,
            "k": k.max(1),
            "hits": hits.len(),
            "prompt_len": prompt.len(),
            "ctx_chars": ctx.iter().map(|(_, t)| t.as_ref().map(|x| x.len()).unwrap_or(0)).sum::<usize>(),
            "ms": started.elapsed().as_millis() as u64,
        }),
    );

    // Tell the UI which frames we're citing.
    let citations: Vec<Frame> = hits.iter().map(|h| h.frame.clone()).collect();
    let _ = app.emit(
        "chat:citations",
        serde_json::json!({
            "session_id": session_id,
            "frames": citations,
        }),
    );

    let cfg = state.config();

    // 2. Build the chat messages.
    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: prompts::effective_system_prompt(&cfg),
        },
        ChatMessage {
            role: "user".into(),
            content: prompts::build_user_message(&prompt, &ctx),
        },
    ];

    // 3. Stream tokens. We channel deltas to a forwarder that also watches
    //    for cancellation.
    let client = llm::build_client(&cfg, &http);

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let cancel = Arc::new(Notify::new());

    // Background task: poll for user cancellation and trigger the notify.
    {
        let state = state.clone();
        let cancel = cancel.clone();
        let session_id = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let cancelled = {
                    let set = state.cancelled_sessions.lock().await;
                    set.contains(&session_id)
                };
                if cancelled {
                    cancel.notify_waiters();
                    break;
                }
            }
        });
    }

    // Forwarder: relay chunks to the webview.
    {
        let app = app.clone();
        let session_id = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            while let Some(delta) = rx.recv().await {
                let _ = app.emit(
                    "chat:delta",
                    serde_json::json!({
                        "session_id": session_id,
                        "delta": delta,
                    }),
                );
            }
        });
    }

    const CHAT_STREAM_TIMEOUT_SECS: u64 = 300;
    match timeout(
        std::time::Duration::from_secs(CHAT_STREAM_TIMEOUT_SECS),
        client.chat_stream(&messages, &tx, &cancel),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            drop(tx);
            perf_log::record(
                &state,
                "chat_error",
                serde_json::json!({
                    "session_id": session_id,
                    "ms": started.elapsed().as_millis() as u64,
                    "error": e.to_string(),
                }),
            );
            return Err(e);
        }
        Err(_) => {
            cancel.notify_waiters();
            drop(tx);
            perf_log::record(
                &state,
                "chat_timeout",
                serde_json::json!({
                    "session_id": session_id,
                    "ms": started.elapsed().as_millis() as u64,
                    "timeout_secs": CHAT_STREAM_TIMEOUT_SECS,
                }),
            );
            return Err(anyhow::anyhow!(
                "Chat timed out after {}s (no complete response from the model)",
                CHAT_STREAM_TIMEOUT_SECS
            ));
        }
    }

    // 4. Close out.
    drop(tx);
    let _ = app.emit(
        "chat:done",
        serde_json::json!({ "session_id": session_id }),
    );
    // Clear cancellation so a future reuse of the same session_id works.
    state.cancelled_sessions.lock().await.remove(session_id);
    perf_log::record(
        &state,
        "chat_ok",
        serde_json::json!({
            "session_id": session_id,
            "ms": started.elapsed().as_millis() as u64,
            "hits": hits.len(),
        }),
    );
    Ok(())
}

/* ------------------------------------------------------------------ */
/* Admin                                                              */
/* ------------------------------------------------------------------ */

#[tauri::command]
pub fn open_data_dir(state: AppStateHandle<'_>) -> CmdResult<()> {
    let dir = state.config().data_dir;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(err)?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(err)?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(err)?;
    }
    Ok(())
}

/// Open the system file manager with this file selected (Windows Explorer, Finder, …).
#[tauri::command]
pub fn reveal_frame_in_folder(path: String) -> CmdResult<()> {
    let p = std::path::Path::new(&path);
    reveal_in_folder::reveal_file(p).map_err(err)
}

#[tauri::command]
pub fn delete_all(state: AppStateHandle<'_>) -> CmdResult<()> {
    // Pause recording while we wipe.
    state.set_recording(false);

    state.store.delete_all().map_err(err)?;

    // Best-effort: remove the frames directory.
    let dir = state.frames_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);

    state.set_recording(true);
    Ok(())
}

#[tauri::command]
pub fn window_minimize_to_tray(state: AppStateHandle<'_>) -> CmdResult<()> {
    if let Some(win) = state.app.get_webview_window("main") {
        win.hide().map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub fn window_quit_app(state: AppStateHandle<'_>) -> CmdResult<()> {
    // Ensure managed local servers don't outlive the app process.
    let _ = state.stop_all_managed_llama_blocking();
    if let Some(win) = state.app.get_webview_window("main") {
        win.close().map_err(err)?;
    } else {
        state.app.exit(0);
    }
    Ok(())
}

#[tauri::command]
pub fn get_perf_log_tail(state: AppStateHandle<'_>, limit: Option<usize>) -> CmdResult<Vec<String>> {
    let max = limit.unwrap_or(300).clamp(1, 5000);
    let path = state.config().data_dir.join("perf-events.jsonl");
    match read_tail_lines(&path, max, 384 * 1024) {
        Ok(lines) => Ok(lines),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
pub fn get_perf_log_path(state: AppStateHandle<'_>) -> CmdResult<String> {
    Ok(state
        .config()
        .data_dir
        .join("perf-events.jsonl")
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub fn get_runtime_log_tail(state: AppStateHandle<'_>, limit: Option<usize>) -> CmdResult<Vec<String>> {
    let max = limit.unwrap_or(400).clamp(1, 5000);
    let path = state.config().data_dir.join("runtime.log");
    match read_tail_lines(&path, max, 384 * 1024) {
        Ok(lines) => Ok(lines),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
pub fn get_process_log_tail(state: AppStateHandle<'_>, limit: Option<usize>) -> CmdResult<Vec<String>> {
    let max = limit.unwrap_or(400).clamp(1, 5000);
    let path = state.config().data_dir.join("process-events.jsonl");
    match read_tail_lines(&path, max, 512 * 1024) {
        Ok(lines) => Ok(lines),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
pub fn get_process_log_path(state: AppStateHandle<'_>) -> CmdResult<String> {
    Ok(state
        .config()
        .data_dir
        .join("process-events.jsonl")
        .to_string_lossy()
        .to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOnStartupStatus {
    pub enabled: bool,
    pub detail: String,
}
const RUN_KEY: &str = "ScreenRecall";

#[tauri::command]
pub fn get_launch_on_startup_status(state: AppStateHandle<'_>) -> CmdResult<LaunchOnStartupStatus> {
    #[cfg(target_os = "windows")]
    {
        process_log::record(
            state.inner(),
            "launch_on_startup_status",
            "reg",
            &[
                "query".into(),
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".into(),
                "/v".into(),
                RUN_KEY.into(),
            ],
            None,
        );
        let out = hidden_command("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                RUN_KEY,
            ])
            .output()
            .map_err(err)?;
        let enabled = out.status.success();
        let detail = String::from_utf8_lossy(&out.stdout).to_string();
        return Ok(LaunchOnStartupStatus { enabled, detail });
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(LaunchOnStartupStatus {
            enabled: false,
            detail: "Launch on startup toggle is currently implemented for Windows only".into(),
        })
    }
}

#[tauri::command]
pub fn set_launch_on_startup(state: AppStateHandle<'_>, enabled: bool) -> CmdResult<LaunchOnStartupStatus> {
    #[cfg(target_os = "windows")]
    {
        if enabled {
            let exe = std::env::current_exe().map_err(err)?;
            let exe_s = exe.to_string_lossy().to_string();
            let value = format!("\"{}\"", exe_s);
            process_log::record(
                state.inner(),
                "set_launch_on_startup",
                "reg",
                &[
                    "add".into(),
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".into(),
                    "/v".into(),
                    RUN_KEY.into(),
                    "/t".into(),
                    "REG_SZ".into(),
                    "/d".into(),
                    value.clone(),
                    "/f".into(),
                ],
                None,
            );
            let out = hidden_command("reg")
                .args([
                    "add",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                    "/v",
                    RUN_KEY,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &value,
                    "/f",
                ])
                .output()
                .map_err(err)?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).to_string());
            }
            return Ok(LaunchOnStartupStatus {
                enabled: true,
                detail: format!("Enabled from {}", exe_s),
            });
        }
        process_log::record(
            state.inner(),
            "set_launch_on_startup",
            "reg",
            &[
                "delete".into(),
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run".into(),
                "/v".into(),
                RUN_KEY.into(),
                "/f".into(),
            ],
            None,
        );
        let out = hidden_command("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                RUN_KEY,
                "/f",
            ])
            .output()
            .map_err(err)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            // Deleting non-existent value is fine.
            if !stderr.to_ascii_lowercase().contains("unable to find")
                && !stdout.to_ascii_lowercase().contains("unable to find")
            {
                return Err(if stderr.trim().is_empty() { stdout } else { stderr });
            }
        }
        return Ok(LaunchOnStartupStatus {
            enabled: false,
            detail: "Disabled".into(),
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(LaunchOnStartupStatus {
            enabled: false,
            detail: "Launch on startup toggle is currently implemented for Windows only".into(),
        })
    }
}
