//! Disk-space monitoring for the data directory.

use std::path::Path;

/// Returns (free_bytes, total_bytes) for the volume containing `path`.
/// On Windows uses GetDiskFreeSpaceExW; falls back to statfs on Unix.
pub fn volume_free_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut free: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;

        let ok = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(wide.as_ptr()),
                Some(&mut free),
                Some(&mut total),
                Some(&mut total_free),
            )
        };
        if ok.is_ok() {
            return Some((free, total));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        use libc::{c_char, statfs};
        use std::ffi::CString;
        let c_path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
        let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { statfs(c_path.as_ptr() as *const c_char, &mut buf) };
        if rc == 0 {
            let block_size = buf.f_bsize as u64;
            let free = buf.f_bavail as u64 * block_size;
            let total = buf.f_blocks as u64 * block_size;
            return Some((free, total));
        }
    }
    None
}

/// Percentage of free space (0.0–100.0).
pub fn free_space_pct(path: &Path) -> Option<f64> {
    let (free, total) = volume_free_space(path)?;
    if total == 0 {
        return None;
    }
    Some((free as f64 / total as f64) * 100.0)
}

/// Sum the on-disk sizes of a list of file paths. Missing files contribute 0.
pub fn sum_file_sizes(paths: &[String]) -> u64 {
    let mut total = 0u64;
    for p in paths {
        if let Ok(m) = std::fs::metadata(p) {
            total += m.len();
        }
    }
    total
}
