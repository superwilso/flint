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
//! Three things live here. Reading and writing the file. **Seeding**: the player starts every count
//! at zero and never reads its own scrobble log, so the first counts come from the history Flint
//! already has ([`seed`]). And **following**: a row is keyed by where the file is, so when a sync
//! moves an album between internal memory and the card its ratings would stay behind under a path
//! nothing has any more ([`follow`]).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::likes::keys;
use crate::scrobblelog::Entry;

pub const FILE_NAME: &str = "cinder_stats.tsv";
const HEADER: &str = "#CINDER-STATS/1";
/// How the player names its two volumes.
pub const INTERNAL: &str = "/contents";
pub const CARD: &str = "/contents_ext";

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

/// An album's rating as the player shows it: the mean of its RATED tracks, rounded to whole stars
/// (half up), or `None` when none of them is rated. One five-star track among nine unrated ones
/// reads five.
pub fn album_rating(ratings: impl IntoIterator<Item = u8>) -> Option<u8> {
    let (sum, n) = ratings.into_iter().filter(|r| *r > 0).fold((0u32, 0u32), |(s, n), r| (s + u32::from(r), n + 1));
    (n > 0).then(|| ((sum * 2 + n) / (n * 2)).clamp(1, 5) as u8)
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

/// A player path as (is it on the SD card, the path below the volume). `None` for anything that
/// names neither volume. The card is asked first: `/contents_ext` starts with `/contents`.
pub(crate) fn split_volume(path: &str) -> Option<(bool, &str)> {
    let below = |prefix: &str| path.strip_prefix(prefix).filter(|rest| rest.starts_with('/'));
    below(CARD).map(|rest| (true, rest)).or_else(|| below(INTERNAL).map(|rest| (false, rest)))
}

/// Where the PC sees a file the player names. `roots` are the player's volumes as the PC sees
/// them, internal memory first and the SD card after it. `None` when the path names a volume that
/// was not given: not being able to look is not the same as the file being gone.
pub fn pc_path(roots: &[PathBuf], player: &str) -> Option<PathBuf> {
    let (card, rest) = split_volume(player)?;
    let root = roots.get(usize::from(card))?;
    Some(rest.split('/').filter(|part| !part.is_empty()).fold(root.clone(), |p, part| p.join(part)))
}

/// The drive a sync destination is on. Flint is given the folder the albums go in; when that is
/// the player's `MUSIC` folder the drive is one up, and otherwise it is the folder itself.
pub fn drive_root(volume_root: &Path) -> PathBuf {
    let is_music = volume_root.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("music"));
    match volume_root.parent() {
        Some(parent) if is_music => parent.to_path_buf(),
        _ => volume_root.to_path_buf(),
    }
}

/// What [`follow`] did, for the screen to say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Followed {
    /// Rows that moved to the volume their file is on now.
    pub moved: usize,
    /// Of those, rows that joined one the player had already started at the new place.
    pub joined: usize,
    /// Rows whose file is on neither volume. Kept: if the album comes back, so does its rating.
    pub missing: usize,
    /// Rows that could not be checked because their volume, or the other one, was not there.
    pub unchecked: usize,
}

/// Move each row whose file has gone from the volume it names, and is on the other one, to where
/// the file is now.
///
/// `on_player` answers for one player path: `Some(true)` the file is there, `Some(false)` it is
/// not, `None` that volume cannot be looked at. **Only a positive sighting moves a row**: a card
/// that is not plugged in costs nothing, and neither does a file that is on neither volume.
///
/// When the player already has a row at the new place (it played the moved copy before Flint
/// caught up), the two are one track's history: the plays add, the later date wins, and the
/// rating is the new row's when it has one. Running it twice moves nothing the second time.
pub fn follow(stats: &mut Stats, on_player: impl Fn(&str) -> Option<bool>) -> Followed {
    let mut report = Followed::default();
    let paths: Vec<String> = stats.tracks.keys().cloned().collect();
    for path in paths {
        let Some((card, rest)) = split_volume(&path) else { continue };
        match on_player(&path) {
            Some(true) => continue,
            None => {
                report.unchecked += 1;
                continue;
            }
            Some(false) => {}
        }
        let twin = format!("{}{rest}", if card { INTERNAL } else { CARD });
        match on_player(&twin) {
            Some(true) => {}
            Some(false) => {
                report.missing += 1;
                continue;
            }
            None => {
                report.unchecked += 1;
                continue;
            }
        }
        let Some(old) = stats.tracks.remove(&path) else { continue };
        report.moved += 1;
        match stats.tracks.get_mut(&twin) {
            Some(there) => {
                report.joined += 1;
                if there.rating == 0 {
                    there.rating = old.rating;
                }
                there.plays = there.plays.saturating_add(old.plays);
                there.last_played = there.last_played.max(old.last_played);
            }
            None => {
                stats.tracks.insert(twin, old);
            }
        }
    }
    report
}

/// [`follow`] over the player's drives: read `cinder_stats.tsv` from the first, look for each
/// row's file on the drives given, and write the file back when `apply` is set and a row moved.
/// A drive with no stats file has nothing to follow.
pub fn follow_player(roots: &[PathBuf], apply: bool, log: &mut dyn FnMut(&str)) -> Result<Followed, String> {
    let Some(internal) = roots.first() else {
        return Err("no player drive given".into());
    };
    let stats_path = internal.join(FILE_NAME);
    if !stats_path.is_file() {
        return Ok(Followed::default());
    }
    let mut stats = Stats::load(&stats_path);
    let report = follow(&mut stats, |player| pc_path(roots, player).map(|pc| pc.is_file()));
    if report.moved > 0 {
        log(&format!(
            "ratings and play counts: {} track(s) moved between internal memory and the card{}",
            report.moved,
            if apply { "; their history went with them" } else { "; their history would go with them" }
        ));
        if apply {
            stats.save(&stats_path).map_err(|e| format!("{}: {e}", stats_path.display()))?;
        }
    }
    if report.missing > 0 {
        log(&format!(
            "ratings and play counts: {} track(s) are no longer on the player. Their history is kept in case they come back",
            report.missing
        ));
    }
    Ok(report)
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
pub fn seed_player(
    roots: &[std::path::PathBuf],
    apply: bool,
    log: &mut dyn FnMut(&str),
) -> Result<PlayerReport, String> {
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
    log(&format!(
        "history: {} listen(s) in the scrobble log; {} tagged file(s) on the player",
        report.history, report.files
    ));
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
        assert_eq!(
            s.tracks.keys().collect::<Vec<_>>(),
            ["/c.flac"],
            "rating 9, a non-number and an empty row are skipped"
        );
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
        assert_eq!(
            s.tracks["/b.flac"],
            Stat { rating: 5, plays: 1, last_played: 20 },
            "rated, never counted: seeded, rating kept"
        );
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
        std::fs::write(
            dir.join(".scrobbler.log"),
            "#AUDIOSCROBBLER/1.1\n#TZ/UNKNOWN\n#CLIENT/x\nA\tAl\tSong\t1\t200\tL\t100\t\n",
        )
        .unwrap();
        let mut lines = Vec::new();
        let r = seed_player(std::slice::from_ref(&dir), true, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!((r.history, r.files, r.seeded.unmatched, r.written), (1, 0, 1, false));
        assert!(!dir.join(FILE_NAME).exists());
        assert!(seed_player(&[], true, &mut |_| {}).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn stats_of(rows: &[(&str, u8, u32, i64)]) -> Stats {
        let mut s = Stats::default();
        for (path, rating, plays, last_played) in rows {
            s.tracks.insert((*path).into(), Stat { rating: *rating, plays: *plays, last_played: *last_played });
        }
        s
    }

    /// An album Flint moved to the card: the row goes with it, and the one that stayed is untouched.
    #[test]
    fn a_rating_follows_its_file_to_the_other_volume() {
        let mut s = stats_of(&[("/contents/MUSIC/A/01.flac", 5, 12, 900), ("/contents/MUSIC/B/01.flac", 3, 1, 100)]);
        let on = ["/contents_ext/MUSIC/A/01.flac", "/contents/MUSIC/B/01.flac"];
        let r = follow(&mut s, |p| Some(on.contains(&p)));
        assert_eq!(r, Followed { moved: 1, joined: 0, missing: 0, unchecked: 0 });
        assert_eq!(
            s,
            stats_of(&[("/contents_ext/MUSIC/A/01.flac", 5, 12, 900), ("/contents/MUSIC/B/01.flac", 3, 1, 100)])
        );
        // And back again, card to internal memory.
        let on = ["/contents/MUSIC/A/01.flac", "/contents/MUSIC/B/01.flac"];
        assert_eq!(follow(&mut s, |p| Some(on.contains(&p))).moved, 1);
        assert!(s.tracks.contains_key("/contents/MUSIC/A/01.flac"));
        // Nothing is left to move.
        assert_eq!(follow(&mut s, |p| Some(on.contains(&p))), Followed::default());
    }

    /// The player played the moved copy before Flint caught up, so there are two rows for one
    /// track: the plays add, the later date wins, and a rating given at the new place stands.
    #[test]
    fn two_rows_for_one_moved_track_become_one() {
        let on = ["/contents_ext/MUSIC/A/01.flac"];
        let mut s =
            stats_of(&[("/contents/MUSIC/A/01.flac", 5, 10, 500), ("/contents_ext/MUSIC/A/01.flac", 0, 3, 900)]);
        let r = follow(&mut s, |p| Some(on.contains(&p)));
        assert_eq!((r.moved, r.joined), (1, 1));
        assert_eq!(s, stats_of(&[("/contents_ext/MUSIC/A/01.flac", 5, 13, 900)]));
        let mut s =
            stats_of(&[("/contents/MUSIC/A/01.flac", 5, 10, 500), ("/contents_ext/MUSIC/A/01.flac", 2, 3, 100)]);
        follow(&mut s, |p| Some(on.contains(&p)));
        assert_eq!(s, stats_of(&[("/contents_ext/MUSIC/A/01.flac", 2, 13, 500)]), "the newer rating stands");
    }

    /// Not being able to look is not the file being gone: with the card out, nothing moves and
    /// nothing is dropped, and a track on neither volume keeps its row.
    #[test]
    fn a_missing_card_or_a_removed_album_loses_nothing() {
        let rows = [
            ("/contents/MUSIC/A/01.flac", 4, 2, 10),
            ("/contents_ext/MUSIC/C/01.flac", 5, 1, 20),
            ("/other/x.flac", 1, 1, 1),
        ];
        let mut s = stats_of(&rows);
        // Internal memory only: A is gone from it, the card cannot be asked.
        let r = follow(&mut s, |p| if p.starts_with("/contents_ext/") { None } else { Some(false) });
        assert_eq!(r, Followed { moved: 0, joined: 0, missing: 0, unchecked: 2 });
        assert_eq!(s, stats_of(&rows));
        // Both volumes there, the files on neither.
        let r = follow(&mut s, |_| Some(false));
        assert_eq!(r, Followed { moved: 0, joined: 0, missing: 2, unchecked: 0 });
        assert_eq!(s, stats_of(&rows), "a path that names neither volume is not ours to judge either");
    }

    #[test]
    fn a_player_path_maps_to_the_drive_it_is_on() {
        let roots = [PathBuf::from("/mnt/int"), PathBuf::from("/mnt/sd")];
        assert_eq!(pc_path(&roots, "/contents/MUSIC/A/01.flac"), Some(PathBuf::from("/mnt/int/MUSIC/A/01.flac")));
        assert_eq!(pc_path(&roots, "/contents_ext/MUSIC/A/01.flac"), Some(PathBuf::from("/mnt/sd/MUSIC/A/01.flac")));
        assert_eq!(pc_path(&roots[..1], "/contents_ext/MUSIC/A/01.flac"), None, "the card was not given");
        assert_eq!(pc_path(&roots, "/contents_extra/x.flac"), None, "a longer name is not the card");
        assert_eq!(pc_path(&roots, "/elsewhere/x.flac"), None);
        assert_eq!(drive_root(Path::new("/mnt/int/MUSIC")), PathBuf::from("/mnt/int"));
        assert_eq!(drive_root(Path::new("/mnt/int/Music")), PathBuf::from("/mnt/int"));
        assert_eq!(drive_root(Path::new("/mnt/int")), PathBuf::from("/mnt/int"));
    }

    /// The whole run over two real folders: the album is on the card now, the file says internal.
    #[test]
    fn following_over_real_drives_rewrites_the_file_only_when_asked() {
        let dir = std::env::temp_dir().join(format!("flint-follow-{}", std::process::id()));
        let (int, sd) = (dir.join("int"), dir.join("sd"));
        std::fs::create_dir_all(sd.join("MUSIC").join("A")).unwrap();
        std::fs::create_dir_all(int.join("MUSIC")).unwrap();
        std::fs::write(sd.join("MUSIC").join("A").join("01.flac"), b"x").unwrap();
        let before = "#CINDER-STATS/1\n# path\trating\tplays\tlast_played\n/contents/MUSIC/A/01.flac\t5\t12\t900\n";
        std::fs::write(int.join(FILE_NAME), before).unwrap();
        let roots = [int.clone(), sd.clone()];
        let mut lines = Vec::new();
        let dry = follow_player(&roots, false, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!(dry.moved, 1);
        assert_eq!(std::fs::read_to_string(int.join(FILE_NAME)).unwrap(), before, "a dry run wrote the file");
        assert!(lines[0].contains("would go with them"), "{lines:?}");
        follow_player(&roots, true, &mut |_| {}).unwrap();
        let after = std::fs::read_to_string(int.join(FILE_NAME)).unwrap();
        assert!(
            after.contains("/contents_ext/MUSIC/A/01.flac\t5\t12\t900\n") && !after.contains("/contents/MUSIC/A"),
            "{after}"
        );
        // A drive with no stats file has nothing to follow, and gains no file.
        assert_eq!(follow_player(std::slice::from_ref(&sd), true, &mut |_| {}).unwrap(), Followed::default());
        assert!(!sd.join(FILE_NAME).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_albums_rating_is_the_mean_of_its_rated_tracks() {
        assert_eq!(album_rating([0, 0, 0]), None);
        assert_eq!(album_rating([5, 0, 0, 0]), Some(5), "unrated tracks do not pull it down");
        assert_eq!(album_rating([4, 5]), Some(5), "4.5 rounds up");
        assert_eq!(album_rating([4, 4, 5]), Some(4));
        assert_eq!(album_rating([1, 2]), Some(2));
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
