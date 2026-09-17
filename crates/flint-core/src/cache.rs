//! The analysis cache: each track's SMFMF result, kept by Flint and never inside the music files.
//!
//! Layout, under the cache directory:
//!
//! ```text
//! index.tsv              key \t size \t mtime \t engine \t bpm \t bytes \t path
//! blobs/<key>.smfmf      the engine's result for that audio
//! ```
//!
//! **The key is the audio, not the file.** A FLAC's STREAMINFO already holds the MD5 of its decoded
//! samples, so its key costs one metadata read; an MP3's is a hash of the bytes between its ID3v2 tag
//! and any ID3v1 tag. Retagging, renaming or moving a track therefore finds its old result. The
//! `(path, size, mtime)` columns are only the fast path that avoids reading an MP3 twice.
//!
//! The index is rewritten whole, through a temporary file and a rename, so an interrupted scan
//! leaves the previous index intact; blobs are written the same way before their line is added.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::{flac, id3};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    pub size: u64,
    pub mtime: i64,
    pub engine: String,
    pub bpm: Option<f32>,
    pub bytes: usize,
    pub path: String,
}

pub struct Cache {
    dir: PathBuf,
    by_key: HashMap<String, Entry>,
    by_path: HashMap<String, String>,
}

/// Where Flint keeps its data when not told otherwise.
pub fn default_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("Flint");
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("flint");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share/flint")
}

/// 64-bit FNV-1a. Not cryptographic; it only has to tell a few thousand tracks apart.
fn fnv1a(mut r: impl Read, mut len: u64) -> io::Result<u64> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut buf = vec![0u8; 1 << 16];
    while len > 0 {
        let want = buf.len().min(usize::try_from(len).unwrap_or(usize::MAX));
        let n = r.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        len -= n as u64;
    }
    Ok(h)
}

/// The audio's identity, or `None` for a file Flint does not handle.
pub fn content_key(path: &Path) -> io::Result<Option<String>> {
    let mut f = BufReader::new(File::open(path)?);
    if let Ok(layout) = flac::read_layout(&mut f) {
        let info = &layout.blocks[0].data;
        if info.len() == 34 {
            let md5 = &info[18..34];
            // 36-bit total-samples count: the low 4 bits of byte 13, then bytes 14..18.
            let samples =
                (u64::from(info[13] & 0x0f) << 32) | u64::from(u32::from_be_bytes(info[14..18].try_into().expect("4")));
            if md5.iter().any(|&b| b != 0) {
                let hex: String = md5.iter().map(|b| format!("{b:02x}")).collect();
                return Ok(Some(format!("flac-{hex}-{samples}")));
            }
        }
        // No MD5 recorded by the encoder: hash the frames instead.
        let len = f.get_ref().metadata()?.len().saturating_sub(layout.audio_offset);
        f.seek(SeekFrom::Start(layout.audio_offset))?;
        return Ok(Some(format!("flacraw-{:016x}-{len}", fnv1a(&mut f, len)?)));
    }
    let start = match id3::read_tag(&mut f) {
        Ok(Some(tag)) => tag.total_len as u64,
        Ok(None) => 0,
        Err(_) => return Ok(None),
    };
    let total = f.get_ref().metadata()?.len();
    let mut end = total;
    if total >= start + 128 {
        f.seek(SeekFrom::Start(total - 128))?;
        let mut tag = [0u8; 3];
        f.read_exact(&mut tag)?;
        if &tag == b"TAG" {
            end -= 128;
        }
    }
    f.seek(SeekFrom::Start(start))?;
    let mut head = [0u8; 2];
    if f.read_exact(&mut head).is_err() || !(head[0] == 0xff && head[1] & 0xe0 == 0xe0) {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(start))?;
    let len = end - start;
    Ok(Some(format!("mp3-{:016x}-{len}", fnv1a(&mut f, len)?)))
}

pub fn stat(path: &Path) -> io::Result<(u64, i64)> {
    let m = fs::metadata(path)?;
    let mtime = m.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    Ok((m.len(), mtime))
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("partial");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

pub(crate) fn clean(field: &str) -> String {
    field.replace(['\t', '\n', '\r'], " ")
}

impl Cache {
    pub fn open(dir: &Path) -> io::Result<Cache> {
        fs::create_dir_all(dir.join("blobs"))?;
        let mut c = Cache { dir: dir.to_path_buf(), by_key: HashMap::new(), by_path: HashMap::new() };
        let index = dir.join("index.tsv");
        if index.exists() {
            for line in BufReader::new(File::open(index)?).lines() {
                let line = line?;
                let f: Vec<&str> = line.splitn(7, '\t').collect();
                if f.len() != 7 {
                    continue;
                }
                let (Ok(size), Ok(mtime), Ok(bytes)) = (f[1].parse(), f[2].parse(), f[5].parse()) else { continue };
                let e = Entry {
                    key: f[0].to_string(),
                    size,
                    mtime,
                    engine: f[3].to_string(),
                    bpm: f[4].parse().ok(),
                    bytes,
                    path: f[6].to_string(),
                };
                if c.blob_path(&e.key).exists() {
                    c.by_path.insert(e.path.clone(), e.key.clone());
                    c.by_key.insert(e.key.clone(), e);
                }
            }
        }
        Ok(c)
    }

    fn blob_path(&self, key: &str) -> PathBuf {
        self.dir.join("blobs").join(format!("{key}.smfmf"))
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// The key for `path` if its size and mtime still match what the cache recorded.
    pub fn fast_key(&self, path: &str, size: u64, mtime: i64) -> Option<&str> {
        let key = self.by_path.get(path)?;
        let e = self.by_key.get(key)?;
        (e.size == size && e.mtime == mtime).then_some(key.as_str())
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.by_key.get(key)
    }

    pub fn blob(&self, key: &str) -> io::Result<Vec<u8>> {
        fs::read(self.blob_path(key))
    }

    /// Record a result. The blob is on disk before this returns; call `save` to persist the index.
    pub fn put(&mut self, entry: Entry, smfmf: &[u8]) -> io::Result<()> {
        write_atomic(&self.blob_path(&entry.key), smfmf)?;
        self.by_path.insert(entry.path.clone(), entry.key.clone());
        self.by_key.insert(entry.key.clone(), entry);
        Ok(())
    }

    /// Point `path` at an already-cached key (the same audio seen under a new name).
    pub fn alias(&mut self, key: &str, path: &str, size: u64, mtime: i64) {
        if let Some(e) = self.by_key.get_mut(key) {
            e.path = path.to_string();
            e.size = size;
            e.mtime = mtime;
            self.by_path.insert(path.to_string(), key.to_string());
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let mut entries: Vec<&Entry> = self.by_key.values().collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut out = String::new();
        for e in entries {
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                e.key,
                e.size,
                e.mtime,
                clean(&e.engine),
                e.bpm.map(|b| format!("{b:.2}")).unwrap_or_default(),
                e.bytes,
                clean(&e.path)
            ));
        }
        write_atomic(&self.dir.join("index.tsv"), out.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flint-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn flac_with(md5: [u8; 16], samples: u64, extra: &[flac::Block]) -> Vec<u8> {
        let mut info = vec![0u8; 34];
        info[13] = ((samples >> 32) & 0x0f) as u8;
        info[14..18].copy_from_slice(&(samples as u32).to_be_bytes());
        info[18..34].copy_from_slice(&md5);
        let mut blocks = vec![flac::Block { kind: 0, data: info }];
        blocks.extend_from_slice(extra);
        let mut f = flac::encode_metadata(&[], &blocks).unwrap();
        f.extend_from_slice(b"\xff\xf8frames");
        f
    }

    #[test]
    fn a_flac_key_ignores_tags_and_follows_the_audio() {
        let d = tmpdir("flac");
        let a = d.join("a.flac");
        let b = d.join("b.flac");
        fs::write(&a, flac_with([7; 16], 0x1_2345_6789, &[])).unwrap();
        let comment = flac::Block { kind: 4, data: b"retagged".to_vec() };
        fs::write(&b, flac_with([7; 16], 0x1_2345_6789, &[comment])).unwrap();
        let ka = content_key(&a).unwrap().unwrap();
        assert_eq!(ka, format!("flac-{}-{}", "07".repeat(16), 0x1_2345_6789u64));
        assert_eq!(content_key(&b).unwrap().unwrap(), ka, "a retag changed the key");
        fs::write(&b, flac_with([8; 16], 0x1_2345_6789, &[])).unwrap();
        assert_ne!(content_key(&b).unwrap().unwrap(), ka);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_mp3_key_ignores_id3v2_and_id3v1() {
        let d = tmpdir("mp3");
        let audio = b"\xff\xfb\x90\x64some-mpeg-frames".to_vec();
        let plain = d.join("plain.mp3");
        fs::write(&plain, &audio).unwrap();
        let tagged = d.join("tagged.mp3");
        id3::copy_with_smfmf(&plain, &tagged, b"x").unwrap();
        let mut with_v1 = fs::read(&tagged).unwrap();
        let mut v1 = b"TAG".to_vec();
        v1.resize(128, b' ');
        with_v1.extend_from_slice(&v1);
        let v1_path = d.join("v1.mp3");
        fs::write(&v1_path, with_v1).unwrap();
        let k = content_key(&plain).unwrap().unwrap();
        assert!(k.starts_with("mp3-"));
        assert_eq!(content_key(&tagged).unwrap().unwrap(), k);
        assert_eq!(content_key(&v1_path).unwrap().unwrap(), k);
        fs::write(d.join("junk.mp3"), b"not audio at all").unwrap();
        assert_eq!(content_key(&d.join("junk.mp3")).unwrap(), None);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn entries_survive_a_reopen_and_the_fast_path_checks_size_and_mtime() {
        let d = tmpdir("index");
        let mut c = Cache::open(&d).unwrap();
        let e = Entry {
            key: "flac-ab-1".into(),
            size: 100,
            mtime: 42,
            engine: "1.1.2.643".into(),
            bpm: Some(93.44),
            bytes: 3,
            path: r"C:\Music\A\01 tab	name.flac".into(),
        };
        c.put(e.clone(), b"abc").unwrap();
        c.save().unwrap();
        let c2 = Cache::open(&d).unwrap();
        let path = r"C:\Music\A\01 tab name.flac";
        assert_eq!(c2.len(), 1);
        assert_eq!(c2.fast_key(path, 100, 42), Some("flac-ab-1"));
        assert_eq!(c2.fast_key(path, 101, 42), None);
        assert_eq!(c2.fast_key(path, 100, 43), None);
        assert_eq!(c2.blob("flac-ab-1").unwrap(), b"abc");
        assert_eq!(c2.get("flac-ab-1").unwrap().bpm, Some(93.44));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_index_line_without_its_blob_is_dropped() {
        let d = tmpdir("orphan");
        fs::write(d.join("index.tsv"), "flac-zz-1\t1\t1\te\t\t1\tC:\\x.flac\nbroken line\n").unwrap();
        assert!(Cache::open(&d).unwrap().is_empty());
        fs::remove_dir_all(&d).unwrap();
    }
}
