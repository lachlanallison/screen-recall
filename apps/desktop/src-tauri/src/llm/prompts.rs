use crate::config::AppConfig;
use crate::store::Frame;
use chrono::{Local, TimeZone};

pub const SYSTEM_PROMPT: &str = r#"You are ScreenRecall, a helpful assistant that answers questions
about the user's recent computer activity. You are given numbered
context entries taken from OCR of the user's screen at specific times.
Each entry begins with `[i | timestamp | app — window title]` followed by the raw OCR text.

Important: You do not receive pixels—only text from OCR. The OCR may include
browser chrome: address bar text, tabs, URLs, and page titles often appear as
lines in the block (not always at the top). Small UI text can be misread or
missing. When the user asks about a webpage, URL, or address bar, search the
OCR for URL-like strings (http/https), domain names, and path segments; quote
them exactly when present. If OCR clearly shows no URL for that window, say so.
Do not claim the address bar is "not visible" in the image—you cannot see images;
only say the URL or bar text is not present in the OCR text.

Use the entries to answer the user's question. Cite the entries you
use by their number in square brackets (e.g. "[2]"). If the context
does not contain the answer, say so briefly instead of inventing one.
Be concise."#;

/// Resolved system prompt: custom text from Settings, or [`SYSTEM_PROMPT`].
pub fn effective_system_prompt(cfg: &AppConfig) -> String {
    match cfg.chat_system_prompt.as_deref().map(|s| s.trim()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => SYSTEM_PROMPT.to_string(),
    }
}

/// Build the user message that stuffs retrieved frames in as context.
pub fn build_user_message(query: &str, frames: &[(Frame, Option<String>)]) -> String {
    let mut s = String::new();
    s.push_str("Context:\n");
    for (i, (frame, ocr)) in frames.iter().enumerate() {
        let idx = i + 1;
        let app = frame.app.as_deref().unwrap_or("—");
        let title = frame.window_title.as_deref().unwrap_or("");
        let mut when = Local
            .timestamp_millis_opt(frame.ts)
            .single()
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| frame.ts.to_string());
        let held_ms = frame.static_until_ms.saturating_sub(frame.ts);
        if held_ms > 2_000 {
            let s_held = held_ms / 1000;
            when.push_str(&format!(" · static ~{}s", s_held));
        }
        s.push_str(&format!("[{idx} | {when} | {app} — {title}]\n"));
        if let Some(text) = ocr {
            // Enough room for dense UIs (browsers, IDEs) while staying within model limits.
            let trimmed = truncate(text, 4000);
            s.push_str(&trimmed);
            s.push_str("\n\n");
        } else {
            s.push_str("(no OCR text)\n\n");
        }
    }
    s.push_str("Question: ");
    s.push_str(query);
    s
}

/// Truncate to at most `max` **bytes** without splitting a UTF-8 codepoint.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s[..end].to_string();
    out.push_str(" …");
    out
}
