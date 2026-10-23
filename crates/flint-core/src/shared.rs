//! One song on two albums, stored once (owner idea, 2026-10-23).
//!
//! A track on an album and again on a best-of, or a single and its album, is often the same
//! recording. Flint finds such pairs by their audio ([`suggest`]: the same content key as the
//! analysis cache, so retagging changes nothing), the owner says yes or no to each, and a sync then
//! copies only the file it keeps. The other album's entry goes into [`LIST_NAME`] beside the
//! manifest on the volume holding the kept file, with that entry's own tags, and Cinder lists it
//! under its own album. Sony's player, which does not read the list, shows only the kept copy.
//!
//! The answers live in Flint's cache folder ([`STORE_NAME`]), one line per pair:
//! `library \t keep \t also \t yes|no`, paths relative to that library. A "no" is kept so the pair
//! is not suggested again.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::Path;

use crate::cache::{clean, content_key, mp3_audio_span, write_atomic};
use crate::sync::{album_of, SourceFile};
use crate::tags::Tags;

/// On a volume, beside `flint-manifest.tsv`: the entries Cinder lists that have no file of their own.
pub const LIST_NAME: &str = "flint-shared.tsv";
/// In Flint's cache folder: the owner's answers.
pub const STORE_NAME: &str = "shared.tsv";
const LIST_HEADER: &str = "#FLINT-SHARED/1\talso\tkeep\ttitle\tartist\talbum\talbum_artist\ttrack\tyear\n";

/// Two library files with the same audio: `keep` is copied, `also` is listed under its own album.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pair {
    pub keep: String,
    pub also: String,
}

/// The owner's answers, for every library.
#[derive(Clone, Debug, Default)]
pub struct Answers {
    /// (library, pair) -> yes.
    pub lines: BTreeMap<(String, Pair), bool>,
}

impl Answers {
    pub fn load(dir: &Path) -> Answers {
        let mut a = Answers::default();
        let Ok(text) = fs::read_to_string(dir.join(STORE_NAME)) else { return a };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if let [lib, keep, also, yes] = f[..] {
                let pair = Pair { keep: keep.into(), also: also.into() };
                a.lines.insert((lib.into(), pair), yes == "yes");
            }
        }
        a
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let mut out = String::new();
        for ((lib, p), yes) in &self.lines {
            let yes = if *yes { "yes" } else { "no" };
            out.push_str(&format!("{}\t{}\t{}\t{yes}\n", clean(lib), clean(&p.keep), clean(&p.also)));
        }
        fs::create_dir_all(dir)?;
        write_atomic(&dir.join(STORE_NAME), out.as_bytes())
    }

    /// The pairs said yes to in `library`.
    pub fn confirmed(&self, library: &Path) -> Vec<Pair> {
        let lib = library.to_string_lossy();
        self.lines.iter().filter(|((l, _), yes)| **yes && *l == lib).map(|((_, p), _)| p.clone()).collect()
    }

    /// Has `library`'s owner answered this pair either way?
    pub fn answered(&self, library: &Path, p: &Pair) -> bool {
        self.lines.contains_key(&(library.to_string_lossy().into_owned(), p.clone()))
    }

    pub fn set(&mut self, library: &Path, p: Pair, yes: bool) {
        self.lines.insert((library.to_string_lossy().into_owned(), p), yes);
    }
}

/// Pairs of files in different albums whose audio is the same, oldest-named kept: the first path
/// in a set is kept and every other one in another album is paired with it.
///
/// Cheap first: a FLAC's key is its STREAMINFO MD5 (one metadata read), and an MP3 is hashed only
/// when another MP3 has an audio part of exactly the same length.
// ponytail: the kept copy is the first by path, not the "original" album; a swap option when an
// owner wants the other one on Sony's player.
pub fn suggest(library: &Path, source: &[SourceFile]) -> Vec<Pair> {
    let full = |f: &SourceFile| library.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let mut by_key: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut mp3_by_len: BTreeMap<u64, Vec<&SourceFile>> = BTreeMap::new();
    for f in source.iter().filter(|f| !f.is_sidecar()) {
        let path = full(f);
        if f.rel.to_ascii_lowercase().ends_with(".mp3") {
            let span = File::open(&path).ok().and_then(|h| mp3_audio_span(&mut BufReader::new(h)).ok().flatten());
            if let Some((_, len)) = span {
                mp3_by_len.entry(len).or_default().push(f);
            }
        } else if let Ok(Some(key)) = content_key(&path) {
            by_key.entry(key).or_default().push(&f.rel);
        }
    }
    for f in mp3_by_len.into_values().filter(|v| v.len() > 1).flatten() {
        if let Ok(Some(key)) = content_key(&full(f)) {
            by_key.entry(key).or_default().push(&f.rel);
        }
    }
    let mut out = Vec::new();
    for mut rels in by_key.into_values().filter(|v| v.len() > 1) {
        rels.sort_unstable();
        let keep = rels[0];
        let mut albums = BTreeSet::from([album_of(keep)]);
        for also in &rels[1..] {
            // Two copies in one album are a library problem, not an album to list it under.
            if albums.insert(album_of(also)) {
                out.push(Pair { keep: keep.to_string(), also: also.to_string() });
            }
        }
    }
    out
}

/// The list for one volume: one line per `(also, keep)` entry, with `also`'s own tags.
pub fn list_body(entries: &[(String, String, Tags)]) -> String {
    let mut out = String::from(LIST_HEADER);
    for (also, keep, t) in entries {
        let cols = [also, keep, &t.title, &t.artist, &t.album, &t.album_artist, &t.track, &t.year];
        let cols: Vec<String> = cols.iter().map(|c| clean(c)).collect();
        out.push_str(&cols.join("\t"));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flac_with_md5(md5: u8) -> Vec<u8> {
        let mut b = b"fLaC".to_vec();
        b.push(0x80);
        b.extend_from_slice(&[0, 0, 34]);
        let mut info = [0u8; 34];
        info[18..34].fill(md5);
        b.extend_from_slice(&info);
        b
    }

    #[test]
    fn the_same_audio_in_two_albums_is_suggested_once_per_album() {
        let d = std::env::temp_dir().join(format!("flint-shared-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let files = [
            ("Album/03 Song.flac", 7),
            ("Best Of/11 Song.flac", 7),
            ("Single/01 Song.flac", 7),
            ("Single/02 Song (copy).flac", 7),
            ("Album/04 Other.flac", 8),
        ];
        let mut source = Vec::new();
        for (rel, md5) in files {
            let p = d.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, flac_with_md5(md5)).unwrap();
            source.push(SourceFile { rel: rel.into(), size: 42, mtime: 1 });
        }
        let got = suggest(&d, &source);
        fs::remove_dir_all(&d).unwrap();
        let pair = |a: &str, b: &str| Pair { keep: a.into(), also: b.into() };
        assert_eq!(
            got,
            vec![pair("Album/03 Song.flac", "Best Of/11 Song.flac"), pair("Album/03 Song.flac", "Single/01 Song.flac")]
        );
    }

    #[test]
    fn answers_round_trip_and_belong_to_their_library() {
        let d = std::env::temp_dir().join(format!("flint-answers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let mut a = Answers::default();
        let p = Pair { keep: "A/1.flac".into(), also: "B/1.flac".into() };
        let q = Pair { keep: "A/2.flac".into(), also: "C/2.flac".into() };
        a.set(Path::new("D:/Music"), p.clone(), true);
        a.set(Path::new("D:/Music"), q.clone(), false);
        a.set(Path::new("E:/Other"), q.clone(), true);
        a.save(&d).unwrap();
        let b = Answers::load(&d);
        fs::remove_dir_all(&d).unwrap();
        assert_eq!(b.confirmed(Path::new("D:/Music")), vec![p.clone()]);
        assert!(b.answered(Path::new("D:/Music"), &q), "a no is remembered");
        assert_eq!(b.confirmed(Path::new("E:/Other")), vec![q]);
    }

    /// The list Cinder reads (`contracts/flint-shared.tsv`; Cinder's `cinder-ffi/src/shared.rs` parses
    /// the same file), byte for byte, including a tab in a tag and a name that is not ASCII.
    #[test]
    fn the_list_is_the_one_cinder_reads() {
        let t = |title: &str, artist: &str, album: &str, album_artist: &str, track: &str, year: &str| Tags {
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: album_artist.into(),
            track: track.into(),
            year: year.into(),
        };
        let body = list_body(&[
            (
                "Best Of/11 Atlas.flac".into(),
                "Isles/01 Atlas.flac".into(),
                t("Atlas", "Bicep", "Best Of", "Various Artists", "11/18", "2022"),
            ),
            (
                "Sigur Ros - Takk/04 Hoppipolla.flac".into(),
                "Takk.../04 Hoppipolla.flac".into(),
                t("Hoppípolla", "Sigur Rós", "Takk\t(Live)", "", "4", ""),
            ),
        ]);
        assert_eq!(body, include_str!("../../../contracts/flint-shared.tsv"));
    }

    #[test]
    fn the_list_is_one_clean_line_per_entry() {
        let t = Tags { title: "Song".into(), album: "Best\tOf".into(), track: "11".into(), ..Tags::default() };
        let body = list_body(&[("Best Of/11 Song.flac".into(), "Album/03 Song.flac".into(), t)]);
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines[0].starts_with("#FLINT-SHARED/1"));
        assert_eq!(lines[1].split('\t').count(), 8);
        assert_eq!(lines[1].split('\t').nth(4), Some("Best Of"), "a tab in a tag does not split it");
    }
}
