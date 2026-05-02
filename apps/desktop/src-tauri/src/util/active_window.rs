//! Thin wrapper around `active-win-pos-rs` that hides the `Err` type
//! (it's a `()` so it can't be `Send`-cast into anyhow errors cleanly)
//! and pins the result into a stable `WindowInfo` struct.

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub title: String,
    pub process: String,
}

pub fn active_window_info() -> Option<WindowInfo> {
    match active_win_pos_rs::get_active_window() {
        Ok(w) => Some(WindowInfo {
            title: w.title,
            process: w
                .process_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        }),
        Err(_) => None,
    }
}
