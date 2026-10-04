//! Cinder's ratings and play counts: `cinder_stats.tsv` at the top of the player's drive.
//!
//! The format is Cinder's (`docs/TRACK_DATA.md` in the Cinder repository):
//!
//! ```text
//! #CINDER-STATS/1
//! # path<TAB>rating<TAB>plays<TAB>last_played
//! /contents/MUSIC/Wunderhorse - Cub/06 - Teal.flac<TAB>5<TAB>12<TAB>1790000000
//! ```
//!
//! Keyed by the path **as the player sees it** (`/contents/…` internal, `/contents_ext/…` the SD
//! card), one line per track that has a rating or a play, written in path order so two saves of the
//! same state are the same bytes. A line that does not read is skipped on its own.
//!
//! Two things live here. Reading and writing the file, so a sync can carry it; and **seeding**: the
//! player starts every count at zero and never reads its own scrobble log, so the first counts come
//! from the history Flint already has ([`seed`]).

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::likes::keys;
use crate::scrobblelog::Entry;

pub const FILE_NAME: &str = "cinder_stats.tsv";
const HEADER: &str = "#CINDER-STATS/1";

/// What the player remembers about one track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    /// Stars, 0 to 5. 0 is "not rated".
    pub rating: u8,
    pub plays: u32,
    /// The player's clock at the last counted listen, 0 for never. Local time labelled as an epoch,
    /// exactly as the scrobble log carries it: compare it only with times from the same player.
    pub last_played: i64,
}

impl Stat {
    fn is_empty(&self) -> bool {
        self.rating == 0 && self.plays == 0
    }
}

/// The whole file, by player path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub tracks: BTreeMap<String, Stat>,
}

impl Stats {
    pub fn parse(body: &str) -> Stats {
        let mut tracks = BTreeMap::new();
        for line in body.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut cols = line.split('\t');
            let (Some(path), Some(rating), Some(plays)) = (cols.next(), cols.next(), cols.next()) else {
                continue;
            };
            let (Ok(rating), Ok(plays)) = (rating.trim().parse::<u8>(), plays.trim().parse::<u32>()) else {
                continue;
            };
            if path.is_empty() || rating > 5 {
                continue;
            }
            // Optional when written by hand.
            let last_played = cols.next().and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0);
            let stat = Stat { rating, plays, last_played };
            if !stat.is_empty() {
                tracks.insert(path.to_string(), stat);
            }
        }
        Stats { tracks }
    }

    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        out.push_str("\n# path\trating\tplays\tlast_played\n");
        for (path, s) in &self.tracks {
            if s.is_empty() {
                continue;
            }
            out.push_str(&format!("{path}\t{}\t{}\t{}\n", s.rating, s.plays, s.last_played));
        }
        out
    }

    /// A missing file is an empty one: a player that has never rated or counted anything.
    pub fn load(path: &Path) -> Stats {
        std::fs::read_to_string(path).map(|b| Stats::parse(&b)).unwrap_or_default()
    }

    /// Temporary file, then rename: a cable pulled mid-write leaves the old file or the new one.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("tsv.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, path)
    }
}

/// A track on the player, as [`seed`] needs it: where the player sees it, and what its tags say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceTrack {
    pub path: String,
    pub artist: String,
    pub title: String,
}

/// The player's path for a file on one of its volumes. `volume_root` is the drive as the PC sees
/// it; `sd_card` picks `/contents_ext` over `/contents`. `None` when the file is not under the root.
pub fn player_path(volume_root: &Path, file: &Path, sd_card: bool) -> Option<String> {
    let rel = file.strip_prefix(volume_root).ok()?;
    let mut out = String::from(if sd_card { "/contents_ext" } else { "/contents" });
    for part in rel.components() {
        out.push('/');
        out.push_str(&part.as_os_str().to_string_lossy());
    }
    Some(out)
}

/// What a seeding did, for the screen to say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seeded {
    /// Tracks that gained a count.
    pub tracks: usize,
    /// Plays written across them.
    pub plays: u64,
    /// Plays in the history that matched no track on the player.
    pub unmatched: u64,
    /// Plays whose artist and title belong to more than one file on the player. Not guessed at.
    pub ambiguous: u64,
    /// Tracks the player had already counted. Left exactly as they were.
    pub kept: usize,
}

/// Give tracks the player has never counted their plays from the scrobble history.
///
/// * Only listened rows count (`L`); a skip is not a play.
/// * **A count the player already has is never touched**, and neither is any rating: this fills the
///   gap before the player started counting, it does not second-guess what it has counted since.
/// * History is matched on artist and title, the same normalised key the likes use. When two files
///   on the player share one, the plays are not split or guessed: they are reported as ambiguous.
/// * `last_played` becomes the newest matching play. Both clocks are the player's, so they compare.
pub fn seed(stats: &mut Stats, history: &[Entry], device: &[DeviceTrack]) -> Seeded {
    let mut by_key: BTreeMap<String, Vec<&DeviceTrack>> = BTreeMap::new();
    for t in device {
        if !t.artist.is_empty() && !t.title.is_empty() {
            by_key.entry(keys::key(&t.artist, &t.title)).or_default().push(t);
        }
    }
    let mut counted: BTreeMap<&str, (u32, i64)> = BTreeMap::new();
    let mut report = Seeded::default();
    for e in history.iter().filter(|e| e.is_play()) {
        match by_key.get(&keys::key(&e.artist, &e.track)).map(Vec::as_slice) {
            None | Some([]) => report.unmatched += 1,
            Some([one]) => {
                let slot = counted.entry(one.path.as_str()).or_insert((0, 0));
                slot.0 = slot.0.saturating_add(1);
                slot.1 = slot.1.max(e.timestamp);
            }
            Some(_) => report.ambiguous += 1,
        }
    }
    for (path, (plays, last)) in counted {
        let stat = stats.tracks.entry(path.to_string()).or_default();
        if stat.plays > 0 {
            report.kept += 1;
            continue;
        }
        stat.plays = plays;
        stat.last_played = stat.last_played.max(last);
        report.tracks += 1;
        report.plays += u64::from(plays);
    }
    report
}

/// What [`seed_player`] found and did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerReport {
    /// Tracks with a rating, and with a count, before anything was changed.
    pub rated: usize,
    pub counted: usize,
    /// Listened rows found in the scrobble logs.
    pub history: usize,
    /// Music files on the player whose tags could be read.
    pub files: usize,
    pub seeded: Seeded,
    /// True when `cinder_stats.tsv` was written.
    pub written: bool,
}

/// Seed the player's play counts from the scrobble logs on its own drives.
///
/// `roots` are the player's volumes as the PC sees them, internal storage first and the SD card
/// after it: the stats file lives on the first, and the order is what turns a file into
/// `/contents/…` or `/contents_ext/…`. Nothing is written unless `apply` is set.
///
/// The history is whatever is still in `.scrobbler.log`. Rows Flint has already sent to Last.fm
/// and removed are not there to count, so seed before the first scrobble, or accept the smaller
/// number: a count that is too low is the honest direction to be wrong in.
pub fn seed_player(roots: &[std::path::PathBuf], apply: bool, log: &mut dyn FnMut(&str)) -> Result<PlayerReport, String> {
    let Some(internal) = roots.first() else {
        return Err("no player drive given".into());
    };
    let stats_path = internal.join(FILE_NAME);
    let mut stats = Stats::load(&stats_path);
    let mut report = PlayerReport {
        rated: stats.tracks.values().filter(|s| s.rating > 0).count(),
        counted: stats.tracks.values().filter(|s| s.plays > 0).count(),
        ..PlayerReport::default()
    };

    let mut history = Vec::new();
    let mut device = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        if let Ok(body) = std::fs::read_to_string(root.join(".scrobbler.log")) {
            history.extend(crate::scrobblelog::parse(&body).plays().cloned());
        }
        let music = crate::likes::Volume::new(root.clone(), "").music_dir();
        if !music.is_dir() {
            continue; // a card with no music folder, or a drive letter with nothing in it
        }
        let files = crate::library::walk(&music).map_err(|e| format!("{}: {e}", music.display()))?;
        for file in files {
            let (Some(path), Some(tags)) = (player_path(root, &file, i > 0), crate::tags::read(&file)) else {
                continue;
            };
            device.push(DeviceTrack { path, artist: tags.artist, title: tags.title });
        }
    }
    // One play is one play, however many drives its row was copied to.
    history.sort_by_key(|e| e.identity());
    history.dedup_by_key(|e| e.identity());
    report.history = history.len();
    report.files = device.len();
    report.seeded = seed(&mut stats, &history, &device);

    log(&format!("on the player: {} rated, {} counted", report.rated, report.counted));
    log(&format!("history: {} listen(s) in the scrobble log; {} tagged file(s) on the player", report.history, report.files));
    log(&format!(
        "to seed: {} track(s), {} play(s). Left alone: {} already counted. Not placed: {} matched nothing, {} matched more than one file",
        report.seeded.tracks, report.seeded.plays, report.seeded.kept, report.seeded.unmatched, report.seeded.ambiguous
    ));
    if apply && report.seeded.tracks > 0 {
        stats.save(&stats_path).map_err(|e| format!("{}: {e}", stats_path.display()))?;
        report.written = true;
        log(&format!("wrote {}", stats_path.display()));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(artist: &str, track: &str, ts: i64, rating: &str) -> Entry {
        Entry {
            artist: artist.into(),
            album: String::new(),
            track: track.into(),
            track_number: String::new(),
            duration: 200,
            rating: rating.into(),
            timestamp: ts,
            mbid: String::new(),
            raw: String::new(),
        }
    }
    fn track(path: &str, artist: &str, title: &str) -> DeviceTrack {
        DeviceTrack { path: path.into(), artist: artist.into(), title: title.into() }
    }

    /// The two lines are the example in Cinder's `docs/TRACK_DATA.md`.
    const SAMPLE: &str = "#CINDER-STATS/1\n# path\trating\tplays\tlast_played\n\
        /contents/MUSIC/Wunderhorse - Cub/06 - Teal.flac\t5\t12\t1790000000\n\
        /contents_ext/MUSIC/Nick Drake/Pink Moon/01 Pink Moon.flac\t0\t3\t1789950000\n";

    #[test]
    fn the_documented_file_reads_and_writes_back_byte_for_byte() {
        let s = Stats::parse(SAMPLE);
        assert_eq!(s.tracks.len(), 2);
        let teal = s.tracks["/contents/MUSIC/Wunderhorse - Cub/06 - Teal.flac"];
        assert_eq!((teal.rating, teal.plays, teal.last_played), (5, 12, 1790000000));
        assert_eq!(s.render(), SAMPLE);
    }

    #[test]
    fn a_bad_line_costs_only_itself() {
        let body = "#CINDER-STATS/1\nnot a row\n/a.flac\t9\t1\t0\n/b.flac\tx\t1\n/c.flac\t3\t2\n/d.flac\t0\t0\t5\n";
        let s = Stats::parse(body);
        assert_eq!(s.tracks.keys().collect::<Vec<_>>(), ["/c.flac"], "rating 9, a non-number and an empty row are skipped");
        assert_eq!(s.tracks["/c.flac"].last_played, 0, "the fourth column is optional");
    }

    #[test]
    fn windows_line_ends_do_not_end_up_in_a_number() {
        let s = Stats::parse("#CINDER-STATS/1\r\n/a.flac\t4\t2\t17\r\n");
        assert_eq!(s.tracks["/a.flac"], Stat { rating: 4, plays: 2, last_played: 17 });
    }

    #[test]
    fn player_paths_name_the_volume() {
        let root = Path::new("/mnt/walkman");
        let file = root.join("MUSIC").join("A").join("01.flac");
        assert_eq!(player_path(root, &file, false).as_deref(), Some("/contents/MUSIC/A/01.flac"));
        assert_eq!(player_path(root, &file, true).as_deref(), Some("/contents_ext/MUSIC/A/01.flac"));
        assert_eq!(player_path(Path::new("/elsewhere"), &file, false), None);
    }

    #[test]
    fn seeding_counts_listens_and_takes_the_newest_time() {
        let mut s = Stats::default();
        let history = [
            play("Wunderhorse", "Teal", 100, "L"),
            play("wunderhorse", "TEAL", 300, "L"),
            play("Wunderhorse", "Teal", 200, "S"), // a skip is not a play
            play("Nobody", "Nothing", 50, "L"),
        ];
        let device = [track("/contents/MUSIC/Teal.flac", "Wunderhorse", "Teal")];
        let r = seed(&mut s, &history, &device);
        assert_eq!(s.tracks["/contents/MUSIC/Teal.flac"], Stat { rating: 0, plays: 2, last_played: 300 });
        assert_eq!(r, Seeded { tracks: 1, plays: 2, unmatched: 1, ambiguous: 0, kept: 0 });
    }

    #[test]
    fn seeding_never_touches_a_count_or_a_rating_the_player_has() {
        let mut s = Stats::default();
        s.tracks.insert("/a.flac".into(), Stat { rating: 4, plays: 7, last_played: 900 });
        s.tracks.insert("/b.flac".into(), Stat { rating: 5, plays: 0, last_played: 0 });
        let history = [play("A", "Song", 10, "L"), play("B", "Song", 20, "L")];
        let device = [track("/a.flac", "A", "Song"), track("/b.flac", "B", "Song")];
        let r = seed(&mut s, &history, &device);
        assert_eq!(s.tracks["/a.flac"], Stat { rating: 4, plays: 7, last_played: 900 }, "counted already: untouched");
        assert_eq!(s.tracks["/b.flac"], Stat { rating: 5, plays: 1, last_played: 20 }, "rated, never counted: seeded, rating kept");
        assert_eq!((r.tracks, r.kept), (1, 1));
    }

    #[test]
    fn two_files_with_one_artist_and_title_are_not_guessed_at() {
        let mut s = Stats::default();
        let history = [play("A", "Song", 10, "L")];
        let device = [track("/album/01.flac", "A", "Song"), track("/best of/09.flac", "A", "Song")];
        let r = seed(&mut s, &history, &device);
        assert!(s.tracks.is_empty());
        assert_eq!(r.ambiguous, 1);
    }

    /// The whole run over a drive with a log and no music: nothing to seed, nothing written, and a
    /// drive with no MUSIC folder is not an error.
    #[test]
    fn a_drive_with_no_music_seeds_nothing_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("flint-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".scrobbler.log"), "#AUDIOSCROBBLER/1.1\n#TZ/UNKNOWN\n#CLIENT/x\nA\tAl\tSong\t1\t200\tL\t100\t\n").unwrap();
        let mut lines = Vec::new();
        let r = seed_player(&[dir.clone()], true, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!((r.history, r.files, r.seeded.unmatched, r.written), (1, 0, 1, false));
        assert!(!dir.join(FILE_NAME).exists());
        assert!(seed_player(&[], true, &mut |_| {}).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn seeding_twice_changes_nothing_the_second_time() {
        let mut s = Stats::default();
        let history = [play("A", "Song", 10, "L")];
        let device = [track("/a.flac", "A", "Song")];
        seed(&mut s, &history, &device);
        let once = s.clone();
        let r = seed(&mut s, &history, &device);
        assert_eq!(s, once);
        assert_eq!((r.tracks, r.kept), (0, 1));
    }
}
