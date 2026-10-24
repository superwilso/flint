//! Synced lyrics from LRCLIB (G1.4).
//!
//! LRCLIB (lrclib.net) is a free lyrics database that needs no key. Flint asks it about each track
//! that has no lyrics Cinder can show as they are sung, and writes the answer as `Song.lrc` beside
//! `Song.flac` in the library: where Cinder, Sony's player and MusicBee all look, and where a sync
//! already carries lyrics to the player ([`crate::sync::SIDECAR_EXT`]). A `.lrc` is never
//! overwritten. Tracks LRCLIB does not know are remembered ([`NONE_NAME`]) so the next run does not
//! ask again.
//!
//! What is asked about: FLAC and MP3 with an artist and a title, no `.lrc` beside them, and no
//! synced lyrics in their tags. A track whose tags hold plain lyrics takes synced ones only, since
//! a `.lrc` wins over the tags in Cinder and plain text would only replace plain text.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::sync::SourceFile;
use crate::tags::{Lyrics, Tags};

/// In Flint's cache folder: `library \t track` for every track LRCLIB had nothing for.
pub const NONE_NAME: &str = "lyrics-none.tsv";
const API: &str = "https://lrclib.net/api/get";

/// A track to ask about.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub rel: String,
    pub tags: Tags,
    /// From a FLAC's STREAMINFO. LRCLIB matches it within two seconds, which tells a live or
    /// radio edit from the album take; an MP3 is asked without one.
    pub seconds: Option<u32>,
}

/// What LRCLIB said.
#[derive(Debug, PartialEq)]
pub enum Answer {
    /// The text to write, synced when LRCLIB has it.
    Lyrics(String),
    /// Not there, instrumental, or only plain text for a track that already has plain text.
    Nothing,
    /// Busy (503, 429): LRCLIB asks for a retry "in a moment". [`ask`] waits and asks again.
    Busy(String),
    /// Failing: stop and try another time. Nothing is remembered.
    Stop(String),
}

/// `Song.flac` → `Song.lrc`.
pub fn lrc_rel(rel: &str) -> String {
    match rel.rfind('.').filter(|&dot| !rel[dot..].contains('/')) {
        Some(dot) => format!("{}.lrc", &rel[..dot]),
        None => format!("{rel}.lrc"),
    }
}

/// The tracks to ask about, in library order, less those already answered "nothing".
pub fn wanted(library: &Path, source: &[SourceFile], nothing: &BTreeSet<String>) -> Vec<Track> {
    // Any spelling of the extension counts, as it does for Cinder (`Song.LRC`).
    let lrcs: BTreeSet<String> =
        source.iter().filter(|f| f.rel.to_ascii_lowercase().ends_with(".lrc")).map(|f| f.rel.to_lowercase()).collect();
    let mut out = Vec::new();
    for f in source.iter().filter(|f| !f.is_sidecar()) {
        if nothing.contains(&f.rel) || lrcs.contains(&lrc_rel(&f.rel).to_lowercase()) {
            continue;
        }
        let path = library.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let Some(tags) = crate::tags::read(&path) else { continue };
        if tags.lyrics == Lyrics::Synced {
            continue;
        }
        out.push(Track { rel: f.rel.clone(), seconds: flac_seconds(&path), tags });
    }
    out
}

fn flac_seconds(path: &Path) -> Option<u32> {
    let mut f = io::BufReader::new(fs::File::open(path).ok()?);
    let layout = crate::flac::read_layout(&mut f).ok()?;
    let info = crate::lossless::stream_info(&layout.blocks.first()?.data)?;
    u32::try_from(info.samples.checked_div(u64::from(info.rate))?).ok().filter(|&s| s > 0)
}

/// The query for one track. Album and length narrow the match when the tags have them.
pub fn query(t: &Track) -> Vec<(String, String)> {
    let mut q =
        vec![("artist_name".to_string(), t.tags.artist.clone()), ("track_name".to_string(), t.tags.title.clone())];
    if !t.tags.album.is_empty() {
        q.push(("album_name".into(), t.tags.album.clone()));
    }
    if let Some(s) = t.seconds {
        q.push(("duration".into(), s.to_string()));
    }
    q
}

/// Read LRCLIB's reply. `has_plain`: the track's tags already hold plain lyrics.
pub fn answer(status: u16, body: &str, has_plain: bool) -> Answer {
    match status {
        200 if body.contains("\"instrumental\":true") => Answer::Nothing,
        200 => {
            let words = |key| crate::musiccenter::json_string(body, key).filter(|s| !s.trim().is_empty());
            match words("syncedLyrics") {
                Some(text) => Answer::Lyrics(text),
                None if has_plain => Answer::Nothing,
                None => words("plainLyrics").map_or(Answer::Nothing, Answer::Lyrics),
            }
        }
        404 => Answer::Nothing,
        _ => {
            let why = crate::musiccenter::json_string(body, "message").unwrap_or_else(|| format!("HTTP {status}"));
            let why = format!("LRCLIB: {why}");
            if matches!(status, 429 | 503) {
                Answer::Busy(why)
            } else {
                Answer::Stop(why)
            }
        }
    }
}

/// Ask LRCLIB about one track, waiting out a busy server for up to 50 seconds before giving up.
pub fn ask(t: &Track) -> Answer {
    let once = || match crate::http::get_form(API, &query(t)) {
        Ok(r) => answer(r.status, &r.body, t.tags.lyrics == Lyrics::Plain),
        Err(e) => Answer::Stop(e.to_string()),
    };
    for wait in [5, 15, 30] {
        match once() {
            Answer::Busy(_) => std::thread::sleep(std::time::Duration::from_secs(wait)),
            a => return a,
        }
    }
    match once() {
        Answer::Busy(why) => Answer::Stop(why),
        a => a,
    }
}

/// Write `Song.lrc` beside the track. `false` when one is already there: it is never replaced.
pub fn write(library: &Path, rel: &str, text: &str) -> io::Result<bool> {
    let path = library.join(lrc_rel(rel).replace('/', std::path::MAIN_SEPARATOR_STR));
    let mut f = match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut text = text.replace("\r\n", "\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    f.write_all(text.as_bytes())?;
    Ok(true)
}

/// The tracks of `library` LRCLIB had nothing for.
pub fn nothing_for(cache: &Path, library: &Path) -> BTreeSet<String> {
    let lib = library.to_string_lossy();
    let text = fs::read_to_string(cache.join(NONE_NAME)).unwrap_or_default();
    text.lines().filter_map(|l| l.split_once('\t')).filter(|(l, _)| *l == lib).map(|(_, rel)| rel.to_string()).collect()
}

/// Remember that LRCLIB had nothing for these tracks.
pub fn remember_nothing(cache: &Path, library: &Path, rels: &[String]) -> io::Result<()> {
    if rels.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(cache)?;
    let mut f = fs::OpenOptions::new().create(true).append(true).open(cache.join(NONE_NAME))?;
    let lib = crate::cache::clean(&library.to_string_lossy());
    for rel in rels {
        writeln!(f, "{lib}\t{}", crate::cache::clean(rel))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Replies copied from lrclib.net on 2026-10-24, the lyrics cut to their first two lines.
    const FOUND: &str = r#"{"id":496,"name":"Creep","trackName":"Creep","artistName":"Radiohead","albumName":"Pablo Honey","duration":239.0,"instrumental":false,"hasWordSync":false,"plainLyrics":"When you were here before\nCouldn't look you in the eye\n","syncedLyrics":"[00:19.16] When you were here before\n[00:24.09] Couldn't look you in the eye\n","lyricsfile":"version: '1.0'\n"}"#;
    const NOT_FOUND: &str = r#"{"message":"Failed to find specified track","name":"TrackNotFound","statusCode":404}"#;
    const BUSY: &str =
        r#"{"message":"The server is busy, please retry in a moment","name":"ServerOverloaded","statusCode":503}"#;

    #[test]
    fn lrclib_replies_read_as_what_to_do() {
        let synced = "[00:19.16] When you were here before\n[00:24.09] Couldn't look you in the eye\n";
        assert_eq!(answer(200, FOUND, false), Answer::Lyrics(synced.into()));
        assert_eq!(answer(200, FOUND, true), Answer::Lyrics(synced.into()));
        let plain_only = FOUND.replace(
            r#""syncedLyrics":"[00:19.16] When you were here before\n[00:24.09] Couldn't look you in the eye\n""#,
            r#""syncedLyrics":null"#,
        );
        assert_eq!(
            answer(200, &plain_only, false),
            Answer::Lyrics("When you were here before\nCouldn't look you in the eye\n".into())
        );
        assert_eq!(answer(200, &plain_only, true), Answer::Nothing, "plain text does not replace plain text");
        let instrumental = FOUND.replace(r#""instrumental":false"#, r#""instrumental":true"#);
        assert_eq!(answer(200, &instrumental, false), Answer::Nothing);
        assert_eq!(answer(404, NOT_FOUND, false), Answer::Nothing);
        assert_eq!(
            answer(503, BUSY, false),
            Answer::Busy("LRCLIB: The server is busy, please retry in a moment".into())
        );
        assert_eq!(answer(502, "<html>", false), Answer::Stop("LRCLIB: HTTP 502".into()));
    }

    #[test]
    fn the_query_names_what_the_tags_have() {
        let tags = Tags { artist: "Radiohead".into(), title: "Creep".into(), ..Tags::default() };
        let t = Track { rel: "a.mp3".into(), tags: tags.clone(), seconds: None };
        let keys = |q: Vec<(String, String)>| q.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(keys(query(&t)), ["artist_name", "track_name"]);
        let t = Track { tags: Tags { album: "Pablo Honey".into(), ..tags }, seconds: Some(238), ..t };
        assert_eq!(query(&t)[2..], [("album_name".into(), "Pablo Honey".into()), ("duration".into(), "238".into())]);
    }

    /// Which tracks are asked about, and that a `.lrc` is written once and never replaced.
    #[test]
    fn tracks_with_lyrics_are_left_alone_and_a_lrc_is_never_replaced() {
        let d = std::env::temp_dir().join(format!("flint-lyrics-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let lib = d.join("lib");
        fs::create_dir_all(lib.join("A")).unwrap();
        let flac = |comments: &[&str], seconds: u32| {
            let mut info = vec![0u8; 34];
            info[10..13].copy_from_slice(&[0x0a, 0xc4, 0x40]); // 44100 Hz
            info[14..18].copy_from_slice(&(seconds * 44100).to_be_bytes());
            let mut vc = 0u32.to_le_bytes().to_vec();
            vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
            for c in comments {
                vc.extend_from_slice(&(c.len() as u32).to_le_bytes());
                vc.extend_from_slice(c.as_bytes());
            }
            let blocks = [crate::flac::Block { kind: 0, data: info }, crate::flac::Block { kind: 4, data: vc }];
            crate::flac::encode_metadata(&[], &blocks).unwrap()
        };
        fs::write(lib.join("A/01 Plain.flac"), flac(&["ARTIST=X", "TITLE=One", "LYRICS=words"], 200)).unwrap();
        fs::write(lib.join("A/02 Synced.flac"), flac(&["ARTIST=X", "TITLE=Two", "LYRICS=[00:01.00]w"], 200)).unwrap();
        fs::write(lib.join("A/03 Has.flac"), flac(&["ARTIST=X", "TITLE=Three"], 200)).unwrap();
        fs::write(lib.join("A/03 Has.LRC"), "[00:01.00]mine\n").unwrap();
        fs::write(lib.join("A/04 Untagged.flac"), flac(&[], 200)).unwrap();
        fs::write(lib.join("A/05 Asked.flac"), flac(&["ARTIST=X", "TITLE=Five"], 200)).unwrap();
        fs::write(lib.join("A/06 None.flac"), flac(&["ARTIST=X", "TITLE=Six"], 61)).unwrap();
        let source = crate::sync::scan_library(&lib).unwrap();
        let cache = d.join("cache");
        remember_nothing(&cache, &lib, &["A/05 Asked.flac".into()]).unwrap();
        let nothing = nothing_for(&cache, &lib);
        assert!(nothing_for(&cache, &d.join("other")).is_empty(), "remembered per library");
        let got: Vec<(String, Option<u32>, Lyrics)> =
            wanted(&lib, &source, &nothing).into_iter().map(|t| (t.rel, t.seconds, t.tags.lyrics)).collect();
        assert_eq!(
            got,
            [("A/01 Plain.flac".into(), Some(200), Lyrics::Plain), ("A/06 None.flac".into(), Some(61), Lyrics::None)]
        );

        assert!(write(&lib, "A/06 None.flac", "[00:01.00] a\r\n[00:02.00] b").unwrap());
        assert_eq!(fs::read_to_string(lib.join("A/06 None.lrc")).unwrap(), "[00:01.00] a\n[00:02.00] b\n");
        assert!(!write(&lib, "A/06 None.flac", "other").unwrap(), "never replaced");
        assert_eq!(fs::read_to_string(lib.join("A/06 None.lrc")).unwrap(), "[00:01.00] a\n[00:02.00] b\n");
        assert_eq!(lrc_rel("A.b/Song"), "A.b/Song.lrc");
        let _ = fs::remove_dir_all(&d);
    }
}
