use std::fs::OpenOptions;
use std::io::Write;

use chrono::Utc;
use serde_json::{json, Value};

use crate::state::SharedState;

/// Append one JSON line under `<data_dir>/perf-events.jsonl`.
/// Best-effort only: failures are intentionally ignored.
pub fn record(state: &SharedState, event: &str, fields: Value) {
    let path = state.config().data_dir.join("perf-events.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let line = json!({
        "ts": Utc::now().to_rfc3339(),
        "event": event,
        "fields": fields,
    });
    let _ = writeln!(f, "{}", line);
}

