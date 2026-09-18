//! Analysis Sony's Music Center has already done.
//!
//! Flint exists because running Sony's engine over a library takes an hour and writing its result
//! into the PC's own files costs a megabyte a track. Someone who already runs Music Center has paid
//! the first cost — and this module is how they avoid paying it twice. There are two places that
//! work can be found, and neither needs the engine, FFmpeg, or a single decode:
//!
//! 1. **Inside the music file.** Music Center writes the analysis into the file itself, as the FLAC
//!    `SMFM` application block or the MP3 `USR_SMFMF` GEOB frame — the same containers Flint writes
//!    (Cinder `analysis/RE_sensme_musiccenter.md` §7, §10). [`in_file`] reads it back out. This is
//!    also what makes Flint safe to run over a library Music Center has touched: without it, every
//!    such track would be decoded and analysed again to produce a result the file already held.
//! 2. **Music Center's own cache**, `fringe\audio\<id>\smfmf.bin` under `%APPDATA%\Sony\Music
//!    Center` (§8). Music Center only commits the tag to a file when a Gracenote match is adopted,
//!    so for most libraries the cache holds analysis the files do not. [`import`] maps those blobs
//!    back to the tracks they came from.
//!
//! **What is verified and what is not.** The containers in (1) are verified both ways: Flint writes
//! them and the player reads them (RE §9, §10). The cache layout in (2) is verified — the files are
//! there and they are SMFMF — but the id → file mapping is **not**: it lives in Music Center's NeDB
//! store, whose field names nobody here has confirmed. [`paths_by_id`] therefore does not assume a
//! schema at all; it takes any absolute path to an audio file that appears in the same JSON object
//! as an `_id`, and every adoption is checked twice more: the blob has to parse as SMFMF, and the
//! path has to still exist and still be readable. A wrong guess produces nothing, not a wrong tag.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{flac, id3, smfmf};

/// Does this blob look like the engine's output rather than something else that happened to be in
/// the file? It has to parse as the chunk framing, and hold at least one chunk the player knows.
pub fn plausible(bytes: &[u8]) -> bool {
    match smfmf::parse(bytes) {
        Ok(chunks) => chunks.iter().any(|c| smfmf::KNOWN.contains(&&c.fourcc)),
        Err(_) => false,
    }
}

/// The SensMe analysis already inside `path`, if any. FLAC and MP3; anything else is `None`.
///
/// Whoever wrote it — Music Center, Flint, or a tool nobody here has heard of — the tag is the same
/// bytes the player's scanner parses, so this does not try to tell them apart.
pub fn in_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let found = match ext.as_str() {
        "flac" => {
            let mut f = io::BufReader::new(fs::File::open(path)?);
            match flac::read_layout(&mut f) {
                Ok(layout) => layout.smfm().map(<[u8]>::to_vec),
                Err(_) => None, // not a FLAC after all, or truncated: nothing to adopt
            }
        }
        "mp3" => {
            let mut f = io::BufReader::new(fs::File::open(path)?);
            match id3::read_tag(&mut f) {
                Ok(Some(tag)) => tag.smfmf().map(<[u8]>::to_vec),
                _ => None,
            }
        }
        _ => None,
    };
    Ok(found.filter(|b| plausible(b)))
}

/// Keep only the chunks the Walkman's scanner knows, dropping anything else.
///
/// This is the anti-bloat step, and it is the reason a Music Center library can be put on a player
/// without carrying Music Center's tag sizes with it. The player's parser walks the chunk list and
/// matches names it knows; a chunk it does not know is stepped over and does nothing, so removing
/// one cannot change what the player computes.
///
/// Returns the compacted blob and the names dropped. A blob that is already all-known comes back
/// unchanged, so this is safe to run unconditionally.
pub fn compact(bytes: &[u8]) -> (Vec<u8>, Vec<String>) {
    let Ok(chunks) = smfmf::parse(bytes) else {
        return (bytes.to_vec(), Vec::new());
    };
    let (keep, drop): (Vec<_>, Vec<_>) =
        chunks.into_iter().partition(|c| smfmf::KNOWN.contains(&&c.fourcc));
    if drop.is_empty() {
        return (bytes.to_vec(), Vec::new());
    }
    (smfmf::encode(&keep), drop.iter().map(smfmf::Chunk::name).collect())
}

/// Where Music Center for PC keeps its library and its analysis cache.
pub fn data_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = PathBuf::from(appdata).join("Sony").join("Music Center");
    dir.is_dir().then_some(dir)
}

/// Every cached analysis under `<data_dir>/fringe/audio/<id>/smfmf.bin`, as `(id, blob)`.
///
/// Blobs that do not parse as SMFMF are left out rather than reported: the directory is Music
/// Center's, and a file in it that is not analysis is none of Flint's business.
pub fn cached_analyses(data_dir: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    let audio = data_dir.join("fringe").join("audio");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&audio) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let blob = match fs::read(entry.path().join("smfmf.bin")) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if plausible(&blob) {
            out.push((entry.file_name().to_string_lossy().into_owned(), blob));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The audio extensions a path has to end in to be taken for a track. Wider than Flint's own
/// [`crate::library::EXTENSIONS`] on purpose: this is reading someone else's library, and a path
/// that turns out not to be readable is dropped later anyway.
const TRACK_EXT: [&str; 6] = ["flac", "mp3", "m4a", "wav", "aac", "wma"];

/// `id → file path`, scraped from Music Center's NeDB store (`<data_dir>/db/*.db`, one JSON object
/// per line).
///
/// **The schema is not assumed.** Every line is scanned for an `"_id"` and for any string value
/// that looks like an absolute path to an audio file; the pair is taken only when both are present.
/// That way a field rename in a Music Center update costs nothing, and a wrong guess yields no
/// mapping rather than a wrong one.
pub fn paths_by_id(data_dir: &Path) -> io::Result<HashMap<String, PathBuf>> {
    let mut out = HashMap::new();
    let db = data_dir.join("db");
    let Ok(entries) = fs::read_dir(&db) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("db") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            let Some(id) = json_string(line, "_id") else { continue };
            if let Some(p) = first_track_path(line) {
                out.entry(id).or_insert(p);
            }
        }
    }
    Ok(out)
}

/// The value of `"<key>": "..."` in one line of JSON, unescaped enough for paths (`\\` and `\"`).
fn json_string(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let mut at = 0;
    while let Some(i) = line[at..].find(&needle) {
        let start = at + i + needle.len();
        let rest = line[start..].trim_start();
        if let Some(body) = rest.strip_prefix('"') {
            if let Some(s) = read_json_string(body) {
                return Some(s);
            }
        }
        at = start;
    }
    None
}

/// Read a JSON string body (the opening quote already consumed) up to its closing quote.
fn read_json_string(body: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    // \uXXXX — keep the four digits' character if it is one we can build.
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                other => out.push(other), // covers \\ and \"
            },
            _ => out.push(c),
        }
    }
    None // unterminated
}

/// The first string value in this line that looks like an absolute path to an audio file.
fn first_track_path(line: &str) -> Option<PathBuf> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let body = &line[i + 1..];
            if let Some(s) = read_json_string(body) {
                // Skip the key half of a `"key": "value"` pair by requiring the shape of a path.
                if looks_like_track_path(&s) {
                    return Some(PathBuf::from(s));
                }
                // Advance past this string, escapes included, rather than into the middle of it.
                i += 1 + escaped_len(body);
                continue;
            }
        }
        i += 1;
    }
    None
}

/// How many bytes of `body` the string occupies, up to and including its closing quote.
fn escaped_len(body: &str) -> usize {
    let mut n = 0;
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return i + 1,
            '\\' => {
                chars.next();
            }
            _ => {}
        }
        n = i;
    }
    n
}

fn looks_like_track_path(s: &str) -> bool {
    let windows_drive = s.len() > 3
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.as_bytes()[1] == b':'
        && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/');
    let unc_or_posix = s.starts_with("\\\\") || s.starts_with('/');
    if !(windows_drive || unc_or_posix) {
        return false;
    }
    s.rsplit_once('.')
        .is_some_and(|(_, e)| TRACK_EXT.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// What an import found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Cached analyses Music Center holds.
    pub cached: usize,
    /// …of which a file path could be resolved for.
    pub mapped: usize,
    /// …of which the file was readable and the result went into Flint's cache.
    pub imported: usize,
    /// Already in Flint's cache under the same audio; nothing to do.
    pub already: usize,
    /// Mapped to a path that is no longer there.
    pub missing: usize,
}

/// Import every cached Music Center analysis whose track Flint can still find, into Flint's own
/// cache, keyed by the audio the way every other Flint result is.
///
/// Nothing is written to Music Center's files or to the PC library. The blob is compacted first, so
/// what Flint stores is the part the player reads.
pub fn import(
    data_dir: &Path,
    cache: &mut crate::cache::Cache,
    mut on_each: impl FnMut(&Path, usize, usize),
) -> io::Result<ImportReport> {
    let mut report = ImportReport::default();
    let analyses = cached_analyses(data_dir)?;
    report.cached = analyses.len();
    if analyses.is_empty() {
        return Ok(report);
    }
    let by_id = paths_by_id(data_dir)?;
    for (id, blob) in analyses {
        let Some(path) = by_id.get(&id) else { continue };
        report.mapped += 1;
        let Ok((size, mtime)) = crate::cache::stat(path) else {
            report.missing += 1;
            continue;
        };
        let label = path.to_string_lossy().into_owned();
        let key = match crate::cache::content_key(path) {
            Ok(Some(k)) => k,
            _ => {
                report.missing += 1;
                continue;
            }
        };
        if cache.get(&key).is_some() {
            cache.alias(&key, &label, size, mtime);
            report.already += 1;
            continue;
        }
        let (small, dropped) = compact(&blob);
        let bpm = smfmf::summarise(&small).ok().and_then(|s| s.bpm);
        let entry = crate::cache::Entry {
            key,
            size,
            mtime,
            engine: MUSIC_CENTER.to_string(),
            bpm,
            bytes: small.len(),
            path: label,
        };
        let was = blob.len();
        let now = small.len();
        cache.put(entry, &small)?;
        report.imported += 1;
        let _ = dropped;
        on_each(path, was, now);
    }
    Ok(report)
}

/// Adopt the analysis inside every one of `paths` the cache does not already know, and report how
/// many were taken and how many bytes of unread chunks were left behind.
///
/// This is what makes `flint sync` work for a Music Center user who has never run `flint scan`:
/// their files already carry the analysis, and reading it costs one metadata read each.
pub fn adopt_all(
    paths: &[PathBuf],
    cache: &mut crate::cache::Cache,
    mut on_each: impl FnMut(&Path),
) -> io::Result<(usize, u64)> {
    let mut adopted = 0;
    let mut saved = 0u64;
    for path in paths {
        let Ok((size, mtime)) = crate::cache::stat(path) else { continue };
        let label = path.to_string_lossy().into_owned();
        if cache.fast_key(&label, size, mtime).is_some() {
            continue;
        }
        let key = match crate::cache::content_key(path) {
            Ok(Some(k)) => k,
            _ => continue,
        };
        if cache.get(&key).is_some() {
            cache.alias(&key, &label, size, mtime);
            continue;
        }
        let Some(found) = in_file(path)? else { continue };
        let (small, _) = compact(&found);
        saved += (found.len() - small.len()) as u64;
        let bpm = smfmf::summarise(&small).ok().and_then(|s| s.bpm);
        cache.put(
            crate::cache::Entry {
                key,
                size,
                mtime,
                engine: IN_FILE.to_string(),
                bpm,
                bytes: small.len(),
                path: label,
            },
            &small,
        )?;
        adopted += 1;
        on_each(path);
    }
    Ok((adopted, saved))
}

/// Recorded in the cache's `engine` column for a result Flint did not compute. It is not a version
/// string and is not meant to look like one: the point is that the row can be told apart from
/// Flint's own runs later, when an engine version really does change.
pub const MUSIC_CENTER: &str = "music-center";

/// …and for one lifted out of the file it was already in.
pub const IN_FILE: &str = "in-file";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smfmf::{Chunk, HEADER_LEN};

    fn chunk(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        smfmf::encode(&[Chunk {
            fourcc: *name,
            family: *b"STAE",
            vendor: *b"MMLW",
            version: 0x0100_8000,
            payload,
        }])
    }

    fn engine_result() -> Vec<u8> {
        let mut v = chunk(b"GBPM", &93.44f32.to_be_bytes());
        v.extend(chunk(b"STMM", &[7u8; 64]));
        v
    }

    #[test]
    fn a_blob_is_only_plausible_when_the_player_would_know_a_chunk_in_it() {
        assert!(plausible(&engine_result()));
        assert!(!plausible(b"not smfmf at all"));
        assert!(!plausible(&[]));
        // Right framing, but nothing the scanner reads: not analysis.
        assert!(!plausible(&chunk(b"ZZZZ", &[0u8; 8])));
    }

    /// The whole anti-bloat claim, as a test: a tag padded out with chunks the player does not read
    /// comes back as the part it does, and the part it does is untouched.
    #[test]
    fn compacting_drops_what_the_player_never_reads() {
        let mut fat = engine_result();
        fat.extend(chunk(b"BLOB", &vec![0xAB; 900_000])); // what a megabyte tag looks like
        assert!(fat.len() > 900_000);
        let (small, dropped) = compact(&fat);
        assert_eq!(dropped, vec!["BLOB".to_string()]);
        assert_eq!(small, engine_result(), "the engine's own chunks survive byte for byte");
        assert_eq!(smfmf::summarise(&small).unwrap().bpm, Some(93.44));
        // Already compact: unchanged, and safe to run over anything.
        let (again, none) = compact(&small);
        assert!(none.is_empty());
        assert_eq!(again, small);
        // Not SMFMF at all: handed back rather than emptied.
        let (raw, none) = compact(b"junk");
        assert!(none.is_empty());
        assert_eq!(raw, b"junk");
    }

    /// A FLAC with a real STREAMINFO (so the cache can key on its MD5), a little padding and a few
    /// bytes standing in for audio. Enough for every reader here; nothing decodes it.
    fn write_flac(path: &Path) {
        let mut info = vec![0x11u8; 34];
        info[18..34].copy_from_slice(&[0xA5; 16]); // the decoded-audio MD5 the cache keys on
        let blocks = [
            flac::Block { kind: 0, data: info },
            flac::Block { kind: 1, data: vec![0; 256] },
        ];
        let mut bytes = flac::encode_metadata(&[], &blocks).unwrap();
        bytes.extend_from_slice(&[0xFF, 0xF8, 0x69, 0x18, 0x00, 0x00, 0x00, 0x00]);
        fs::write(path, bytes).unwrap();
    }

    /// An MP3 with an ID3v2.3 tag and one frame header, which is what the readers need to see.
    fn write_mp3(path: &Path) {
        let tag = id3::encode_tag(
            &id3::Tag {
                major: 3,
                revision: 0,
                flags: 0,
                extended: Vec::new(),
                frames: Vec::new(),
                total_len: 0,
            },
            None,
        )
        .unwrap();
        let mut bytes = tag;
        bytes.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // MPEG1 Layer III header
        bytes.extend_from_slice(&[0u8; 400]);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_tag_already_in_a_flac_is_found() {
        let dir = std::env::temp_dir().join(format!("flint-mc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("plain.flac");
        write_flac(&src);
        assert_eq!(in_file(&src).unwrap(), None, "an untagged file has nothing to adopt");

        let tagged = dir.join("tagged.flac");
        flac::copy_with_smfm(&src, &tagged, &engine_result()).unwrap();
        assert_eq!(in_file(&tagged).unwrap(), Some(engine_result()));

        // A file that is not FLAC at all, and one whose extension we do not handle.
        let odd = dir.join("notes.txt");
        fs::write(&odd, b"hello").unwrap();
        assert_eq!(in_file(&odd).unwrap(), None);
        let fake = dir.join("fake.flac");
        fs::write(&fake, b"hello").unwrap();
        assert_eq!(in_file(&fake).unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_tag_already_in_an_mp3_is_found() {
        let dir = std::env::temp_dir().join(format!("flint-mc-mp3-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("plain.mp3");
        write_mp3(&src);
        assert_eq!(in_file(&src).unwrap(), None);
        let tagged = dir.join("tagged.mp3");
        id3::copy_with_smfmf(&src, &tagged, &engine_result()).unwrap();
        assert_eq!(in_file(&tagged).unwrap(), Some(engine_result()));
        fs::remove_dir_all(&dir).ok();
    }

    /// The NeDB scrape takes an `_id` and a path out of a line without knowing the schema, and
    /// takes nothing at all from a line that has only one of them.
    #[test]
    fn the_nedb_scrape_does_not_assume_a_schema() {
        let dir = std::env::temp_dir().join(format!("flint-mc-db-{}", std::process::id()));
        fs::create_dir_all(dir.join("db")).unwrap();
        fs::write(
            dir.join("db").join("tracks.db"),
            concat!(
                r#"{"_id":"aaa","title":"Back in Black","filePath":"C:\\Music\\ACDC\\01 Back in Black.flac"}"#,
                "\n",
                r#"{"_id":"bbb","location":"D:/Music/Air/01 La Femme d'argent.mp3","artist":"Air"}"#,
                "\n",
                r#"{"_id":"ccc","title":"no path here"}"#,
                "\n",
                r#"{"title":"no id here","filePath":"C:\\Music\\x.flac"}"#,
                "\n",
                "not json at all\n",
            ),
        )
        .unwrap();
        let map = paths_by_id(&dir).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map["aaa"], PathBuf::from(r"C:\Music\ACDC\01 Back in Black.flac"));
        assert_eq!(map["bbb"], PathBuf::from("D:/Music/Air/01 La Femme d'argent.mp3"));
        assert!(!map.contains_key("ccc"));
        fs::remove_dir_all(&dir).ok();
    }

    /// The cache walk only returns directories that hold something that parses.
    #[test]
    fn the_cache_walk_ignores_what_is_not_analysis() {
        let dir = std::env::temp_dir().join(format!("flint-mc-cache-{}", std::process::id()));
        let audio = dir.join("fringe").join("audio");
        fs::create_dir_all(audio.join("aaa")).unwrap();
        fs::create_dir_all(audio.join("bbb")).unwrap();
        fs::create_dir_all(audio.join("ccc")).unwrap();
        fs::write(audio.join("aaa").join("smfmf.bin"), engine_result()).unwrap();
        fs::write(audio.join("bbb").join("smfmf.bin"), b"not analysis").unwrap();
        // ccc has no smfmf.bin at all.
        let found = cached_analyses(&dir).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "aaa");
        assert_eq!(found[0].1, engine_result());
        // A directory Music Center has never been near answers empty rather than failing.
        assert!(cached_analyses(Path::new("/nonexistent-music-center")).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    /// End to end, without Music Center: a cached blob, a mapping to a real file, and the result in
    /// Flint's cache keyed by the audio — compacted on the way in.
    #[test]
    fn importing_puts_the_analysis_in_flints_cache_against_the_real_audio() {
        let dir = std::env::temp_dir().join(format!("flint-mc-imp-{}", std::process::id()));
        fs::create_dir_all(dir.join("db")).unwrap();
        let track = dir.join("track.flac");
        write_flac(&track);
        let audio = dir.join("fringe").join("audio").join("xyz");
        fs::create_dir_all(&audio).unwrap();
        let mut fat = engine_result();
        fat.extend(chunk(b"BLOB", &vec![0u8; 50_000]));
        fs::write(audio.join("smfmf.bin"), &fat).unwrap();
        fs::write(
            dir.join("db").join("tracks.db"),
            format!("{{\"_id\":\"xyz\",\"filePath\":\"{}\"}}\n", track.display()),
        )
        .unwrap();

        let mut cache = crate::cache::Cache::open(&dir.join("flint-cache")).unwrap();
        let mut seen = Vec::new();
        let report = import(&dir, &mut cache, |p, was, now| seen.push((p.to_path_buf(), was, now)))
            .unwrap();
        assert_eq!(report.cached, 1);
        assert_eq!(report.mapped, 1);
        assert_eq!(report.imported, 1);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].1 > seen[0].2, "the megabyte went away: {} -> {}", seen[0].1, seen[0].2);

        let key = crate::cache::content_key(&track).unwrap().unwrap();
        let entry = cache.get(&key).expect("in the cache now");
        assert_eq!(entry.engine, MUSIC_CENTER);
        assert_eq!(cache.blob(&key).unwrap(), engine_result(), "compacted, and nothing else");

        // Running it again finds it already there rather than importing twice.
        let report = import(&dir, &mut cache, |_, _, _| {}).unwrap();
        assert_eq!((report.imported, report.already), (0, 1));
        fs::remove_dir_all(&dir).ok();
    }

    /// `flint sync` on a Music Center library, without a `scan`: the tags come out of the files.
    #[test]
    fn adopting_a_whole_library_takes_only_what_is_already_there() {
        let dir = std::env::temp_dir().join(format!("flint-mc-adopt-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain.flac");
        write_flac(&plain);
        let tagged = dir.join("tagged.flac");
        let mut fat = engine_result();
        fat.extend(chunk(b"BLOB", &vec![0u8; 20_000]));
        flac::copy_with_smfm(&plain, &tagged, &fat).unwrap();

        let mut cache = crate::cache::Cache::open(&dir.join("cache")).unwrap();
        let mut seen = Vec::new();
        let (n, saved) =
            adopt_all(&[plain.clone(), tagged.clone()], &mut cache, |p| seen.push(p.to_path_buf()))
                .unwrap();
        assert_eq!(n, 1, "only the file that had one");
        assert_eq!(seen, vec![tagged.clone()]);
        assert!(saved >= 20_000, "the unread chunk was left behind: {saved}");
        let key = crate::cache::content_key(&tagged).unwrap().unwrap();
        assert_eq!(cache.blob(&key).unwrap(), engine_result());
        // Idempotent: a second run has nothing left to take.
        let (again, _) = adopt_all(&[plain, tagged], &mut cache, |_| {}).unwrap();
        assert_eq!(again, 0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_chunk_header_is_twenty_bytes() {
        assert_eq!(HEADER_LEN, 20);
    }
}
