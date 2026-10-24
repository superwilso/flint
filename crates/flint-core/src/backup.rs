//! A copy of what the player keeps for itself, taken whenever the window reads the player (audit
//! G1.1).
//!
//! The player's internal memory holds the only copy of what was made on it: the playlists, the
//! likes, the ratings and play counts, the settings, the palettes and the play log. A reset, a
//! format or a restore of the whole player loses them. Each read copies them into a dated folder
//! under Flint's data folder, `backups/<YYYY-MM-DD HHMMSS>/`, unless nothing has changed since the
//! newest one. [`restore`] puts one back, and takes a copy of the player as it is first, so a
//! restore can itself be undone.
//!
//! What counts: every `cinder_*.conf` and `cinder_*.tsv` at the top of the drive, `.scrobbler.log`,
//! and the `cinder_playlists`, `cinder_palettes` and `Art` folders. Logs, probe output and marker
//! files are left out; the music is the library's.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const FOLDER: &str = "backups";
/// How many copies are kept; the oldest go first.
pub const KEEP: usize = 20;
const DIRS: [&str; 3] = ["cinder_playlists", "cinder_palettes", "Art"];

fn wanted_file(name: &str) -> bool {
    name == ".scrobbler.log" || (name.starts_with("cinder_") && (name.ends_with(".conf") || name.ends_with(".tsv")))
}

/// The player's own files on `drive`, as paths relative to it with `/`, sorted.
pub fn files_on(drive: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(top) = fs::read_dir(drive) {
        for e in top.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_file()) && wanted_file(&name) {
                out.push(name);
            }
        }
    }
    for dir in DIRS {
        walk(&drive.join(dir), dir, &mut out);
    }
    out.sort();
    out
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            walk(&e.path(), &format!("{rel}/{name}"), out);
        } else if t.is_file() {
            out.push(format!("{rel}/{name}"));
        }
    }
}

fn path_of(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

/// The copies in `data`, oldest first.
pub fn list(data: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(data.join(FOLDER))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Does the copy at `dir` hold exactly `files` from `drive`, byte for byte?
fn same(drive: &Path, dir: &Path, files: &[String]) -> bool {
    files_on(dir) == files
        && files.iter().all(|rel| match (fs::read(path_of(drive, rel)), fs::read(path_of(dir, rel))) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Taken {
    /// Nothing of the player's own on the drive.
    Nothing,
    /// The newest copy already holds this.
    Unchanged(String),
    New {
        name: String,
        files: usize,
        bytes: u64,
    },
}

/// Copy the player's own files on `drive` into `data/backups/<stamp>`, unless the newest copy
/// already holds them. Written to a hidden folder first and renamed, so a copy that is there is
/// whole. Keeps the newest [`KEEP`].
pub fn take(drive: &Path, data: &Path, stamp: &str) -> io::Result<Taken> {
    let files = files_on(drive);
    if files.is_empty() {
        return Ok(Taken::Nothing);
    }
    let root = data.join(FOLDER);
    if let Some(newest) = list(data).pop() {
        if same(drive, &root.join(&newest), &files) {
            return Ok(Taken::Unchanged(newest));
        }
    }
    let tmp = root.join(format!(".{stamp}"));
    let _ = fs::remove_dir_all(&tmp);
    let mut bytes = 0;
    for rel in &files {
        let to = path_of(&tmp, rel);
        fs::create_dir_all(to.parent().unwrap_or(&tmp))?;
        bytes += fs::copy(path_of(drive, rel), &to)?;
    }
    // Two copies in one second: the second gets a suffix rather than replacing the first.
    let mut name = stamp.to_string();
    let mut n = 1;
    while root.join(&name).exists() {
        n += 1;
        name = format!("{stamp} ({n})");
    }
    fs::rename(&tmp, root.join(&name))?;
    let all = list(data);
    for old in &all[..all.len().saturating_sub(KEEP)] {
        let _ = fs::remove_dir_all(root.join(old));
    }
    Ok(Taken::New { name, files: files.len(), bytes })
}

/// Put copy `name` back on `drive`: each of its files replaces the player's. Files the player has
/// that the copy does not are left alone. A copy of the player as it is is taken first (`stamp`),
/// and returned, so the restore can be undone with it.
pub fn restore(data: &Path, name: &str, drive: &Path, stamp: &str) -> io::Result<(Taken, usize)> {
    let from = data.join(FOLDER).join(name);
    if name.starts_with('.') || !from.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no backup called {name}")));
    }
    let before = take(drive, data, stamp)?;
    let files = files_on(&from);
    for rel in &files {
        let to = path_of(drive, rel);
        fs::create_dir_all(to.parent().unwrap_or(drive))?;
        fs::copy(path_of(&from, rel), &to)?;
    }
    Ok((before, files.len()))
}

/// A folder name for now, in local time: `2026-10-23 183005`. Sorts by time.
pub fn stamp_now() -> String {
    let utc = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let t = utc + crate::localtime::utc_offset_at(utc);
    let (y, m, d) = crate::localtime::civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("flint-backup-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn put(root: &Path, rel: &str, body: &str) {
        let p = path_of(root, rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn the_players_own_files_and_nothing_else() {
        let d = Dir::new("pick");
        for (rel, body) in [
            ("cinder_settings.conf", "theme=night"),
            ("cinder_liked.conf", "1"),
            ("cinder_stats.tsv", "x"),
            (".scrobbler.log", "#AUDIOSCROBBLER/1.1"),
            ("cinder_playlists/Night Bus.m3u8", "#EXTM3U"),
            ("cinder_palettes/mine.palette", "p"),
            ("Art/covers/a.jpg", "jpg"),
            ("cinderhome.log", "log"),
            ("cinder_discovery.txt", "probe"),
            ("cinder_screen.png", "png"),
            ("MUSIC/A/01.flac", "music"),
        ] {
            put(&d.0, rel, body);
        }
        assert_eq!(
            files_on(&d.0),
            [
                ".scrobbler.log",
                "Art/covers/a.jpg",
                "cinder_liked.conf",
                "cinder_palettes/mine.palette",
                "cinder_playlists/Night Bus.m3u8",
                "cinder_settings.conf",
                "cinder_stats.tsv"
            ]
        );
    }

    #[test]
    fn a_copy_is_taken_when_something_changed_and_restored_with_an_undo() {
        let player = Dir::new("player");
        let data = Dir::new("data");
        assert_eq!(take(&player.0, &data.0, "2026-10-23 100000").unwrap(), Taken::Nothing);
        put(&player.0, "cinder_liked.conf", "1\n2\n");
        put(&player.0, "cinder_playlists/Night Bus.m3u8", "#EXTM3U\n/contents/MUSIC/a.flac\n");
        let first = take(&player.0, &data.0, "2026-10-23 100001").unwrap();
        assert_eq!(
            first,
            Taken::New {
                name: "2026-10-23 100001".into(),
                files: 2,
                bytes: ("1\n2\n".len() + "#EXTM3U\n/contents/MUSIC/a.flac\n".len()) as u64
            }
        );
        assert_eq!(
            take(&player.0, &data.0, "2026-10-23 100002").unwrap(),
            Taken::Unchanged("2026-10-23 100001".into()),
            "the same files are not copied twice"
        );

        // The player is reset: the playlist is gone and the likes are different.
        fs::remove_dir_all(player.0.join("cinder_playlists")).unwrap();
        put(&player.0, "cinder_liked.conf", "9\n");
        put(&player.0, "cinder_settings.conf", "new=1\n");
        let (undo, n) = restore(&data.0, "2026-10-23 100001", &player.0, "2026-10-23 110000").unwrap();
        assert_eq!(n, 2);
        assert!(matches!(undo, Taken::New { ref name, files: 2, .. } if name == "2026-10-23 110000"), "{undo:?}");
        assert_eq!(fs::read_to_string(player.0.join("cinder_liked.conf")).unwrap(), "1\n2\n");
        assert!(player.0.join("cinder_playlists/Night Bus.m3u8").is_file());
        assert_eq!(fs::read_to_string(player.0.join("cinder_settings.conf")).unwrap(), "new=1\n", "left alone");
        assert_eq!(list(&data.0), ["2026-10-23 100001", "2026-10-23 110000"]);
        assert!(restore(&data.0, "nope", &player.0, "x").is_err());
    }

    #[test]
    fn only_the_newest_are_kept() {
        let player = Dir::new("keep-player");
        let data = Dir::new("keep-data");
        for i in 0..KEEP + 3 {
            put(&player.0, "cinder_liked.conf", &i.to_string());
            take(&player.0, &data.0, &format!("2026-10-23 1{i:05}")).unwrap();
        }
        let kept = list(&data.0);
        assert_eq!(kept.len(), KEEP);
        assert_eq!(kept[0], "2026-10-23 100003", "the three oldest went");
    }

    #[test]
    fn the_stamp_sorts_by_time() {
        let s = stamp_now();
        assert_eq!(s.len(), "2026-10-23 183005".len());
        assert_eq!(&s[4..5], "-");
    }
}
