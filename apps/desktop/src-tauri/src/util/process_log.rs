use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use serde_json::json;

use crate::state::SharedState;

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Append one JSON line under `<data_dir>/process-events.jsonl`.
/// Best-effort only: failures are intentionally ignored.
pub fn record(
    state: &SharedState,
    source: &str,
    command: &str,
    args: &[String],
    cwd: Option<&str>,
) {
    let Ok(_guard) = write_lock().lock() else {
        return;
    };
    let path = state.config().data_dir.join("process-events.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(_) => return,
    };
    let line = json!({
        "ts": Utc::now().to_rfc3339(),
        "source": source,
        "command": command,
        "args": args,
        "cwd": cwd.unwrap_or(""),
    });
    let _ = writeln!(f, "{}", line);
}
