//! Liked songs, kept in step between the player and Last.fm.
//!
//! The Walkman has no WiFi, so it cannot talk to Last.fm itself. Cinder writes what it knows to
//! two files at the root of the player's storage and reads one back:
//!
//! ```text
//! cinder_liked.conf         object ids — the real store, and meaningless off the device
//! cinder_loved.tsv          artist ⇥ title — Cinder EXPORTS this whenever the liked set changes
//! cinder_liked_import.tsv   artist ⇥ title — Flint WRITES this; Cinder merges it and renames .done
//! MUSIC/Liked Songs.m3u8    the playable list, per volume
//! ```
//!
//! **Why this needs a memory.** A track that is on Last.fm and not on the player is ambiguous: it
//! was either just loved on Last.fm (push it to the player) or just unliked on the player (unlove
//! it on Last.fm). Set arithmetic over the two current lists cannot tell those apart, and guessing
//! deletes a hand-curated list. So each side is compared against **its own** snapshot from the last
//! run, and the difference — not the state — is what moves.
//!
//! Three rules fall out of that, and each one exists because the alternative destroys data:
//!
//! * **the first run is additive**: with no snapshots, everything on both sides is kept and nothing
//!   is removed anywhere;
//! * **a source that is not there this run proves nothing**: an unplugged player is not evidence
//!   that every one of its likes was removed;
//! * **while an import is pending** — Flint wrote `cinder_liked_import.tsv` and Cinder has not
//!   consumed it yet — the player is additive only. Its export still shows the pre-push list, and
//!   reading that as removals would push the difference straight back out as unloves.
//!
//! Matching is by artist and title, normalised the same way Cinder normalises in
//! `player/cinder-ffi/src/likes.rs`. The two sides have to agree or nothing lines up, so the rules
//! here are deliberately the same ones: case, curly punctuation, `feat.` credits and re-issue
//! suffixes fold away, and anything that marks a *different recording* — live, remix, acoustic,
//! demo — stays distinct.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// What Cinder exports.
pub const LOVED_TSV: &str = "cinder_loved.tsv";
/// What Flint writes back.
pub const IMPORT_TSV: &str = "cinder_liked_import.tsv";
/// The playlist, inside the player's music folder.
pub const PLAYLIST: &str = "Liked Songs.m3u8";

const IMPORT_HEADER: &str = "# artist\ttitle — the liked list, pushed from the PC by Flint.\n\
                             # Cinder merges this into cinder_liked.conf on the next start and renames it .done.\n";

/// One track, in whatever spelling the source that reported it used.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Track {
    pub artist: String,
    pub title: String,
}

impl Track {
    pub fn new(artist: &str, title: &str) -> Track {
        Track { artist: artist.trim().to_string(), title: title.trim().to_string() }
    }

    pub fn key(&self) -> String {
        keys::key(&self.artist, &self.title)
    }
}

// ── the key: the same folding Cinder does on the device ────────────────────────────────────────

pub mod keys {
    //! Ported from `player/cinder-ffi/src/likes.rs`, line for line where it matters. If these two
    //! ever disagree, a like crosses in one direction and comes back as a different track.

    /// Re-issue markers: the same recording under another release. Deliberately excludes live,
    /// remix, acoustic, demo and instrumental — those are different recordings.
    const NOISE: &[&str] = &[
        "remaster",
        "remastered",
        "explicit",
        "explicit version",
        "clean",
        "clean version",
        "album version",
        "single version",
        "original version",
        "bonus track",
        "deluxe",
        "deluxe edition",
        "expanded",
        "expanded edition",
        "anniversary edition",
        "mono",
        "stereo",
    ];

    const FEAT_MARKERS: &[&str] = &[
        "(feat.",
        "(feat ",
        "[feat.",
        "[feat ",
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        "(ft.",
        "(ft ",
        "(featuring",
        "[featuring",
    ];

    const ARTIST_SPLITS: &[&str] = &[" & ", ", ", "; ", " and ", " vs. ", " vs ", " x ", " / ", "/"];

    fn fold_punctuation(input: &str) -> String {
        input
            .chars()
            .map(|c| match c {
                '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{2032}' => '\'',
                '\u{201C}' | '\u{201D}' => '"',
                '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
                '\u{00A0}' => ' ',
                other => other,
            })
            .collect()
    }

    fn collapse(input: &str) -> String {
        input.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn base(input: &str) -> String {
        collapse(&fold_punctuation(input).to_lowercase())
    }

    fn strip_feat(input: &str) -> String {
        let mut cut = None;
        for marker in FEAT_MARKERS {
            if let Some(position) = input.rfind(marker) {
                cut = Some(match cut {
                    Some(existing) if existing < position => existing,
                    _ => position,
                });
            }
        }
        match cut {
            Some(position) => input[..position].trim_end_matches([' ', '(', '[', '-']).trim().to_string(),
            None => input.to_string(),
        }
    }

    fn is_noise(tail: &str) -> bool {
        let tail = tail.trim().trim_matches(['(', ')', '[', ']']).trim();
        if tail.is_empty() {
            return false;
        }
        if NOISE.contains(&tail) {
            return true;
        }
        // Year-anchored only, so a bare "club mix" — a different recording — is never stripped.
        let words: Vec<&str> = tail.split(' ').collect();
        let has_year = words.iter().any(|w| w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()));
        if !has_year {
            return false;
        }
        words.iter().any(|w| w.starts_with("remaster") || *w == "mix" || *w == "mixes")
    }

    fn strip_noise(input: &str) -> String {
        let trimmed = input.trim_end();
        if trimmed.ends_with(')') || trimmed.ends_with(']') {
            let open = if trimmed.ends_with(')') { '(' } else { '[' };
            if let Some(position) = trimmed.rfind(open) {
                if is_noise(&trimmed[position..]) {
                    return trimmed[..position].trim().to_string();
                }
            }
        }
        if let Some(position) = trimmed.rfind(" - ") {
            if is_noise(&trimmed[position + 3..]) {
                return trimmed[..position].trim().to_string();
            }
        }
        trimmed.to_string()
    }

    pub fn norm_title(value: &str) -> String {
        let mut text = base(value);
        for _ in 0..3 {
            let stripped = strip_noise(&strip_feat(&text));
            if stripped == text || stripped.is_empty() {
                break;
            }
            text = stripped;
        }
        text
    }

    pub fn norm_artist(value: &str) -> String {
        strip_feat(&base(value)).trim().to_string()
    }

    /// First credited artist — the fallback when a collaboration is spelled differently on each
    /// side. Used only after the exact key misses, because it also merges genuinely different
    /// collaborations.
    pub fn primary_artist(value: &str) -> String {
        let normalised = norm_artist(value);
        let mut cut = normalised.len();
        for separator in ARTIST_SPLITS {
            if let Some(position) = normalised.find(separator) {
                if position < cut {
                    cut = position;
                }
            }
        }
        normalised[..cut].trim().to_string()
    }

    /// The join key. `\u{241F}` (unit separator) cannot occur in a tag.
    pub fn key(artist: &str, title: &str) -> String {
        format!("{}\u{241F}{}", norm_artist(artist), norm_title(title))
    }

    /// Second-pass key: primary artist only. Never stored, only used for matching.
    pub fn loose_key(artist: &str, title: &str) -> String {
        format!("{}\u{241F}{}", primary_artist(artist), norm_title(title))
    }
}

// ── what one side looks like right now ─────────────────────────────────────────────────────────

/// One participant in the sync, as it is this run.
#[derive(Clone, Debug, Default)]
pub struct Source {
    pub name: String,
    /// False when it could not be read at all — an unplugged player, a Last.fm that would not
    /// answer. Absent is not empty, and it never causes a removal.
    pub available: bool,
    /// True when it is here but not trusted to prove a removal (a pending import, a first run).
    pub additive_only: bool,
    pub tracks: BTreeMap<String, Track>,
    pub note: String,
}

impl Source {
    pub fn from_tracks(name: &str, tracks: impl IntoIterator<Item = Track>) -> Source {
        let mut map = BTreeMap::new();
        for track in tracks {
            map.insert(track.key(), track);
        }
        Source { name: name.to_string(), available: true, additive_only: false, tracks: map, note: String::new() }
    }

    pub fn missing(name: &str, why: &str) -> Source {
        Source { name: name.to_string(), available: false, note: why.to_string(), ..Source::default() }
    }
}

/// What each side looked like at the end of the last successful run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub last_sync: i64,
    /// Source name → its keys and the spelling it used.
    pub snapshots: BTreeMap<String, BTreeMap<String, Track>>,
    /// The merged truth after that run.
    pub liked: BTreeMap<String, Track>,
}

impl State {
    pub fn path() -> PathBuf {
        crate::cache::default_dir().join("likes-state.tsv")
    }

    pub fn load(path: &Path) -> State {
        std::fs::read_to_string(path).map(|body| State::parse(&body)).unwrap_or_default()
    }

    /// A flat, line-based format on purpose: this is the only record of a hand-curated list, and a
    /// person has to be able to read it — and repair it — with nothing but a text editor.
    pub fn parse(body: &str) -> State {
        let mut state = State::default();
        let mut section: Option<String> = None;
        for line in body.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('\t') {
                let Some((artist, title)) = rest.split_once('\t') else { continue };
                let track = Track::new(artist, title);
                match section.as_deref() {
                    Some("liked") => {
                        state.liked.insert(track.key(), track);
                    }
                    Some(name) => {
                        state.snapshots.entry(name.to_string()).or_default().insert(track.key(), track);
                    }
                    None => {}
                }
                continue;
            }
            let mut parts = line.split('\t');
            match parts.next() {
                Some("last_sync") => state.last_sync = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                Some("snapshot") => {
                    let name = parts.next().unwrap_or("").to_string();
                    state.snapshots.entry(name.clone()).or_default();
                    section = Some(name);
                }
                Some("liked") => section = Some("liked".to_string()),
                _ => {}
            }
        }
        state
    }

    pub fn render(&self) -> String {
        let mut out = String::from(
            "# Flint's likes state: what each side looked like after the last successful run.\n\
             # Delete it to start over — the next run is then purely additive and removes nothing.\n",
        );
        out.push_str(&format!("last_sync\t{}\n", self.last_sync));
        for (name, tracks) in &self.snapshots {
            out.push_str(&format!("snapshot\t{name}\n"));
            for track in tracks.values() {
                out.push_str(&format!("\t{}\t{}\n", track.artist, track.title));
            }
        }
        out.push_str("liked\n");
        for track in self.liked.values() {
            out.push_str(&format!("\t{}\t{}\n", track.artist, track.title));
        }
        out
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tsv.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, path)
    }
}

/// What a conflict means: the same track added on one side and removed on the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Conflict {
    /// Keep the like. Losing a hand-curated like to an ambiguity is worse than keeping one that
    /// was meant to go.
    #[default]
    KeepLiked,
    /// Let the removal win.
    KeepRemoved,
}

/// What the run would do, before it does any of it.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// The merged list: what both sides should hold when this is done.
    pub liked: BTreeMap<String, Track>,
    pub device_add: Vec<Track>,
    pub device_remove: Vec<Track>,
    pub lastfm_love: Vec<Track>,
    pub lastfm_unlove: Vec<Track>,
    /// Anything worth saying out loud: a source that was absent, a first run, a pending import.
    pub notes: Vec<String>,
    /// True when no snapshot existed for a source, so nothing may be removed from it this run.
    pub first_run: bool,
}

impl Plan {
    pub fn changes(&self) -> usize {
        self.device_add.len() + self.device_remove.len() + self.lastfm_love.len() + self.lastfm_unlove.len()
    }
}

/// Work out what should move. Pure: it reads nothing and writes nothing.
pub fn plan(state: &State, device: &Source, lastfm: &Source, conflict: Conflict) -> Plan {
    let mut out = Plan::default();
    let sources = [device, lastfm];

    let mut added: BTreeSet<String> = BTreeSet::new();
    let mut removed: BTreeSet<String> = BTreeSet::new();
    let mut spelling: BTreeMap<String, Track> = state.liked.clone();

    for source in sources {
        if !source.available {
            if !source.note.is_empty() {
                out.notes.push(format!("{}: {}", source.name, source.note));
            } else {
                out.notes.push(format!("{}: not available — nothing read, nothing removed", source.name));
            }
            continue;
        }
        for (key, track) in &source.tracks {
            spelling.entry(key.clone()).or_insert_with(|| track.clone());
        }
        let snapshot = state.snapshots.get(&source.name);
        let Some(snapshot) = snapshot else {
            // No memory of this source: everything it holds is an addition and it cannot prove a
            // removal. This is the first run, and the first run never deletes.
            out.first_run = true;
            added.extend(source.tracks.keys().cloned());
            out.notes.push(format!("{}: first run — additive only", source.name));
            continue;
        };
        added.extend(source.tracks.keys().filter(|k| !snapshot.contains_key(*k)).cloned());
        if source.additive_only {
            if !source.note.is_empty() {
                out.notes.push(format!("{}: {}", source.name, source.note));
            }
            continue;
        }
        removed.extend(snapshot.keys().filter(|k| !source.tracks.contains_key(*k)).cloned());
    }

    // A key that was added on one side and removed on the other is the ambiguous case the
    // snapshots exist to narrow, and it can still happen when both changed since the last run.
    let conflicted: Vec<String> = added.intersection(&removed).cloned().collect();
    for key in &conflicted {
        match conflict {
            Conflict::KeepLiked => {
                removed.remove(key);
            }
            Conflict::KeepRemoved => {
                added.remove(key);
            }
        }
    }
    if !conflicted.is_empty() {
        out.notes.push(format!(
            "{} track(s) changed on both sides — {}",
            conflicted.len(),
            match conflict {
                Conflict::KeepLiked => "kept liked",
                Conflict::KeepRemoved => "removed",
            }
        ));
    }

    let mut liked: BTreeMap<String, Track> = state.liked.clone();
    for key in &added {
        if let Some(track) = spelling.get(key) {
            liked.insert(key.clone(), track.clone());
        }
    }
    for key in &removed {
        liked.remove(key);
    }
    // A source that is here and trusted also proves what it still holds: anything it has that the
    // merged list lost to someone else's removal is not resurrected, but anything it has that was
    // never in the state at all has already been added above.
    out.liked = liked;

    if device.available {
        for (key, track) in &out.liked {
            if !device.tracks.contains_key(key) {
                out.device_add.push(track.clone());
            }
        }
        for (key, track) in &device.tracks {
            if !out.liked.contains_key(key) {
                out.device_remove.push(track.clone());
            }
        }
    }
    if lastfm.available {
        for (key, track) in &out.liked {
            if !lastfm.tracks.contains_key(key) {
                out.lastfm_love.push(track.clone());
            }
        }
        for (key, track) in &lastfm.tracks {
            if !out.liked.contains_key(key) {
                out.lastfm_unlove.push(track.clone());
            }
        }
    }
    out
}

// ── the player's files ─────────────────────────────────────────────────────────────────────────

/// One volume of the player: the drive root, as Windows sees it.
#[derive(Clone, Debug)]
pub struct Volume {
    pub root: PathBuf,
    pub label: String,
}

impl Volume {
    pub fn new(root: impl Into<PathBuf>, label: &str) -> Volume {
        Volume { root: root.into(), label: label.to_string() }
    }

    pub fn music_dir(&self) -> PathBuf {
        for name in ["MUSIC", "Music"] {
            let candidate = self.root.join(name);
            if candidate.is_dir() {
                return candidate;
            }
        }
        self.root.join("MUSIC")
    }

    pub fn loved_path(&self) -> PathBuf {
        self.root.join(LOVED_TSV)
    }

    pub fn import_path(&self) -> PathBuf {
        self.root.join(IMPORT_TSV)
    }

    pub fn playlist_path(&self) -> PathBuf {
        self.music_dir().join(PLAYLIST)
    }

    pub fn present(&self) -> bool {
        self.root.is_dir()
    }

    /// True while an import written by an earlier run is still sitting there unconsumed. Cinder
    /// renames it `.done` once it has merged it, so its presence means the device's own export is
    /// still the pre-push list.
    pub fn import_pending(&self) -> bool {
        self.import_path().is_file()
    }
}

/// Read `cinder_loved.tsv`. A missing file is an empty list, not an error — a player with no likes
/// yet and a player that was never plugged in must not look the same, and that is what
/// [`Volume::present`] is for.
pub fn read_loved(volume: &Volume) -> Vec<Track> {
    let mut out = Vec::new();
    for candidate in [volume.loved_path(), volume.music_dir().join(LOVED_TSV)] {
        let Ok(body) = std::fs::read_to_string(&candidate) else { continue };
        for line in body.lines() {
            let line = line.trim_start_matches('\u{feff}');
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let Some((artist, title)) = line.split_once('\t') else { continue };
            let track = Track::new(artist, title.split('\t').next().unwrap_or(title));
            if !track.artist.is_empty() && !track.title.is_empty() {
                out.push(track);
            }
        }
        break;
    }
    out
}

/// Write the whole liked list for Cinder to merge — the full list, never a delta. The device is
/// the one participant that can be wiped and reindexed, so it has to be able to rebuild from this
/// one file, and a delta would make an unlike impossible to express.
pub fn write_import(volume: &Volume, tracks: &[Track]) -> std::io::Result<()> {
    let mut body = String::from(IMPORT_HEADER);
    for track in tracks {
        body.push_str(&format!("{}\t{}\n", track.artist, track.title));
    }
    atomic_write(&volume.import_path(), &body)
}

/// `Liked Songs.m3u8` in this volume's music folder, holding only this volume's tracks: a playlist
/// row naming a file on the other volume is a dead row on this one.
pub fn write_playlist(volume: &Volume, relative_paths: &[String]) -> std::io::Result<()> {
    let mut body = String::from("#EXTM3U\n");
    for path in relative_paths {
        body.push_str(&path.replace('\\', "/"));
        body.push('\n');
    }
    atomic_write(&volume.playlist_path(), &body)
}

/// Every track on this volume, keyed the way the liked list is keyed, with the path the playlist
/// needs — relative to the music folder, which is what Sony's indexer accepts.
///
/// This reads the tags of every file on the volume, over USB, which is the slowest thing Flint
/// does per track. It is only needed for the playlist: the hearts on the device come from the
/// import file and need no paths at all. `on_file` is called as it goes so a caller can say so.
pub fn index_volume(volume: &Volume, mut on_file: impl FnMut(usize)) -> BTreeMap<String, String> {
    let music = volume.music_dir();
    let mut found = BTreeMap::new();
    let Ok(files) = crate::library::walk(&music) else { return found };
    for (seen, path) in files.iter().enumerate() {
        on_file(seen);
        let Some(tags) = crate::tags::read(path) else { continue };
        let Ok(relative) = path.strip_prefix(&music) else { continue };
        let key = keys::key(&tags.artist, &tags.title);
        found.entry(key).or_insert_with(|| relative.to_string_lossy().replace('\\', "/"));
    }
    found
}

/// The rows of `Liked Songs.m3u8` for one volume: the liked tracks this volume actually holds, in
/// the order the liked list is in. A liked track that lives on the other volume is left out —
/// a playlist row naming a file that is not there is a dead row.
pub fn playlist_rows(liked: &BTreeMap<String, Track>, index: &BTreeMap<String, String>) -> Vec<String> {
    liked.keys().filter_map(|key| index.get(key).cloned()).collect()
}

/// Temp file, then rename: the player's volume is FAT or exFAT on removable flash that gets pulled
/// out mid-write, and a half-written liked list is worse than none.
fn atomic_write(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        // FAT will not replace an existing name on some setups; remove and retry rather than
        // leaving the new list in a .tmp nobody reads.
        Err(_) => {
            std::fs::remove_file(path).ok();
            std::fs::rename(&tmp, path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str, tracks: &[(&str, &str)]) -> Source {
        Source::from_tracks(name, tracks.iter().map(|(a, t)| Track::new(a, t)))
    }

    fn state_with(device: &[(&str, &str)], lastfm: &[(&str, &str)], liked: &[(&str, &str)]) -> State {
        let map = |rows: &[(&str, &str)]| -> BTreeMap<String, Track> {
            rows.iter().map(|(a, t)| Track::new(a, t)).map(|t| (t.key(), t)).collect()
        };
        State {
            last_sync: 1,
            snapshots: BTreeMap::from([("device".into(), map(device)), ("lastfm".into(), map(lastfm))]),
            liked: map(liked),
        }
    }

    #[test]
    fn the_key_folds_what_cinder_folds() {
        assert_eq!(
            keys::key("Amy Winehouse", "Love Is A Losing Game"),
            keys::key("amy winehouse", "love is a losing game")
        );
        // Re-issue suffixes and feat. credits fold; a different recording does not.
        assert_eq!(keys::key("Bowie", "Heroes (2017 Remaster)"), keys::key("Bowie", "Heroes"));
        assert_eq!(keys::key("Little Simz feat. Cleo Sol", "Woman"), keys::key("Little Simz", "Woman"));
        assert_ne!(keys::key("Bicep", "Glue (Live)"), keys::key("Bicep", "Glue"));
        assert_ne!(keys::key("Bicep", "Glue (Club Mix)"), keys::key("Bicep", "Glue"));
        // Curly punctuation is the commonest tag drift of all.
        assert_eq!(keys::key("Sigur Rós", "Hoppípolla’s"), keys::key("Sigur Rós", "Hoppípolla's"));
        // The loose key only drops the second credited artist.
        assert_eq!(keys::loose_key("A & B", "Song"), keys::loose_key("A", "Song"));
    }

    #[test]
    fn the_first_run_is_additive_in_both_directions() {
        let plan = plan(
            &State::default(),
            &source("device", &[("Bonobo", "Kerala")]),
            &source("lastfm", &[("Bicep", "Atlas")]),
            Conflict::default(),
        );
        assert!(plan.first_run);
        assert_eq!(plan.liked.len(), 2);
        assert_eq!(plan.device_add.len(), 1, "the Last.fm track goes to the player");
        assert_eq!(plan.lastfm_love.len(), 1, "the player's track goes to Last.fm");
        assert!(
            plan.device_remove.is_empty() && plan.lastfm_unlove.is_empty(),
            "nothing is ever removed on a first run"
        );
    }

    #[test]
    fn a_removal_propagates_once_there_is_a_snapshot() {
        // Both knew about Kerala last time; the player no longer has it, so it was unliked there.
        let state = state_with(&[("Bonobo", "Kerala")], &[("Bonobo", "Kerala")], &[("Bonobo", "Kerala")]);
        let plan =
            plan(&state, &source("device", &[]), &source("lastfm", &[("Bonobo", "Kerala")]), Conflict::default());
        assert!(plan.liked.is_empty());
        assert_eq!(plan.lastfm_unlove.len(), 1);
        assert!(plan.device_add.is_empty(), "it must not be pushed back to the player it was removed on");
    }

    #[test]
    fn an_absent_source_removes_nothing() {
        let state = state_with(&[("Bonobo", "Kerala")], &[("Bonobo", "Kerala")], &[("Bonobo", "Kerala")]);
        let plan = plan(
            &state,
            &Source::missing("device", "the player is not plugged in"),
            &source("lastfm", &[("Bonobo", "Kerala")]),
            Conflict::default(),
        );
        assert_eq!(plan.liked.len(), 1, "an unplugged player is not an unlike");
        assert!(plan.lastfm_unlove.is_empty());
        assert!(plan.notes.iter().any(|n| n.contains("not plugged in")));
    }

    #[test]
    fn a_pending_import_makes_the_player_additive_only() {
        // Flint pushed Atlas last run; Cinder has not merged it yet, so the device export still
        // shows the old list. That must not read as "Atlas was unliked".
        let state = state_with(
            &[("Bonobo", "Kerala")],
            &[("Bonobo", "Kerala"), ("Bicep", "Atlas")],
            &[("Bonobo", "Kerala"), ("Bicep", "Atlas")],
        );
        let mut device = source("device", &[("Bonobo", "Kerala")]);
        device.additive_only = true;
        device.note = "an import is still waiting to be merged — additive only".into();
        let plan =
            plan(&state, &device, &source("lastfm", &[("Bonobo", "Kerala"), ("Bicep", "Atlas")]), Conflict::default());
        assert_eq!(plan.liked.len(), 2);
        assert!(plan.lastfm_unlove.is_empty(), "the pending push must not come back as an unlove");
        assert_eq!(plan.device_add.len(), 1, "Atlas is still owed to the player");
    }

    #[test]
    fn both_sides_changing_is_settled_by_the_policy() {
        // Loved on Last.fm since the snapshot, removed on the player since the snapshot.
        let state = state_with(&[("Bicep", "Atlas")], &[], &[("Bicep", "Atlas")]);
        let device = source("device", &[]);
        let lastfm = source("lastfm", &[("Bicep", "Atlas")]);
        let keep = plan(&state, &device, &lastfm, Conflict::KeepLiked);
        assert_eq!(keep.liked.len(), 1);
        assert_eq!(keep.device_add.len(), 1);
        let drop = plan(&state, &device, &lastfm, Conflict::KeepRemoved);
        assert!(drop.liked.is_empty());
        assert_eq!(drop.lastfm_unlove.len(), 1);
    }

    #[test]
    fn a_track_spelled_differently_on_each_side_is_one_track() {
        let plan = plan(
            &State::default(),
            &source("device", &[("Little Simz feat. Cleo Sol", "Woman")]),
            &source("lastfm", &[("Little Simz", "Woman (Explicit)")]),
            Conflict::default(),
        );
        assert_eq!(plan.liked.len(), 1, "one track, two spellings");
        assert!(plan.changes() == 0, "nothing to move — both sides already have it");
    }

    #[test]
    fn state_survives_a_round_trip() {
        let state = state_with(&[("A", "One")], &[("B", "Two")], &[("A", "One"), ("B", "Two")]);
        let back = State::parse(&state.render());
        assert_eq!(back, state);
        // A state file that has been hand-edited down to nothing still reads.
        assert_eq!(State::parse("").snapshots.len(), 0);
    }

    #[test]
    fn the_playlist_holds_only_what_this_volume_has() {
        let liked: BTreeMap<String, Track> =
            [Track::new("Bicep", "Atlas"), Track::new("Bonobo", "Kerala")].into_iter().map(|t| (t.key(), t)).collect();
        // The index knows one of them, spelled differently — the fold has to bridge that.
        let index: BTreeMap<String, String> =
            [(keys::key("Bicep", "Atlas (2021 Remaster)"), "Bicep - Isles/01 Atlas.flac".to_string())]
                .into_iter()
                .collect();
        assert_eq!(playlist_rows(&liked, &index), vec!["Bicep - Isles/01 Atlas.flac".to_string()]);
    }

    #[test]
    fn the_device_files_are_written_whole_and_atomically() {
        let dir = std::env::temp_dir().join(format!("flint-likes-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("MUSIC")).unwrap();
        let volume = Volume::new(&dir, "internal");
        assert!(!volume.import_pending());
        write_import(&volume, &[Track::new("Bonobo", "Kerala")]).unwrap();
        assert!(volume.import_pending());
        let body = std::fs::read_to_string(volume.import_path()).unwrap();
        assert!(body.starts_with("# artist\ttitle"), "Cinder refuses a file without its header");
        assert!(body.contains("Bonobo\tKerala"));
        // The export Cinder writes comes back as tracks.
        std::fs::write(volume.loved_path(), "# artist\ttitle\nBicep\tAtlas\n\nBonobo\tKerala\n").unwrap();
        let loved = read_loved(&volume);
        assert_eq!(loved.len(), 2);
        write_playlist(&volume, &["Bicep - Isles\\01 Atlas.flac".to_string()]).unwrap();
        let playlist = std::fs::read_to_string(volume.playlist_path()).unwrap();
        assert!(playlist.starts_with("#EXTM3U\n"));
        assert!(playlist.contains("Bicep - Isles/01 Atlas.flac"), "separators are forward slashes on the device");
        std::fs::remove_dir_all(&dir).ok();
    }
}
