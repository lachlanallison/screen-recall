//! Open the system file manager with a file selected (Explorer on Windows, Finder on macOS, …).

use std::path::Path;
use std::process::Command;

/// Reveal `path` in the platform file manager (e.g. Explorer with file selected on Windows).
pub fn reveal_file(path: &Path) -> Result<(), String> {
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("not a file: {}", path.display()));
    }

    if cfg!(windows) {
        // `explorer /select,` + path as a separate argument (handles spaces in path).
        let st = Command::new("explorer")
            .arg("/select,")
            .arg(&path)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            return Err(format!("explorer exit {}", st));
        }
        return Ok(());
    }

    if cfg!(target_os = "macos") {
        let st = Command::new("open")
            .arg("-R")
            .arg(&path)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            return Err(format!("open -R exit {}", st));
        }
        return Ok(());
    }

    if cfg!(unix) {
        if let Some(dir) = path.parent() {
            let _ = Command::new("xdg-open").arg(dir).status();
        }
        return Ok(());
    }

    Err("reveal in folder: unsupported platform".to_string())
}
