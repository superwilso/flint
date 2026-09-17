//! Free space on a volume, without a dependency: `GetDiskFreeSpaceExW` on Windows, and nothing
//! anywhere else — Flint's own platform is Windows, and every caller treats `None` as "unknown" and
//! falls back to a stated budget.

use std::path::Path;

/// Bytes free to this user on the volume holding `path`, or `None` if it cannot be determined.
#[cfg(windows)]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, free_to_caller: *mut u64, total: *mut u64, free: *mut u64)
            -> i32;
    }

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let (mut free_to_caller, mut total, mut free) = (0u64, 0u64, 0u64);
    // Safety: the string is NUL-terminated and the three outputs are valid for the call.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_to_caller, &mut total, &mut free) };
    (ok != 0).then_some(free_to_caller)
}

#[cfg(not(windows))]
pub fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

/// `bytes` as a short human-readable string, the way Flint prints sizes everywhere.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_people_write_them() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(999), "999 B");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(human(54 * 1024 * 1024 * 1024), "54.0 GB");
        assert_eq!(human(512 * 1024 * 1024 * 1024), "512 GB");
    }
}
