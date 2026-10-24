//! Tags that will look wrong on the player, found before the copy (audit G3.1).
//!
//! Cinder lists an album by its album name and its album artist, or the track artist where a file
//! has no album artist. So a folder whose files disagree on either shows up as several albums, and
//! a compilation with no album artist as one album per guest. A track with no number plays out of
//! order, and a file with no artist or title is listed under nothing useful. These show only after
//! the copy and the unplug; read from the tags, they can be fixed first.
//!
//! The unit is the sync's album, the folder holding the music ([`crate::sync::album_of`]), so disc
//! folders count with their album.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::sync::{album_of, SourceFile};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// The folder's files make this many albums on the player: (album, artist) as Cinder groups.
    Splits { folder: String, albums: Vec<(String, String)> },
    /// Files in a folder of several tracks with no track number.
    NoTrackNumber { folder: String, files: usize },
    /// A music file whose artist or title could not be read.
    NoTags { file: String },
    /// A FLAC wider than 24 bits. The player's decoder stalls on one, so Cinder skips it.
    TooWide { file: String, bits: u8 },
}

impl Finding {
    pub fn line(&self) -> String {
        match self {
            Finding::Splits { folder, albums } => {
                let names: Vec<String> = albums
                    .iter()
                    .map(|(album, artist)| {
                        format!("\"{album}\" by {}", if artist.is_empty() { "no one" } else { artist })
                    })
                    .collect();
                format!("{folder}: shows as {} albums: {}", albums.len(), names.join(", "))
            }
            Finding::NoTrackNumber { folder, files } => {
                format!("{folder}: {files} track(s) with no number, so out of order")
            }
            Finding::NoTags { file } => format!("{file}: no artist or title"),
            Finding::TooWide { file, bits } => {
                format!("{file}: {bits}-bit, which the player cannot play; convert it to 24-bit")
            }
        }
    }
}

fn is_music(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    l.ends_with(".flac") || l.ends_with(".mp3")
}

/// A FLAC's bits per sample, from its STREAMINFO.
fn flac_bits(path: &Path) -> Option<u8> {
    let mut f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let layout = crate::flac::read_layout(&mut f).ok()?;
    crate::lossless::stream_info(&layout.blocks.first()?.data).map(|i| i.bits)
}

/// Read every FLAC and MP3 in `source` and report what will look wrong. Folders in path order.
pub fn lint(library: &Path, source: &[SourceFile]) -> Vec<Finding> {
    let mut folders: BTreeMap<&str, Vec<&SourceFile>> = BTreeMap::new();
    for f in source.iter().filter(|f| is_music(&f.rel)) {
        folders.entry(album_of(&f.rel)).or_default().push(f);
    }
    let mut out = Vec::new();
    for (folder, files) in folders {
        let mut albums: BTreeSet<(String, String)> = BTreeSet::new();
        let mut shown: Vec<(String, String)> = Vec::new();
        let mut unnumbered = 0;
        let mut no_tags = Vec::new();
        for f in &files {
            let path = library.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(bits) = flac_bits(&path).filter(|&b| b > 24) {
                out.push(Finding::TooWide { file: f.rel.clone(), bits });
            }
            let Some(t) = crate::tags::read(&path) else {
                no_tags.push(f.rel.clone());
                continue;
            };
            let artist = if t.album_artist.trim().is_empty() { t.artist.trim() } else { t.album_artist.trim() };
            if albums.insert((t.album.trim().to_lowercase(), artist.to_lowercase())) {
                shown.push((t.album.trim().to_string(), artist.to_string()));
            }
            unnumbered += usize::from(t.track.split('/').next().unwrap_or("").trim().parse::<u32>().is_err());
        }
        if shown.len() > 1 {
            out.push(Finding::Splits { folder: folder.to_string(), albums: shown });
        }
        if files.len() > 1 && unnumbered > 0 {
            out.push(Finding::NoTrackNumber { folder: folder.to_string(), files: unnumbered });
        }
        out.extend(no_tags.into_iter().map(|file| Finding::NoTags { file }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A FLAC with a VORBIS_COMMENT block holding `fields`, and nothing else.
    fn flac(fields: &[(&str, &str)]) -> Vec<u8> {
        let mut vc = Vec::new();
        vc.extend_from_slice(&0u32.to_le_bytes());
        vc.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        for (k, v) in fields {
            let kv = format!("{k}={v}");
            vc.extend_from_slice(&(kv.len() as u32).to_le_bytes());
            vc.extend_from_slice(kv.as_bytes());
        }
        let mut b = b"fLaC".to_vec();
        b.push(0); // STREAMINFO, not last
        b.extend_from_slice(&[0, 0, 34]);
        b.extend_from_slice(&[0u8; 34]);
        b.push(0x80 | 4); // VORBIS_COMMENT, last
        b.extend_from_slice(&(vc.len() as u32).to_be_bytes()[1..]);
        b.extend_from_slice(&vc);
        b
    }

    #[test]
    fn split_albums_missing_numbers_and_missing_tags_are_found() {
        let d = std::env::temp_dir().join(format!("flint-lint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let t = |artist: &str, album_artist: &str, track: &str| {
            vec![
                ("ARTIST", artist),
                ("TITLE", "Song"),
                ("ALBUM", "Now 99"),
                ("ALBUMARTIST", album_artist),
                ("TRACKNUMBER", track),
            ]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>()
        };
        let files = [
            // A compilation with no album artist: one album per guest.
            ("Now 99/01.flac", t("Blur", "", "1")),
            ("Now 99/02.flac", t("Pulp", "", "2")),
            // The same compilation tagged properly: one album, disc folders included.
            ("Best/CD1/01.flac", t("Blur", "Various Artists", "1")),
            ("Best/CD2/01.flac", t("Pulp", "Various Artists", "1/12")),
            // Numbers missing on one of two.
            ("Album/01.flac", t("Blur", "Blur", "1")),
            ("Album/02.flac", t("Blur", "Blur", "")),
            // A single file needs no number.
            ("Single/01.flac", t("Blur", "Blur", "")),
        ];
        let mut source = Vec::new();
        for (rel, fields) in &files {
            let p = d.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            let pairs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            fs::write(&p, flac(&pairs)).unwrap();
            source.push(SourceFile { rel: rel.to_string(), size: 1, mtime: 1 });
        }
        fs::write(d.join("Album/03.flac"), flac(&[("ALBUM", "Album")])).unwrap();
        source.push(SourceFile { rel: "Album/03.flac".into(), size: 1, mtime: 1 });
        // 44.1 kHz, two channels, 32 bits: rate in bytes 10-12, then channels-1 and bits-1.
        let mut wide = flac(&t("Blur", "Blur", "1").iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>());
        wide[18..22].copy_from_slice(&[0x0a, 0xc4, 0x43, 0xf0]);
        fs::create_dir_all(d.join("Wide")).unwrap();
        fs::write(d.join("Wide/01.flac"), wide).unwrap();
        source.push(SourceFile { rel: "Wide/01.flac".into(), size: 1, mtime: 1 });
        source.push(SourceFile { rel: "Album/cover.jpg".into(), size: 1, mtime: 1 });
        let got = lint(&d, &source);
        fs::remove_dir_all(&d).unwrap();
        assert_eq!(
            got,
            vec![
                Finding::NoTrackNumber { folder: "Album".into(), files: 1 },
                Finding::NoTags { file: "Album/03.flac".into() },
                Finding::Splits {
                    folder: "Now 99".into(),
                    albums: vec![("Now 99".into(), "Blur".into()), ("Now 99".into(), "Pulp".into())]
                },
                Finding::TooWide { file: "Wide/01.flac".into(), bits: 32 },
            ]
        );
        assert_eq!(got[2].line(), "Now 99: shows as 2 albums: \"Now 99\" by Blur, \"Now 99\" by Pulp");
    }
}
