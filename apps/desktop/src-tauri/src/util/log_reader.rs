use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Read up to `max_lines` from the end of a file, reading at most `max_bytes`
/// backwards. If the file doesn't exist, returns an empty `Vec`.
pub fn read_tail_lines(path: &Path, max_lines: usize, max_bytes: u64) -> std::io::Result<Vec<String>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max_bytes.max(1));
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(|x| x.to_string()).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > max_lines {
        lines.drain(0..(lines.len() - max_lines));
    }
    Ok(lines)
}
