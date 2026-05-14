use std::path::PathBuf;

use crate::util::process::configure_hidden_std;

/// Locate a working tesseract binary by probing known paths with `--version`.
/// Returns `Some(path)` if found, `None` otherwise.
pub fn find_binary() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("tesseract.exe"),
        PathBuf::from("tesseract"),
        PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe"),
    ];

    for c in candidates {
        let mut cmd = std::process::Command::new(&c);
        configure_hidden_std(&mut cmd);
        let ok = cmd
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

/// Check known install locations (file-existence only, no subprocess).
/// Used by health snapshot for a quick "known path" hint.
pub fn probe_known_path() -> Option<String> {
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
