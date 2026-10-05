//! The playlists made on the player: `cinder_playlists/*.m3u8` at the top of its drive.
//!
//! Cinder keeps its own playlists as ordinary m3u files of **the player's paths**
//! (`docs/PLAYLISTS.md` in the Cinder repository):
//!
//! ```text
//! #EXTM3U
//! #PLAYLIST:Late Night On The Bus
//! #CINDER-EDITED:1790000000
//! #EXTINF:-1,Wunderhorse - Teal
//! /contents/MUSIC/Wunderhorse - Cub/06 - Wunderhorse - Teal.flac
//! ```
//!
//! Every time the player changes one it stamps the file with `#CINDER-EDITED:`, and shows an
//! EDITED tag on the row until a PC tool has taken the playlist back. Taking it back is [`pull`]:
//! the playlist is written to a folder on the PC in paths the PC can use, and only once that copy
//! is known to be there is the stamp taken off the player's file.
//!
//! This is a different folder from the one a sync writes. A sync puts the PC's playlists at the
//! music root for Sony's database, and sweeps what it did not plan there; `cinder_playlists` is
//! out of its way and a sync never reads or removes anything in it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::stats::split_volume;

pub const FOLDER: &str = "cinder_playlists";
const EDITED_TAG: &str = "#CINDER-EDITED:";
const COVER_TAG: &str = "#EXTIMG:";
const NAME_TAG: &str = "#PLAYLIST:";

/// One playlist on the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerPlaylist {
    /// The file's name in `cinder_playlists`, extension included.
    pub file: String,
    /// What the player calls it: the `#PLAYLIST:` line, or the file's name without the extension.
    pub name: String,
    /// When the player last changed it, by its own clock, if it carries the stamp. A stamp with no
    /// readable time is `Some(0)`: the flag is the fact.
    pub edited: Option<i64>,
    pub tracks: usize,
    /// The file as it is on the player.
    pub body: String,
}

fn is_playlist(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case("m3u8") || e.eq_ignore_ascii_case("m3u"))
}

fn is_track(line: &str) -> bool {
    let l = line.trim();
    !l.is_empty() && !l.starts_with('#')
}

fn describe(file: &str, body: String) -> PlayerPlaylist {
    let mut name = file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string();
    let mut edited = None;
    for line in body.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix(NAME_TAG) {
            if !rest.trim().is_empty() {
                name = rest.trim().to_string();
            }
        } else if let Some(rest) = line.strip_prefix(EDITED_TAG) {
            edited = Some(rest.trim().parse::<i64>().unwrap_or(0).max(0));
        }
    }
    let tracks = body.lines().filter(|l| is_track(l)).count();
    PlayerPlaylist { file: file.to_string(), name, edited, tracks, body }
}

/// Every playlist in the player's own folder, by file name. `drive` is the top of the player's
/// internal memory as the PC sees it. A player that has never made a playlist has no folder, and
/// that is an empty list, not an error.
pub fn read_player(drive: &Path) -> io::Result<Vec<PlayerPlaylist>> {
    let dir = drive.join(FOLDER);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_file() || !is_playlist(&file) || file.starts_with('.') {
            continue;
        }
        // Not UTF-8 is still a playlist: read what can be read, as the player does.
        let body = fs::read(entry.path()).map(|b| String::from_utf8_lossy(&b).into_owned())?;
        if !body.is_empty() {
            out.push(describe(&file, body));
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(out)
}

/// The player's file without its `#CINDER-EDITED:` line, every other byte as it was. `None` when
/// there is no stamp to take off.
pub fn without_stamp(body: &str) -> Option<String> {
    let stamped = |line: &str| line.trim().starts_with(EDITED_TAG);
    if !body.lines().any(stamped) {
        return None;
    }
    Some(body.split_inclusive('\n').filter(|line| !stamped(line)).collect())
}

/// A player's playlist, rewritten for the PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForPc {
    pub body: String,
    /// Tracks written as a path below the music folder because they are not in the library given
    /// (or none was given).
    pub relative: usize,
    /// Lines that are not a path on either of the player's volumes. Left exactly as they were.
    pub foreign: usize,
}

/// The path below the music folder for a file the player names: `/contents/MUSIC/A/01.flac` and
/// `/contents_ext/MUSIC/A/01.flac` are both `A/01.flac`, which is the path Flint copied it from
/// in the library. A player synced at the top of the drive has no `MUSIC` to drop.
pub fn library_relative(player_path: &str) -> Option<String> {
    let (_, rest) = split_volume(player_path)?;
    let rest = rest.trim_start_matches('/');
    let below_music = rest.split_once('/').filter(|(first, _)| first.eq_ignore_ascii_case("music")).map(|(_, r)| r);
    Some(below_music.unwrap_or(rest).to_string()).filter(|r| !r.is_empty())
}

/// Rewrite a player's playlist so a PC can use it.
///
/// * The edited stamp goes: it says "the PC has not seen this", and this is the PC seeing it.
/// * The cover line goes: it is a path on the player.
/// * Each track becomes the file in `library` when it is there, and otherwise its path below the
///   music folder, which is how Flint's own playlists name a track.
/// * The name, the labels and anything else stay as they are.
pub fn for_pc(body: &str, library: Option<&Path>) -> ForPc {
    let mut out = ForPc { body: String::new(), relative: 0, foreign: 0 };
    for line in body.lines() {
        let l = line.trim();
        if l.starts_with(EDITED_TAG) || l.starts_with(COVER_TAG) {
            continue;
        }
        if !is_track(l) {
            out.body.push_str(l);
            out.body.push('\n');
            continue;
        }
        let Some(rel) = library_relative(&l.replace('\\', "/")) else {
            out.foreign += 1;
            out.body.push_str(l);
            out.body.push('\n');
            continue;
        };
        let in_library =
            library.map(|lib| rel.split('/').fold(lib.to_path_buf(), |p, part| p.join(part))).filter(|p| p.is_file());
        match in_library {
            Some(path) => out.body.push_str(&path.to_string_lossy()),
            None => {
                out.relative += 1;
                out.body.push_str(&rel);
            }
        }
        out.body.push('\n');
    }
    out
}

/// What [`pull`] found and did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pulled {
    /// Playlists in the player's folder.
    pub on_player: usize,
    /// Of those, the ones the player has changed since a PC last took them.
    pub edited: usize,
    /// Playlists written to the PC folder (or that were there already, the same).
    pub pulled: usize,
    /// Stamps taken off the player's files.
    pub cleared: usize,
    /// Playlists that could not be written or cleared. Their stamps are still on.
    pub failed: usize,
}

fn write_through_temp(path: &Path, body: &str) -> io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.flint-partial"));
    let written = fs::write(&tmp, body).and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Take the playlists the player has changed back to the PC, and take their EDITED mark off.
///
/// For each playlist that carries the stamp: write it to `to` under the same file name, in PC
/// paths ([`for_pc`]); read that copy back; and only then rewrite the player's file without the
/// stamp. A playlist that fails at any step keeps its stamp, so it is offered again next time.
///
/// A file of the same name already in `to` that says something else is kept beside the new one as
/// `<name>.bak` — one generation, not a playlist, so a later sync does not send it to the player.
/// Nothing is written anywhere unless `apply` is set.
pub fn pull(
    drive: &Path,
    to: &Path,
    library: Option<&Path>,
    apply: bool,
    log: &mut dyn FnMut(&str),
) -> Result<Pulled, String> {
    let lists = read_player(drive).map_err(|e| format!("{}: {e}", drive.join(FOLDER).display()))?;
    let mut report = Pulled { on_player: lists.len(), ..Pulled::default() };
    let same_folder = |a: &Path, b: &Path| match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    if same_folder(to, &drive.join(FOLDER)) {
        return Err(format!("{} is the player's own playlist folder; choose a folder on the PC", to.display()));
    }
    // A playlist is opened from wherever it is saved, so it has to name its files in full.
    let library = library.map(|lib| std::path::absolute(lib).unwrap_or_else(|_| lib.to_path_buf()));
    let library = library.as_deref();
    for list in lists.iter().filter(|l| l.edited.is_some()) {
        report.edited += 1;
        let pc = for_pc(&list.body, library);
        let dst = to.join(&list.file);
        let note = match (pc.relative, pc.foreign) {
            (0, 0) => String::new(),
            (r, 0) => format!(" ({r} not found in the library, written as a path below the music folder)"),
            (r, f) => format!(" ({r} written as a path below the music folder, {f} left as the player wrote them)"),
        };
        if !apply {
            log(&format!("would pull  {} — {} track(s){note}", list.name, list.tracks));
            continue;
        }
        let already = fs::read_to_string(&dst).ok();
        let step = (|| -> io::Result<()> {
            if already.as_deref() != Some(pc.body.as_str()) {
                fs::create_dir_all(to)?;
                if already.is_some() {
                    let bak: PathBuf = to.join(format!("{}.bak", list.file));
                    fs::copy(&dst, &bak)?;
                    log(&format!("kept the PC's earlier {} as {}", list.file, bak.display()));
                }
                write_through_temp(&dst, &pc.body)?;
            }
            // The stamp comes off only once the PC's copy is known to be there and whole.
            if fs::read_to_string(&dst)? != pc.body {
                return Err(io::Error::other("the copy on the PC did not read back as written"));
            }
            Ok(())
        })();
        if let Err(e) = step {
            report.failed += 1;
            log(&format!("FAILED {}: {e}. Its EDITED mark is still on", list.name));
            continue;
        }
        report.pulled += 1;
        log(&format!("pulled  {} — {} track(s){note}", list.name, list.tracks));
        let Some(clean) = without_stamp(&list.body) else { continue };
        match write_through_temp(&drive.join(FOLDER).join(&list.file), &clean) {
            Ok(()) => report.cleared += 1,
            Err(e) => {
                report.failed += 1;
                log(&format!("FAILED to take the EDITED mark off {}: {e}", list.name));
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example in Cinder's `docs/PLAYLISTS.md`.
    const SAMPLE: &str = "#EXTM3U\n#PLAYLIST:Late Night On The Bus\n#CINDER-EDITED:1790000000\n\
        #EXTIMG:/contents/Art/late-night.jpg\n#EXTINF:-1,Wunderhorse - Teal\n\
        /contents/MUSIC/Wunderhorse - Cub/06 - Wunderhorse - Teal.flac\n\
        /contents_ext/MUSIC/Nick Drake/01 Pink Moon.flac\n";

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("flint-pl-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn player_with(dir: &Dir, files: &[(&str, &str)]) -> PathBuf {
        let drive = dir.0.join("drive");
        fs::create_dir_all(drive.join(FOLDER)).unwrap();
        for (name, body) in files {
            fs::write(drive.join(FOLDER).join(name), body).unwrap();
        }
        drive
    }

    #[test]
    fn the_documented_playlist_is_read_with_its_name_stamp_and_tracks() {
        let dir = Dir::new("read");
        let drive =
            player_with(&dir, &[("late.m3u8", SAMPLE), ("plain.M3U", "/contents/MUSIC/a.flac\n"), ("notes.txt", "x")]);
        let lists = read_player(&drive).unwrap();
        assert_eq!(lists.len(), 2, "only playlists are read");
        assert_eq!(
            (lists[0].name.as_str(), lists[0].edited, lists[0].tracks),
            ("Late Night On The Bus", Some(1790000000), 2)
        );
        assert_eq!((lists[1].name.as_str(), lists[1].edited, lists[1].tracks), ("plain", None, 1));
        assert!(read_player(&dir.0.join("no-such-drive")).unwrap().is_empty(), "no folder is no playlists");
    }

    #[test]
    fn taking_the_stamp_off_changes_nothing_else() {
        let clean = without_stamp(SAMPLE).unwrap();
        assert_eq!(clean, SAMPLE.replace("#CINDER-EDITED:1790000000\n", ""));
        assert_eq!(without_stamp(&clean), None, "nothing to take off twice");
        // Windows line ends, an unreadable time and a last line with no newline all survive.
        let odd = "#EXTM3U\r\n#CINDER-EDITED:soon\r\n/contents/MUSIC/a.flac";
        assert_eq!(without_stamp(odd).unwrap(), "#EXTM3U\r\n/contents/MUSIC/a.flac");
    }

    #[test]
    fn a_player_path_becomes_the_path_below_the_music_folder() {
        assert_eq!(library_relative("/contents/MUSIC/A/01.flac").as_deref(), Some("A/01.flac"));
        assert_eq!(library_relative("/contents_ext/Music/A/01.flac").as_deref(), Some("A/01.flac"));
        assert_eq!(
            library_relative("/contents/A/01.flac").as_deref(),
            Some("A/01.flac"),
            "synced at the top of the drive"
        );
        assert_eq!(
            library_relative("/contents/MUSIC").as_deref(),
            Some("MUSIC"),
            "a file called MUSIC is not the folder"
        );
        assert_eq!(library_relative("C:/Music/A/01.flac"), None);
        assert_eq!(library_relative("/contents/"), None);
    }

    #[test]
    fn the_pc_copy_names_library_files_and_drops_what_only_the_player_can_use() {
        let dir = Dir::new("forpc");
        let library = dir.0.join("library");
        fs::create_dir_all(library.join("Wunderhorse - Cub")).unwrap();
        let teal = library.join("Wunderhorse - Cub").join("06 - Wunderhorse - Teal.flac");
        fs::write(&teal, b"x").unwrap();
        let pc = for_pc(&format!("{SAMPLE}D:\\elsewhere\\x.flac\n"), Some(&library));
        assert_eq!(
            pc.body,
            format!(
                "#EXTM3U\n#PLAYLIST:Late Night On The Bus\n#EXTINF:-1,Wunderhorse - Teal\n{}\nNick Drake/01 Pink Moon.flac\nD:\\elsewhere\\x.flac\n",
                teal.display()
            )
        );
        assert_eq!((pc.relative, pc.foreign), (1, 1));
        // With no library every track is a path below the music folder: what `flint sync
        // --playlists` reads.
        let pc = for_pc(SAMPLE, None);
        assert!(
            pc.body.ends_with("Wunderhorse - Cub/06 - Wunderhorse - Teal.flac\nNick Drake/01 Pink Moon.flac\n"),
            "{}",
            pc.body
        );
        assert_eq!((pc.relative, pc.foreign), (2, 0));
    }

    #[test]
    fn a_pull_writes_the_pc_copy_and_only_then_clears_the_mark() {
        let dir = Dir::new("pull");
        let untouched = "#EXTM3U\n#PLAYLIST:From the PC\n/contents/MUSIC/a.flac\n";
        let drive = player_with(&dir, &[("late.m3u8", SAMPLE), ("pc.m3u8", untouched)]);
        let to = dir.0.join("pc-playlists");
        let on_player = drive.join(FOLDER).join("late.m3u8");

        // A dry run says what it would do and writes nothing, on either side.
        let mut lines = Vec::new();
        let dry = pull(&drive, &to, None, false, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!(dry, Pulled { on_player: 2, edited: 1, pulled: 0, cleared: 0, failed: 0 });
        assert!(lines[0].starts_with("would pull  Late Night On The Bus"), "{lines:?}");
        assert!(!to.exists() && fs::read_to_string(&on_player).unwrap() == SAMPLE);

        let done = pull(&drive, &to, None, true, &mut |_| {}).unwrap();
        assert_eq!(done, Pulled { on_player: 2, edited: 1, pulled: 1, cleared: 1, failed: 0 });
        assert_eq!(fs::read_to_string(to.join("late.m3u8")).unwrap(), for_pc(SAMPLE, None).body);
        assert_eq!(fs::read_to_string(&on_player).unwrap(), without_stamp(SAMPLE).unwrap());
        assert!(!to.join("pc.m3u8").exists(), "a playlist the player has not changed is not pulled");
        assert_eq!(fs::read_to_string(drive.join(FOLDER).join("pc.m3u8")).unwrap(), untouched);
        assert!(fs::read_dir(drive.join(FOLDER)).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("partial")));

        // Nothing is edited now, so a second pull does nothing.
        assert_eq!(pull(&drive, &to, None, true, &mut |_| {}).unwrap(), Pulled { on_player: 2, ..Pulled::default() });
    }

    #[test]
    fn the_pcs_earlier_copy_is_kept_once_and_an_identical_one_is_not_rewritten() {
        let dir = Dir::new("bak");
        let drive = player_with(&dir, &[("late.m3u8", SAMPLE)]);
        let to = dir.0.join("pc-playlists");
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("late.m3u8"), "#EXTM3U\nold/one.flac\n").unwrap();
        let mut lines = Vec::new();
        pull(&drive, &to, None, true, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!(fs::read_to_string(to.join("late.m3u8.bak")).unwrap(), "#EXTM3U\nold/one.flac\n");
        assert!(lines.iter().any(|l| l.contains("kept the PC's earlier late.m3u8")), "{lines:?}");

        // The player changes it again without changing what it holds: the PC copy is already
        // right, so it is not rewritten and no second .bak is made, but the mark still comes off.
        fs::write(drive.join(FOLDER).join("late.m3u8"), SAMPLE).unwrap();
        fs::remove_file(to.join("late.m3u8.bak")).unwrap();
        let again = pull(&drive, &to, None, true, &mut |_| {}).unwrap();
        assert_eq!((again.pulled, again.cleared), (1, 1));
        assert!(!to.join("late.m3u8.bak").exists());
    }

    #[test]
    fn a_copy_that_cannot_be_written_leaves_the_mark_on() {
        let dir = Dir::new("fail");
        let drive = player_with(&dir, &[("late.m3u8", SAMPLE)]);
        // The destination is a file, not a folder: nothing can be written into it.
        let to = dir.0.join("not-a-folder");
        fs::write(&to, b"x").unwrap();
        let mut lines = Vec::new();
        let r = pull(&drive, &to, None, true, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!((r.pulled, r.cleared, r.failed), (0, 0, 1));
        assert_eq!(
            fs::read_to_string(drive.join(FOLDER).join("late.m3u8")).unwrap(),
            SAMPLE,
            "the stamp is still there"
        );
        assert!(lines[0].contains("EDITED mark is still on"), "{lines:?}");
        // And the player's own folder is refused as a destination outright.
        assert!(pull(&drive, &drive.join(FOLDER), None, true, &mut |_| {}).is_err());
    }
}
