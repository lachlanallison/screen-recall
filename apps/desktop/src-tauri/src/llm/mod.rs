//! Chat-completion clients.
//!
//! Both backends stream tokens back through a channel. The caller typically
//! forwards each delta into a Tauri event so the webview can render as
//! tokens arrive.

pub mod prompts;

use anyhow::Result;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{AppConfig, LlmBackend};

/// Guard against malformed streams that never delimit SSE/JSON lines — prevents unbounded growth.
const MAX_STREAM_SCRATCH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant"
    pub content: String,
}

#[async_trait::async_trait]
pub trait ChatClient: Send + Sync {
    /// Stream a chat completion. Each token is sent on `tx`. The function
    /// returns when streaming completes or errors.
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tx: &UnboundedSender<String>,
        cancel: &tokio::sync::Notify,
    ) -> Result<()>;
}

pub fn build_client(cfg: &AppConfig, http: &Client) -> Box<dyn ChatClient> {
    match cfg.llm_backend {
        LlmBackend::Ollama => Box::new(OllamaChat {
            base_url: cfg.ollama_url.clone(),
            model: cfg.chat_model.clone(),
            http: http.clone(),
        }),
        LlmBackend::Openai => Box::new(OpenAiChat {
            base_url: cfg.openai_base_url.clone(),
            api_key: cfg.openai_api_key.clone(),
            model: cfg.chat_model.clone(),
            http: http.clone(),
        }),
    }
}

/* ------------------------------------------------------------------ */
/* Ollama                                                             */
/* ------------------------------------------------------------------ */

pub struct OllamaChat {
    pub base_url: String,
    pub model: String,
    pub http: Client,
}

#[async_trait::async_trait]
impl ChatClient for OllamaChat {
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tx: &UnboundedSender<String>,
        cancel: &tokio::sync::Notify,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            messages: &'a [ChatMessage],
            stream: bool,
        }
        #[derive(Deserialize)]
        struct Chunk {
            #[serde(default)]
            message: Option<ChunkMsg>,
            #[serde(default)]
            done: bool,
        }
        #[derive(Deserialize)]
        struct ChunkMsg {
            #[serde(default)]
            content: String,
        }

        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .post(url)
            .json(&Req {
                model: &self.model,
                messages,
                stream: true,
            })
            .send()
            .await?
            .error_for_status()?;

        let mut stream = res.bytes_stream();
        let mut buf = Vec::<u8>::new();
        loop {
            tokio::select! {
                _ = cancel.notified() => {
                    return Ok(());
                }
                next = stream.next() => {
                    let bytes = match next {
                        Some(Ok(b)) => b,
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    };
                    buf.extend_from_slice(&bytes);
                    if buf.len() > MAX_STREAM_SCRATCH_BYTES {
                        return Err(anyhow::anyhow!(
                            "Ollama stream buffer exceeded {}MiB — malformed/non-newline-delimited JSON",
                            MAX_STREAM_SCRATCH_BYTES / (1024 * 1024)
                        ));
                    }
                    // Ollama emits newline-delimited JSON.
                    while let Some(idx) = buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=idx).collect();
                        let line = &line[..line.len() - 1];
                        if line.is_empty() {
                            continue;
                        }
                        let chunk: Chunk = match serde_json::from_slice(line) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        if let Some(m) = chunk.message {
                            if !m.content.is_empty() {
                                let _ = tx.send(m.content);
                            }
                        }
                        if chunk.done {
                            return Ok(());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/* ------------------------------------------------------------------ */
/* OpenAI-compatible (SSE)                                            */
/* ------------------------------------------------------------------ */

pub struct OpenAiChat {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub http: Client,
}

#[async_trait::async_trait]
impl ChatClient for OpenAiChat {
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tx: &UnboundedSender<String>,
        cancel: &tokio::sync::Notify,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            messages: &'a [ChatMessage],
            stream: bool,
        }
        #[derive(Deserialize)]
        struct Delta {
            #[serde(default)]
            content: Option<String>,
        }
        #[derive(Deserialize)]
        struct Choice {
            delta: Delta,
        }
        #[derive(Deserialize)]
        struct Chunk {
            choices: Vec<Choice>,
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&Req {
                model: &self.model,
                messages,
                stream: true,
            })
            .send()
            .await?
            .error_for_status()?;

        let mut stream = res.bytes_stream();
        let mut buf = Vec::<u8>::new();
        loop {
            tokio::select! {
                _ = cancel.notified() => return Ok(()),
                next = stream.next() => {
                    let bytes = match next {
                        Some(Ok(b)) => b,
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    };
                    buf.extend_from_slice(&bytes);
                    if buf.len() > MAX_STREAM_SCRATCH_BYTES {
                        return Err(anyhow::anyhow!(
                            "SSE stream buffer exceeded {}MiB — malformed chunked response",
                            MAX_STREAM_SCRATCH_BYTES / (1024 * 1024)
                        ));
                    }
                    // SSE events are `data: <json>\n\n`.
                    while let Some(end) = find_double_newline(&buf) {
                        let event: Vec<u8> = buf.drain(..end + 2).collect();
                        let text = std::str::from_utf8(&event).unwrap_or("");
                        for line in text.lines() {
                            let Some(data) = line.strip_prefix("data: ") else {
                                continue;
                            };
                            if data.trim() == "[DONE]" {
                                return Ok(());
                            }
                            let Ok(chunk) = serde_json::from_str::<Chunk>(data) else {
                                continue;
                            };
                            for choice in chunk.choices {
                                if let Some(delta) = choice.delta.content {
                                    if !delta.is_empty() {
                                        let _ = tx.send(delta);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}
