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

/// Linux and macOS: POSIX `df -Pk`, whose output is the same on both, rather than `statvfs`, whose
/// struct layout is not.
#[cfg(not(windows))]
pub fn free_bytes(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").arg("-Pk").arg(path).output().ok()?;
    df_available(&String::from_utf8_lossy(&out.stdout))
}

/// The "Available" column of `df -Pk`, in bytes. A mount point with spaces in its name only adds
/// words after it, so the column is counted from the left.
#[cfg_attr(windows, allow(dead_code))]
fn df_available(out: &str) -> Option<u64> {
    let kb: u64 = out.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb * 1024)
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
    fn df_available_reads_the_fourth_column() {
        let out = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                   /dev/sdb1         61498368  12345678  49152690      21% /media/me/WALKMAN SD\n";
        assert_eq!(df_available(out), Some(49152690 * 1024));
        assert_eq!(df_available("df: /nowhere: No such file or directory\n"), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn free_space_is_read_here() {
        assert!(free_bytes(Path::new(".")).is_some_and(|b| b > 0));
    }

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
