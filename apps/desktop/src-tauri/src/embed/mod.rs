//! Text embedding pipeline.
//!
//! Uses whichever backend is configured in the app settings. For now that's
//! either Ollama's `/api/embeddings` or an OpenAI-compatible `/v1/embeddings`.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{self, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::config::{AppConfig, LlmBackend};
use crate::state::SharedState;
use crate::util::perf_log;
use crate::worker_activity::EmbedActiveGuard;

/// OpenAI→Ollama fallback logs at most once per distinct server URL + model, until
/// `reset_ollama_fallback_log_signature` (e.g. after saving Settings) clears the signature.
static OLLAMA_FALLBACK_LOG_SIG: Mutex<String> = Mutex::new(String::new());

/// Call when embed-related config may have changed so the next fallback logs again.
pub fn reset_ollama_fallback_log_signature() {
    if let Ok(mut g) = OLLAMA_FALLBACK_LOG_SIG.lock() {
        g.clear();
    }
}

fn log_ollama_fallback_once(openai_base: &str, ollama_url: &str, embed_model: &str) {
    let sig = format!("{openai_base}|{ollama_url}|{embed_model}");
    let mut g = match OLLAMA_FALLBACK_LOG_SIG.lock() {
        Ok(x) => x,
        Err(_) => return,
    };
    if g.as_str() == sig.as_str() {
        return;
    }
    g.clear();
    g.push_str(&sig);
    info!(
        "OpenAI /v1/embeddings unavailable; using Ollama at {} (model {})",
        ollama_url, embed_model
    );
}

#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

pub struct OllamaEmbedder {
    pub base_url: String,
    pub model: String,
    pub http: Client,
}

#[async_trait::async_trait]
impl Embedder for OllamaEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            prompt: &'a str,
        }
        #[derive(Deserialize)]
        struct Res {
            embedding: Vec<f32>,
        }

        let url = format!("{}/api/embeddings", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(url)
            .json(&Req {
                model: &self.model,
                prompt: text,
            })
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            let snip: String = body.chars().take(768).collect();
            return Err(anyhow!("Ollama /api/embeddings HTTP {}: {}", status, snip));
        }
        let res: Res = serde_json::from_str(&body)
            .map_err(|e| anyhow!("Ollama embeddings parse: {e} body: {} ", body.chars().take(200).collect::<String>()))?;
        if res.embedding.is_empty() {
            return Err(anyhow!("ollama returned empty embedding"));
        }
        Ok(res.embedding)
    }
}

pub struct OpenAiEmbedder {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub http: Client,
}

#[async_trait::async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            input: &'a str,
        }
        #[derive(Deserialize)]
        struct Item {
            embedding: Vec<f32>,
        }
        #[derive(Deserialize)]
        struct Res {
            data: Vec<Item>,
        }

        // User configures the full base url, e.g. "https://api.openai.com/v1".
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let mut req = self.http.post(&url).json(&Req {
            model: &self.model,
            input: text,
        });
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            let snip: String = body.chars().take(768).collect();
            return Err(anyhow!("HTTP {status} for {url}: {snip}"));
        }
        let res: Res = serde_json::from_str(&body)
            .map_err(|e| anyhow!("OpenAI /v1/embeddings parse: {e} body: {} ", body.chars().take(200).collect::<String>()))?;
        res.data
            .into_iter()
            .next()
            .map(|i| i.embedding)
            .ok_or_else(|| anyhow!("openai returned empty embedding"))
    }
}

/// When the primary OpenAI-compatible server has no `/v1/embeddings` (HTTP 501), or you only set
/// `openai_base_url` for chat: fall back to Ollama's `/api/embeddings`. Prefer
/// `openai_embedding_base_url` in Settings to point at a second `llama-server` for embeddings
/// (GPU) instead of this fallback.
pub struct OpenAiWithOllamaFallback {
    openai: OpenAiEmbedder,
    ollama: OllamaEmbedder,
}

#[async_trait::async_trait]
impl Embedder for OpenAiWithOllamaFallback {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        match self.openai.embed(text).await {
            Ok(v) => Ok(v),
            Err(e) => {
                let msg = e.to_string();
                // llama-server and many local servers omit the embeddings route.
                if msg.contains("501")
                    || msg.contains("404")
                    || msg.contains("405")
                    || msg.contains("Not Implemented")
                {
                    log_ollama_fallback_once(
                        &self.openai.base_url,
                        &self.ollama.base_url,
                        &self.openai.model,
                    );
                    self.ollama.embed(text).await
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// `Some(base)` → use `POST {base}/embeddings` (OpenAI-compatible) with optional Ollama fallback
/// to `ollama_url` when the server returns 404/501/405.
/// `None` → Ollama native `/api/embeddings` on `ollama_url` (only when no dedicated embed base is
/// configured *and* backend is Ollama; OpenAI backend without embed override uses `openai_base_url`
/// and always becomes `Some`).
fn openai_style_embed_base(cfg: &AppConfig) -> Option<String> {
    if let Some(ref s) = cfg.openai_embedding_base_url {
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if matches!(cfg.llm_backend, LlmBackend::Openai) {
        let t = cfg.openai_base_url.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    } else {
        None
    }
}

pub fn build_embedder(cfg: &AppConfig, http: &Client) -> Box<dyn Embedder> {
    if let Some(base) = openai_style_embed_base(cfg) {
        return Box::new(OpenAiWithOllamaFallback {
            openai: OpenAiEmbedder {
                base_url: base,
                api_key: cfg.openai_api_key.clone(),
                model: cfg.embed_model.clone(),
                http: http.clone(),
            },
            ollama: OllamaEmbedder {
                base_url: cfg.ollama_url.clone(),
                model: cfg.embed_model.clone(),
                http: http.clone(),
            },
        });
    }
    Box::new(OllamaEmbedder {
        base_url: cfg.ollama_url.clone(),
        model: cfg.embed_model.clone(),
        http: http.clone(),
    })
}

pub async fn run_worker(
    state: SharedState,
    mut rx: UnboundedReceiver<i64>,
) -> Result<()> {
    info!("embedding worker started");
    let http = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    flush_pending(&state, &http).await;

    // Do not use `select!` with a sleep that is recreated each turn (it never fires while
    // recv keeps unblocking) or with `biased` that prefers recv — both starve the timer so the
    // DB backlog is never drained under steady capture. A real `Interval` still advances while we
    // process frames, and (without `biased`) tokio can schedule the tick when both are ready.
    let mut flush_tick = time::interval_at(
        time::Instant::now() + Duration::from_secs(5),
        Duration::from_secs(5),
    );
    flush_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = flush_tick.tick() => {
                flush_pending(&state, &http).await;
            }
            id = rx.recv() => {
                match id {
                    Some(id) => process_one(&state, &http, id).await,
                    None => {
                        info!("embed channel closed; draining pending then exiting");
                        flush_pending(&state, &http).await;
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

const EMBED_FLUSH_BATCH: i64 = 256;
const EMBED_FLUSH_MAX_ROUNDS: usize = 500;
const EMBED_TEXT_RETRY_ATTEMPTS: usize = 4;
const EMBED_TEXT_SHRINK_NUM: usize = 3;
const EMBED_TEXT_SHRINK_DEN: usize = 4;
const EMBED_TEXT_MIN_CHARS: usize = 120;

fn looks_like_input_too_large_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("too large to process")
        || m.contains("input is too large")
        || m.contains("context length")
        || m.contains("maximum context length")
        || m.contains("increase the physical batch size")
        || m.contains("n_ctx")
}

fn shrink_text_for_retry(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if len <= EMBED_TEXT_MIN_CHARS {
        return None;
    }
    let mut new_len = len.saturating_mul(EMBED_TEXT_SHRINK_NUM) / EMBED_TEXT_SHRINK_DEN;
    if new_len >= len {
        new_len = len.saturating_sub(1);
    }
    if new_len < EMBED_TEXT_MIN_CHARS {
        new_len = EMBED_TEXT_MIN_CHARS.min(len.saturating_sub(1));
    }
    if new_len == 0 || new_len >= len {
        return None;
    }
    Some(chars[..new_len].iter().collect())
}

/// Drain the embed backlog in a few large batches. The old code took at most 16 every ~30s and
/// could be starved entirely when new frame ids never stopped (timer arm never won).
async fn flush_pending(state: &SharedState, http: &Client) {
    for _ in 0..EMBED_FLUSH_MAX_ROUNDS {
        let ids = match state.store.pending_embed(EMBED_FLUSH_BATCH) {
            Ok(ids) => ids,
            Err(err) => {
                warn!(?err, "pending_embed query failed");
                return;
            }
        };
        if ids.is_empty() {
            return;
        }
        for id in ids {
            process_one(state, http, id).await;
        }
    }
    warn!(
        "flush_pending: stopped after {}×{} frames in one run; will continue on the next tick",
        EMBED_FLUSH_MAX_ROUNDS,
        EMBED_FLUSH_BATCH
    );
}

async fn process_one(state: &SharedState, http: &Client, id: i64) {
    let _active = EmbedActiveGuard::new(&state.worker_activity, id);
    let started = std::time::Instant::now();
    let text = match state.store.get_ocr_text(id) {
        Ok(Some(t)) => t,
        Ok(None) => {
            // Embed worker receives fresh frame ids before OCR completes; that's expected.
            // Only warn when OCR is already marked done but the OCR row is still missing.
            let ocr_done = state
                .store
                .get_frame(id)
                .ok()
                .flatten()
                .map(|f| f.ocr_done)
                .unwrap_or(false);
            if ocr_done {
                warn!(
                    frame_id = id,
                    "get_ocr_text returned None even though ocr_done=1 — deferring"
                );
            } else {
                debug!(
                    frame_id = id,
                    "embedding deferred: OCR not finished yet for this frame"
                );
            }
            perf_log::record(
                state,
                "embed_waiting_for_ocr",
                serde_json::json!({ "frame_id": id }),
            );
            return;
        }
        Err(err) => {
            warn!(frame_id = id, ?err, "get_ocr_text failed");
            return;
        }
    };

    // Nothing to embed if OCR found no text — mark the embed pipeline done so the UI
    // and `pending_embed` do not show an endless "in progress" state.
    if text.trim().is_empty() {
        if let Err(err) = state.store.set_embed_done_skipped(id) {
            warn!(frame_id = id, ?err, "set_embed_done_skipped failed");
        }
        let ms = started.elapsed().as_millis() as u64;
        perf_log::record(
            state,
            "embed_skipped_empty_text",
            serde_json::json!({
                "frame_id": id,
                "ms": ms,
            }),
        );
        state.worker_activity.record_embed_ms(ms);
        return;
    }

    let cfg = state.config();
    let embedder = build_embedder(&cfg, http);
    let mut embed_text = text.clone();
    let mut shrinks = 0usize;

    loop {
        match embedder.embed(&embed_text).await {
        Ok(vec) => {
            if let Err(err) = state.store.set_embedding(id, &vec) {
                warn!(frame_id = id, ?err, "save embedding failed");
                let ms = started.elapsed().as_millis() as u64;
                perf_log::record(
                    state,
                    "embed_error",
                    serde_json::json!({
                        "frame_id": id,
                        "ms": ms,
                        "error": err.to_string(),
                    }),
                );
                state.worker_activity.record_embed_ms(ms);
            } else {
                debug!(frame_id = id, dim = vec.len(), "embedded");
                let ms = started.elapsed().as_millis() as u64;
                perf_log::record(
                    state,
                    "embed_ok",
                    serde_json::json!({
                        "frame_id": id,
                        "dim": vec.len(),
                        "chars": text.len(),
                        "request_chars": embed_text.len(),
                        "input_shrinks": shrinks,
                        "ms": ms,
                    }),
                );
                state.worker_activity.record_embed_ms(ms);
            }
            break;
        }
        Err(err) => {
            let err_s = err.to_string();
            if looks_like_input_too_large_error(&err_s) && shrinks < EMBED_TEXT_RETRY_ATTEMPTS {
                if let Some(next) = shrink_text_for_retry(&embed_text) {
                    let before = embed_text.chars().count();
                    let after = next.chars().count();
                    shrinks += 1;
                    warn!(
                        frame_id = id,
                        attempt = shrinks,
                        before_chars = before,
                        after_chars = after,
                        "embedding input too large; retrying with shorter text"
                    );
                    embed_text = next;
                    continue;
                }
            }
            // Don't mark embed_done so we'll retry later (e.g. if Ollama is down).
            let ms = started.elapsed().as_millis() as u64;
            warn!(frame_id = id, ?err, "embedding call failed");
            perf_log::record(
                state,
                "embed_error",
                serde_json::json!({
                    "frame_id": id,
                    "ms": ms,
                    "error": err_s,
                    "request_chars": embed_text.len(),
                    "input_shrinks": shrinks,
                }),
            );
            state.worker_activity.record_embed_ms(ms);
            break;
        }
        }
    }
}
