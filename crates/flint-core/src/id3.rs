//! ID3v2: write a copy of an MP3 whose tag carries Sony's SensMe data as a `GEOB` frame.
//!
//! The frame is the one Sony's `OmgPcMan.dll` writes from its built-in templates (Cinder
//! `analysis/RE_sensme_musiccenter.md` §10): text encoding 0, MIME type `Application/SMFMF`, an empty
//! filename, description `USR_SMFMF`, then the SMFMF bytes. The Walkman's MP3 parser reads it into
//! the same database rows as the FLAC `SMFM` block.
//!
//! Scope, deliberately narrow: ID3v2.3 and v2.4 tags are rewritten; frames other than an existing
//! SMFMF `GEOB` are carried across byte for byte, flags included. A tag using whole-tag
//! unsynchronisation, a v2.4 footer, or v2.2 is refused rather than half-understood. An MP3 with no
//! ID3v2 tag gets a new v2.3 tag holding only the frame. Like the FLAC path, only a COPY is written.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const SMFMF_MIME: &str = "Application/SMFMF";
pub const SMFMF_DESCRIPTION: &str = "USR_SMFMF";

/// Padding given to a rewritten tag that had to grow, so a later update can happen in place.
pub const GROWTH_PADDING: usize = 4096;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    UnsupportedVersion(u8),
    Unsynchronised,
    Footer,
    Truncated(&'static str),
    TooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::UnsupportedVersion(v) => write!(f, "ID3v2.{v} tags are not rewritten (only 2.3 and 2.4)"),
            Error::Unsynchronised => write!(f, "the ID3 tag uses whole-tag unsynchronisation; not rewritten"),
            Error::Footer => write!(f, "the ID3v2.4 tag has a footer; not rewritten"),
            Error::Truncated(what) => write!(f, "file ends inside {what}"),
            Error::TooLarge => write!(f, "the tag would exceed ID3v2's 256 MB limit"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

fn synchsafe(bytes: &[u8]) -> usize {
    bytes.iter().fold(0usize, |acc, b| (acc << 7) | usize::from(b & 0x7f))
}

fn to_synchsafe(n: usize) -> Result<[u8; 4], Error> {
    if n >= 1 << 28 {
        return Err(Error::TooLarge);
    }
    Ok([(n >> 21) as u8 & 0x7f, (n >> 14) as u8 & 0x7f, (n >> 7) as u8 & 0x7f, n as u8 & 0x7f])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: [u8; 4],
    pub flags: [u8; 2],
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    /// 3 or 4.
    pub major: u8,
    pub revision: u8,
    pub flags: u8,
    /// The extended header, verbatim, when flag 0x40 is set.
    pub extended: Vec<u8>,
    pub frames: Vec<Frame>,
    /// Bytes the whole tag occupied in the file (header included) — where the audio starts.
    pub total_len: usize,
}

/// Read a leading ID3v2 tag. `Ok(None)` when the file does not start with one.
pub fn read_tag<R: Read + Seek>(r: &mut R) -> Result<Option<Tag>, Error> {
    let mut h = [0u8; 10];
    r.seek(SeekFrom::Start(0))?;
    match r.read_exact(&mut h) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    if &h[..3] != b"ID3" {
        return Ok(None);
    }
    let (major, revision, flags) = (h[3], h[4], h[5]);
    if major != 3 && major != 4 {
        return Err(Error::UnsupportedVersion(major));
    }
    if flags & 0x80 != 0 {
        return Err(Error::Unsynchronised);
    }
    if major == 4 && flags & 0x10 != 0 {
        return Err(Error::Footer);
    }
    let size = synchsafe(&h[6..10]);
    let mut body = vec![0u8; size];
    r.read_exact(&mut body).map_err(|_| Error::Truncated("the ID3v2 tag"))?;

    let mut at = 0;
    let mut extended = Vec::new();
    if flags & 0x40 != 0 {
        if body.len() < 4 {
            return Err(Error::Truncated("the extended header"));
        }
        // v2.3: size excludes its own 4 bytes. v2.4: synchsafe and includes them.
        let len = if major == 3 {
            4 + u32::from_be_bytes(body[0..4].try_into().expect("4 bytes")) as usize
        } else {
            synchsafe(&body[0..4])
        };
        if len > body.len() {
            return Err(Error::Truncated("the extended header"));
        }
        extended = body[..len].to_vec();
        at = len;
    }

    let mut frames = Vec::new();
    while at + 10 <= body.len() {
        let id: [u8; 4] = body[at..at + 4].try_into().expect("4 bytes");
        if id[0] == 0 {
            break; // padding
        }
        let len = if major == 3 {
            u32::from_be_bytes(body[at + 4..at + 8].try_into().expect("4 bytes")) as usize
        } else {
            synchsafe(&body[at + 4..at + 8])
        };
        let start = at + 10;
        if start + len > body.len() {
            return Err(Error::Truncated("an ID3 frame"));
        }
        frames.push(Frame { id, flags: [body[at + 8], body[at + 9]], body: body[start..start + len].to_vec() });
        at = start + len;
    }
    Ok(Some(Tag { major, revision, flags, extended, frames, total_len: 10 + size }))
}

/// The body of Sony's SMFMF `GEOB` frame.
pub fn smfmf_geob_body(smfmf: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(2 + SMFMF_MIME.len() + SMFMF_DESCRIPTION.len() + 2 + smfmf.len());
    b.push(0); // ISO-8859-1
    b.extend_from_slice(SMFMF_MIME.as_bytes());
    b.push(0);
    b.push(0); // empty filename
    b.extend_from_slice(SMFMF_DESCRIPTION.as_bytes());
    b.push(0);
    b.extend_from_slice(smfmf);
    b
}

/// A `GEOB` body's MIME type, whatever its text encoding (the MIME type itself is always Latin-1).
fn geob_mime(body: &[u8]) -> Option<&[u8]> {
    let rest = body.get(1..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    Some(&rest[..end])
}

impl Frame {
    pub fn is_smfmf(&self) -> bool {
        &self.id == b"GEOB" && geob_mime(&self.body).is_some_and(|m| m.eq_ignore_ascii_case(SMFMF_MIME.as_bytes()))
    }

    /// The SMFMF bytes of an encoding-0 SMFMF `GEOB` (the form Flint writes).
    pub fn smfmf_payload(&self) -> Option<&[u8]> {
        if !self.is_smfmf() || self.body.first() != Some(&0) {
            return None;
        }
        let rest = &self.body[1..];
        let mime_end = rest.iter().position(|&c| c == 0)?;
        let after_mime = &rest[mime_end + 1..];
        let file_end = after_mime.iter().position(|&c| c == 0)?;
        let after_file = &after_mime[file_end + 1..];
        let desc_end = after_file.iter().position(|&c| c == 0)?;
        Some(&after_file[desc_end + 1..])
    }
}

impl Tag {
    pub fn smfmf(&self) -> Option<&[u8]> {
        self.frames.iter().find_map(Frame::smfmf_payload)
    }
}

/// The tag with `smfmf` as its only SMFMF `GEOB`, in the place of any old one or appended.
pub fn with_smfmf(tag: &Tag, smfmf: &[u8]) -> Tag {
    let new = Frame { id: *b"GEOB", flags: [0, 0], body: smfmf_geob_body(smfmf) };
    let mut frames = Vec::with_capacity(tag.frames.len() + 1);
    let mut placed = false;
    for f in &tag.frames {
        if f.is_smfmf() {
            if !placed {
                frames.push(new.clone());
                placed = true;
            }
        } else {
            frames.push(f.clone());
        }
    }
    if !placed {
        frames.push(new);
    }
    Tag { frames, ..tag.clone() }
}

/// Encode a tag. It fills exactly `fit` bytes when it can (the old size, so the audio does not move);
/// otherwise it grows with `GROWTH_PADDING` bytes of padding.
pub fn encode_tag(tag: &Tag, fit: Option<usize>) -> Result<Vec<u8>, Error> {
    let mut body = tag.extended.clone();
    for f in &tag.frames {
        body.extend_from_slice(&f.id);
        if tag.major == 3 {
            body.extend_from_slice(&u32::try_from(f.body.len()).map_err(|_| Error::TooLarge)?.to_be_bytes());
        } else {
            body.extend_from_slice(&to_synchsafe(f.body.len())?);
        }
        body.extend_from_slice(&f.flags);
        body.extend_from_slice(&f.body);
    }
    let unpadded = 10 + body.len();
    let total = match fit {
        Some(n) if n >= unpadded => n,
        _ => unpadded + GROWTH_PADDING,
    };
    body.resize(total - 10, 0);
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"ID3");
    out.push(tag.major);
    out.push(tag.revision);
    out.push(tag.flags);
    out.extend_from_slice(&to_synchsafe(body.len())?);
    out.extend_from_slice(&body);
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyReport {
    pub source_len: u64,
    pub copy_len: u64,
    pub audio_in_place: bool,
    pub replaced: bool,
    pub created_tag: bool,
}

/// Copy `src` to `dst` with `smfmf` in the ID3v2 tag. The source is never written.
pub fn copy_with_smfmf(src: &Path, dst: &Path, smfmf: &[u8]) -> Result<CopyReport, Error> {
    let mut input = BufReader::new(File::open(src)?);
    let existing = read_tag(&mut input)?;
    let source_len = input.get_ref().metadata()?.len();
    let (encoded, audio_offset, replaced, created) = match &existing {
        Some(tag) => {
            let new = with_smfmf(tag, smfmf);
            (encode_tag(&new, Some(tag.total_len))?, tag.total_len as u64, tag.smfmf().is_some(), false)
        }
        None => {
            let tag = Tag { major: 3, revision: 0, flags: 0, extended: Vec::new(), frames: Vec::new(), total_len: 0 };
            (encode_tag(&with_smfmf(&tag, smfmf), None)?, 0, false, true)
        }
    };

    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dst.with_file_name(format!(".{name}.flint-partial"));
    let written = (|| -> Result<u64, Error> {
        let mut out = BufWriter::new(File::create(&tmp)?);
        out.write_all(&encoded)?;
        input.seek(SeekFrom::Start(audio_offset))?;
        io::copy(&mut input, &mut out)?;
        let file = out.into_inner().map_err(|e| Error::Io(e.into_error()))?;
        file.sync_all()?;
        Ok(file.metadata()?.len())
    })();
    let copy_len = match written {
        Ok(n) => n,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if let Err(e) = fs::rename(&tmp, dst) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(CopyReport {
        source_len,
        copy_len,
        audio_in_place: existing.is_some() && encoded.len() as u64 == audio_offset,
        replaced,
        created_tag: created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const AUDIO: &[u8] = b"\xff\xfb\x90\x64mpeg-frames-stand-in";

    fn text_frame(id: &[u8; 4], text: &str) -> Frame {
        let mut body = vec![3u8];
        body.extend_from_slice(text.as_bytes());
        Frame { id: *id, flags: [0, 0], body }
    }

    fn file(major: u8, frames: Vec<Frame>, padding: usize) -> Vec<u8> {
        let tag = Tag { major, revision: 0, flags: 0, extended: Vec::new(), frames, total_len: 0 };
        let mut bytes = encode_tag(&tag, None).unwrap();
        // encode_tag adds GROWTH_PADDING when not fitting; trim to the padding asked for.
        let content = bytes.len() - 10 - GROWTH_PADDING;
        bytes.resize(10 + content + padding, 0);
        let size = to_synchsafe(content + padding).unwrap();
        bytes[6..10].copy_from_slice(&size);
        bytes.extend_from_slice(AUDIO);
        bytes
    }

    fn parse(bytes: &[u8]) -> Tag {
        read_tag(&mut Cursor::new(bytes)).unwrap().unwrap()
    }

    #[test]
    fn the_frame_matches_sonys_template_byte_for_byte() {
        // OmgPcMan.dll's encoding-0 template: 30 bytes before the object data.
        let body = smfmf_geob_body(b"");
        assert_eq!(body, b"\x00Application/SMFMF\x00\x00USR_SMFMF\x00");
        assert_eq!(body.len(), 0x1e);
    }

    #[test]
    fn v24_with_room_keeps_the_audio_in_place() {
        let bytes = file(4, vec![text_frame(b"TIT2", "Song"), text_frame(b"TPE1", "Artist")], 8000);
        let tag = parse(&bytes);
        let new = with_smfmf(&tag, &[0xab; 6000]);
        let encoded = encode_tag(&new, Some(tag.total_len)).unwrap();
        assert_eq!(encoded.len(), tag.total_len);
        let back = parse(&[encoded, AUDIO.to_vec()].concat());
        assert_eq!(back.smfmf(), Some(&[0xab; 6000][..]));
        assert_eq!(back.frames[..2], tag.frames[..]);
    }

    #[test]
    fn v23_sizes_are_plain_big_endian_and_the_tag_grows_when_it_must() {
        let big = "x".repeat(200); // > 127 bytes: a synchsafe/plain mix-up would corrupt this frame
        let bytes = file(3, vec![text_frame(b"COMM", &big)], 10);
        let tag = parse(&bytes);
        assert_eq!(tag.frames[0].body.len(), 201);
        let encoded = encode_tag(&with_smfmf(&tag, &[1; 3000]), Some(tag.total_len)).unwrap();
        assert!(encoded.len() > tag.total_len);
        let back = parse(&[encoded.clone(), AUDIO.to_vec()].concat());
        assert_eq!(back.major, 3);
        assert_eq!(back.frames[0], tag.frames[0]);
        assert_eq!(back.smfmf(), Some(&[1u8; 3000][..]));
        assert_eq!(&[encoded, AUDIO.to_vec()].concat()[back.total_len..], AUDIO);
    }

    #[test]
    fn an_existing_smfmf_frame_is_replaced_in_place_and_other_geobs_kept() {
        let other = Frame { id: *b"GEOB", flags: [0, 0], body: b"\x00image/png\x00a.png\x00art\x00PNG".to_vec() };
        let old = Frame { id: *b"GEOB", flags: [0, 0], body: smfmf_geob_body(&[9; 40]) };
        let tag = parse(&file(4, vec![text_frame(b"TIT2", "t"), old, other.clone()], 100));
        let new = with_smfmf(&tag, &[2; 10]);
        assert_eq!(new.frames.iter().filter(|f| f.is_smfmf()).count(), 1);
        assert!(new.frames[1].is_smfmf());
        assert_eq!(new.frames[2], other);
    }

    #[test]
    fn a_utf16_smfmf_geob_written_by_sony_is_recognised_and_replaced() {
        // Template with encoding 1 and a BOM, as OmgPcMan also writes.
        let mut body = b"\x01Application/SMFMF\x00\xff\xfe\x00\x00\xff\xfe".to_vec();
        for c in "USR_SMFMF".encode_utf16() {
            body.extend_from_slice(&c.to_le_bytes());
        }
        body.extend_from_slice(&[0, 0, 7, 7, 7]);
        let sony = Frame { id: *b"GEOB", flags: [0, 0], body };
        let tag = parse(&file(3, vec![sony], 0));
        assert!(tag.frames[0].is_smfmf());
        let new = with_smfmf(&tag, b"ours");
        assert_eq!(new.frames.len(), 1);
        assert_eq!(new.smfmf(), Some(&b"ours"[..]));
    }

    #[test]
    fn refuses_tags_it_would_misread() {
        let mut v22 = b"ID3\x02\x00\x00\x00\x00\x00\x00".to_vec();
        v22.extend_from_slice(AUDIO);
        assert!(matches!(read_tag(&mut Cursor::new(v22)), Err(Error::UnsupportedVersion(2))));
        let mut unsync = file(3, vec![text_frame(b"TIT2", "t")], 0);
        unsync[5] = 0x80;
        assert!(matches!(read_tag(&mut Cursor::new(unsync)), Err(Error::Unsynchronised)));
        let mut footer = file(4, vec![text_frame(b"TIT2", "t")], 0);
        footer[5] = 0x10;
        assert!(matches!(read_tag(&mut Cursor::new(footer)), Err(Error::Footer)));
        assert!(read_tag(&mut Cursor::new(AUDIO.to_vec())).unwrap().is_none());
    }

    #[test]
    fn copy_creates_a_tag_when_there_is_none_and_never_touches_the_source() {
        let dir = std::env::temp_dir().join(format!("flint-id3-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("bare.mp3");
        fs::write(&src, AUDIO).unwrap();
        let dst = dir.join("tagged.mp3");
        let r = copy_with_smfmf(&src, &dst, &[5; 64]).unwrap();
        assert!(r.created_tag);
        assert_eq!(fs::read(&src).unwrap(), AUDIO);
        let copy = fs::read(&dst).unwrap();
        let tag = parse(&copy);
        assert_eq!(tag.major, 3);
        assert_eq!(tag.smfmf(), Some(&[5u8; 64][..]));
        assert_eq!(&copy[tag.total_len..], AUDIO);

        let src2 = dir.join("tagged2.mp3");
        let again = copy_with_smfmf(&dst, &src2, &[6; 64]).unwrap();
        assert!(again.replaced && again.audio_in_place);
        fs::remove_dir_all(&dir).unwrap();
    }
}
