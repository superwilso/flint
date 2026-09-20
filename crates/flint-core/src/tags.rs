//! Artist and title, read back out of a file.
//!
//! Flint's sync works in albums and folders and never needs to know what a track is called — but
//! the liked-songs playlist does: `Liked Songs.m3u8` has to name the files on the player that match
//! the artist-and-title pairs the sync deals in, and nothing but the tags can say which those are.
//!
//! Two containers, two fields, and nothing else is read: FLAC's VORBIS_COMMENT and MP3's ID3v2
//! `TPE1`/`TIT2`. A file that has neither simply does not match, which costs one playlist row.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::{flac, id3};

/// FLAC metadata block type for VORBIS_COMMENT.
const VORBIS_COMMENT: u8 = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags {
    pub artist: String,
    pub title: String,
}

impl Tags {
    pub fn is_usable(&self) -> bool {
        !self.artist.is_empty() && !self.title.is_empty()
    }
}

/// Read the artist and title. `None` when the file is neither FLAC nor MP3, cannot be opened, or
/// carries no tags — all three are the same thing to the caller: no match.
pub fn read(path: &Path) -> Option<Tags> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let tags = match extension.as_str() {
        "flac" => read_flac(path)?,
        "mp3" => read_mp3(path)?,
        _ => return None,
    };
    tags.is_usable().then_some(tags)
}

fn read_flac(path: &Path) -> Option<Tags> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let layout = flac::read_layout(&mut reader).ok()?;
    let block = layout.blocks.iter().find(|b| b.kind == VORBIS_COMMENT)?;
    Some(parse_vorbis_comment(&block.data))
}

/// `vendor length, vendor, count, then count × (length, "KEY=value")` — all little-endian, all
/// UTF-8. Anything that runs off the end stops the walk instead of panicking.
pub fn parse_vorbis_comment(data: &[u8]) -> Tags {
    let mut tags = Tags::default();
    let u32_at = |at: usize| -> Option<usize> {
        let bytes = data.get(at..at + 4)?;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    };
    let Some(vendor_len) = u32_at(0) else { return tags };
    let mut at = 4 + vendor_len;
    let Some(count) = u32_at(at) else { return tags };
    at += 4;
    for _ in 0..count {
        let Some(len) = u32_at(at) else { break };
        at += 4;
        let Some(raw) = data.get(at..at + len) else { break };
        at += len;
        let text = String::from_utf8_lossy(raw);
        let Some((key, value)) = text.split_once('=') else { continue };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        match key.to_ascii_uppercase().as_str() {
            "ARTIST" if tags.artist.is_empty() => tags.artist = value,
            "TITLE" if tags.title.is_empty() => tags.title = value,
            _ => {}
        }
    }
    tags
}

fn read_mp3(path: &Path) -> Option<Tags> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let tag = id3::read_tag(&mut reader).ok()??;
    let mut tags = Tags::default();
    for frame in &tag.frames {
        match &frame.id {
            b"TPE1" if tags.artist.is_empty() => tags.artist = decode_text(&frame.body),
            b"TIT2" if tags.title.is_empty() => tags.title = decode_text(&frame.body),
            _ => {}
        }
    }
    Some(tags)
}

/// An ID3v2 text frame: one encoding byte, then the text. Latin-1, UTF-16 with a BOM, UTF-16BE and
/// UTF-8 are the four the standard allows, and taggers use all of them.
pub fn decode_text(body: &[u8]) -> String {
    let Some((encoding, rest)) = body.split_first() else { return String::new() };
    let text = match encoding {
        0 => rest.iter().map(|b| *b as char).collect::<String>(),
        1 => decode_utf16(rest, None),
        2 => decode_utf16(rest, Some(true)),
        _ => String::from_utf8_lossy(rest).into_owned(),
    };
    // Frames are NUL-padded, and a multi-value frame separates with NUL: the first value is ours.
    text.split('\u{0}').next().unwrap_or("").trim().to_string()
}

fn decode_utf16(bytes: &[u8], force_big_endian: Option<bool>) -> String {
    let (big_endian, body) = match force_big_endian {
        Some(big) => (big, bytes),
        None => match bytes {
            [0xff, 0xfe, rest @ ..] => (false, rest),
            [0xfe, 0xff, rest @ ..] => (true, rest),
            _ => (false, bytes),
        },
    };
    let units: Vec<u16> =
        body.chunks_exact(2)
            .map(|pair| {
                if big_endian {
                    u16::from_be_bytes([pair[0], pair[1]])
                } else {
                    u16::from_le_bytes([pair[0], pair[1]])
                }
            })
            .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::{decode_text, parse_vorbis_comment, read, Tags};

    fn vorbis_block(entries: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        let vendor = b"reference libFLAC";
        out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        out.extend_from_slice(vendor);
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for entry in entries {
            out.extend_from_slice(&(entry.len() as u32).to_le_bytes());
            out.extend_from_slice(entry.as_bytes());
        }
        out
    }

    #[test]
    fn a_vorbis_comment_gives_up_its_artist_and_title() {
        let block = vorbis_block(&["ALBUM=Isles", "artist=Bicep", "TITLE=Atlas", "DATE=2021"]);
        let tags = parse_vorbis_comment(&block);
        // Keys are case-insensitive in the wild; the values are not touched.
        assert_eq!(tags, Tags { artist: "Bicep".into(), title: "Atlas".into() });
    }

    #[test]
    fn a_truncated_comment_block_does_not_panic() {
        let block = vorbis_block(&["ARTIST=Bicep", "TITLE=Atlas"]);
        for cut in 0..block.len() {
            let _ = parse_vorbis_comment(&block[..cut]);
        }
    }

    #[test]
    fn id3_text_frames_decode_in_every_encoding() {
        assert_eq!(decode_text(&[0x00, b'B', b'i', b'c', b'e', b'p']), "Bicep");
        assert_eq!(decode_text(&[0x03, 0xc3, 0xa9, b'a']), "éa");
        // UTF-16 with a little-endian BOM, which is what Windows taggers write.
        let mut utf16 = vec![0x01, 0xff, 0xfe];
        for unit in "Atlas".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_text(&utf16), "Atlas");
        // A NUL-separated multi-value frame yields the first value.
        assert_eq!(decode_text(&[0x00, b'A', 0x00, b'B']), "A");
        assert_eq!(decode_text(&[]), "");
    }

    #[test]
    fn a_real_flac_file_reads_back() {
        // A minimal but valid FLAC: marker, STREAMINFO (34 bytes), then the comment block, marked
        // last. `flac::read_layout` refuses anything else, so this also pins that agreement.
        let dir = std::env::temp_dir().join(format!("flint-tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("track.flac");
        let comment = vorbis_block(&["ARTIST=Bonobo", "TITLE=Kerala"]);
        let mut file = b"fLaC".to_vec();
        file.push(0); // STREAMINFO, not last
        file.extend_from_slice(&[0, 0, 34]);
        file.extend_from_slice(&[0u8; 34]);
        file.push(0x80 | 4); // VORBIS_COMMENT, last block
        let len = comment.len();
        file.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        file.extend_from_slice(&comment);
        std::fs::write(&path, &file).unwrap();

        assert_eq!(read(&path), Some(Tags { artist: "Bonobo".into(), title: "Kerala".into() }));
        // Something that is not a music file at all is simply no match.
        let other = dir.join("cover.jpg");
        std::fs::write(&other, b"not audio").unwrap();
        assert_eq!(read(&other), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
