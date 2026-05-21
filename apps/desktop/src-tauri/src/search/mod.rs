//! Retrieval: full-text and semantic search over the frame index.

use anyhow::Result;
use reqwest::Client;
use serde::Serialize;

use crate::embed;
use crate::state::SharedState;
use crate::store::Frame;
use crate::util::perf_log;

#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub frame: Frame,
    pub score: f32,
    pub snippet: Option<String>,
}

/// Run a hybrid search. When `semantic` is true we embed the query and
/// blend cosine similarity scores with the FTS5 BM25 scores; otherwise
/// we return FTS5 results only.
pub async fn search(
    state: &SharedState,
    http: &Client,
    query: &str,
    limit: usize,
    semantic: bool,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
) -> Result<Vec<SearchHit>> {
    let started = std::time::Instant::now();
    let fts = state
        .store
        .fts_search_range(&fts_query(query), limit as i64, start_ts, end_ts)?;

    if !semantic {
        perf_log::record(
            state,
            "search_fts_only",
            serde_json::json!({
                "query_len": query.len(),
                "limit": limit,
                "hits": fts.len(),
                "ms": started.elapsed().as_millis() as u64,
            }),
        );
        return Ok(fts
            .into_iter()
            .map(|(f, snip, s)| SearchHit {
                frame: f,
                score: s,
                snippet: Some(snip),
            })
            .collect());
    }

    // Try semantic + merge. If the embedder fails (e.g. Ollama down) we
    // silently fall back to FTS-only.
    let cfg = state.config();
    let embedder = embed::build_embedder(&cfg, http);

    match embedder.embed(query).await {
        Ok(qvec) => {
            let sem = state
                .store
                .semantic_search_range(&qvec, limit, start_ts, end_ts)
                .unwrap_or_default();
            perf_log::record(
                state,
                "search_hybrid_ok",
                serde_json::json!({
                    "query_len": query.len(),
                    "limit": limit,
                    "fts_hits": fts.len(),
                    "sem_hits": sem.len(),
                    "ms": started.elapsed().as_millis() as u64,
                }),
            );
            Ok(merge(fts, sem, limit))
        }
        Err(err) => {
            tracing::warn!(?err, "embedding query failed; using FTS only");
            perf_log::record(
                state,
                "search_semantic_fallback",
                serde_json::json!({
                    "query_len": query.len(),
                    "limit": limit,
                    "fts_hits": fts.len(),
                    "ms": started.elapsed().as_millis() as u64,
                    "error": err.to_string(),
                }),
            );
            Ok(fts
                .into_iter()
                .map(|(f, snip, s)| SearchHit {
                    frame: f,
                    score: s,
                    snippet: Some(snip),
                })
                .collect())
        }
    }
}

fn merge(fts: Vec<(Frame, String, f32)>, sem: Vec<(Frame, f32)>, limit: usize) -> Vec<SearchHit> {
    use std::collections::HashMap;

    // Weighted blend: 0.4 * fts + 0.6 * semantic.
    let mut map: HashMap<i64, SearchHit> = HashMap::new();
    for (f, snip, s) in fts {
        map.insert(
            f.id,
            SearchHit {
                frame: f,
                score: s * 0.4,
                snippet: Some(snip),
            },
        );
    }
    for (f, s) in sem {
        map.entry(f.id)
            .and_modify(|h| h.score += s * 0.6)
            .or_insert(SearchHit {
                frame: f,
                score: s * 0.6,
                snippet: None,
            });
    }
    let mut out: Vec<SearchHit> = map.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(limit);
    out
}

/// FTS5 has picky syntax. For a friendly UX we:
///   - quote terms to avoid magic-operator errors on user punctuation
///   - fall back to a single-word match when the user typed nothing tokenizable
fn fts_query(q: &str) -> String {
    let words: Vec<String> = q
        .split_whitespace()
        .map(|w| {
            let cleaned: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if cleaned.is_empty() {
                String::new()
            } else {
                format!("\"{}\"", cleaned)
            }
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        "\"\"".to_string()
    } else {
        words.join(" ")
    }
}
