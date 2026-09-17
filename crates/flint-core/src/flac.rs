//! FLAC metadata: read the block list, and write a copy that carries an `SMFM` block.
//!
//! A FLAC file is `fLaC`, a run of metadata blocks, then audio frames. Each block has a 4-byte
//! header — bit 7 = "last metadata block", 7-bit type, 24-bit big-endian length — then its data.
//! STREAMINFO (type 0) must come first. Sony's Music Center stores SensMe analysis as an APPLICATION
//! block (type 2) whose 4-byte application id is `SMFM` (Cinder `analysis/RE_sensme_musiccenter.md`
//! §7, read from `OpcFlac.dll`); the Walkman's scanner looks for exactly that.
//!
//! Flint only ever writes into a COPY. The source is opened read-only, the destination is written to
//! a temporary name beside it and renamed into place, so a failure part-way never leaves a
//! half-written file under the real name — the rule for the Walkman's unjournalled vfat volume.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const TYPE_STREAMINFO: u8 = 0;
pub const TYPE_PADDING: u8 = 1;
pub const TYPE_APPLICATION: u8 = 2;

/// The application id Sony uses for SensMe (SMFMF) data.
pub const SMFM_ID: [u8; 4] = *b"SMFM";

/// A block's data is at most 2^24 - 1 bytes: its length field is 24 bits.
pub const MAX_BLOCK_LEN: usize = (1 << 24) - 1;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    NotFlac,
    Truncated(&'static str),
    NoStreamInfo,
    BlockTooLarge(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NotFlac => write!(f, "not a FLAC file (no fLaC marker)"),
            Error::Truncated(what) => write!(f, "file ends inside {what}"),
            Error::NoStreamInfo => write!(f, "the first metadata block is not STREAMINFO"),
            Error::BlockTooLarge(n) => write!(f, "a {n}-byte block does not fit FLAC's 24-bit length"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: u8,
    pub data: Vec<u8>,
}

impl Block {
    pub fn is_smfm(&self) -> bool {
        self.kind == TYPE_APPLICATION && self.data.len() >= 4 && self.data[..4] == SMFM_ID
    }

    /// Bytes this block occupies in the file, header included.
    pub fn encoded_len(&self) -> usize {
        4 + self.data.len()
    }
}

/// Everything before the audio frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// An ID3v2 tag some taggers put in front of `fLaC`. Not valid FLAC, but common; kept verbatim.
    pub prefix: Vec<u8>,
    pub blocks: Vec<Block>,
    /// Offset of the first audio frame.
    pub audio_offset: u64,
}

impl Layout {
    /// The SMFMF payload (after the 4-byte application id), if the file carries one.
    pub fn smfm(&self) -> Option<&[u8]> {
        self.blocks.iter().find(|b| b.is_smfm()).map(|b| &b.data[4..])
    }
}

/// Length of a leading ID3v2 tag, or 0.
fn id3v2_len(head: &[u8; 10]) -> u64 {
    if &head[..3] != b"ID3" {
        return 0;
    }
    // Synchsafe: 7 bits per byte.
    let size = head[6..10].iter().fold(0u64, |acc, b| (acc << 7) | u64::from(b & 0x7f));
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    10 + size + footer
}

pub fn read_layout<R: Read + Seek>(r: &mut R) -> Result<Layout, Error> {
    let mut head = [0u8; 10];
    r.read_exact(&mut head).map_err(|_| Error::NotFlac)?;
    let skip = id3v2_len(&head);
    let mut prefix = Vec::new();
    if skip > 0 {
        r.seek(SeekFrom::Start(0))?;
        prefix.resize(skip as usize, 0);
        r.read_exact(&mut prefix).map_err(|_| Error::Truncated("the ID3v2 tag before fLaC"))?;
    } else {
        r.seek(SeekFrom::Start(0))?;
    }
    let mut marker = [0u8; 4];
    r.read_exact(&mut marker).map_err(|_| Error::NotFlac)?;
    if &marker != b"fLaC" {
        return Err(Error::NotFlac);
    }
    let mut blocks = Vec::new();
    let mut offset = skip + 4;
    loop {
        let mut h = [0u8; 4];
        r.read_exact(&mut h).map_err(|_| Error::Truncated("a metadata block header"))?;
        let last = h[0] & 0x80 != 0;
        let kind = h[0] & 0x7f;
        let len = (usize::from(h[1]) << 16) | (usize::from(h[2]) << 8) | usize::from(h[3]);
        let mut data = vec![0u8; len];
        r.read_exact(&mut data).map_err(|_| Error::Truncated("a metadata block"))?;
        if blocks.is_empty() && kind != TYPE_STREAMINFO {
            return Err(Error::NoStreamInfo);
        }
        blocks.push(Block { kind, data });
        offset += 4 + len as u64;
        if last {
            break;
        }
    }
    Ok(Layout { prefix, blocks, audio_offset: offset })
}

/// The block list with `payload` as the file's only SMFM block.
///
/// Placement: where an existing SMFM block was, else just before the first PADDING block, else at the
/// end. When there is PADDING, it gives up exactly the bytes the change adds (or takes back what the
/// change frees), so the audio frames stay at the same offset whenever the padding is big enough —
/// which is what lets a later version update a tag in place rather than rewriting the whole file.
pub fn with_smfm(blocks: &[Block], payload: &[u8]) -> Result<Vec<Block>, Error> {
    let mut data = Vec::with_capacity(4 + payload.len());
    data.extend_from_slice(&SMFM_ID);
    data.extend_from_slice(payload);
    if data.len() > MAX_BLOCK_LEN {
        return Err(Error::BlockTooLarge(data.len()));
    }
    let new = Block { kind: TYPE_APPLICATION, data };
    let old_len: usize = blocks.iter().map(Block::encoded_len).sum();

    let mut out: Vec<Block> = Vec::with_capacity(blocks.len() + 1);
    let mut placed = false;
    for b in blocks {
        if b.is_smfm() {
            if !placed {
                out.push(new.clone());
                placed = true;
            }
            continue;
        }
        if b.kind == TYPE_PADDING && !placed {
            out.push(new.clone());
            placed = true;
        }
        out.push(b.clone());
    }
    if !placed {
        out.push(new);
    }

    let new_len: usize = out.iter().map(Block::encoded_len).sum();
    if let Some(i) = out.iter().position(|b| b.kind == TYPE_PADDING) {
        let pad = out[i].data.len();
        if new_len > old_len {
            let grow = new_len - old_len;
            if grow <= pad {
                out[i].data.truncate(pad - grow);
            } else if grow == pad + 4 {
                out.remove(i);
            }
            // Otherwise the padding cannot absorb it: the audio moves, which a copy does not mind.
        } else {
            let shrink = old_len - new_len;
            if pad + shrink <= MAX_BLOCK_LEN {
                out[i].data.resize(pad + shrink, 0);
            }
        }
    }
    Ok(out)
}

/// `fLaC` plus the blocks, with the last-block flag on the final one.
pub fn encode_metadata(prefix: &[u8], blocks: &[Block]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(prefix.len() + 4 + blocks.iter().map(Block::encoded_len).sum::<usize>());
    out.extend_from_slice(prefix);
    out.extend_from_slice(b"fLaC");
    for (i, b) in blocks.iter().enumerate() {
        if b.data.len() > MAX_BLOCK_LEN {
            return Err(Error::BlockTooLarge(b.data.len()));
        }
        let last = if i + 1 == blocks.len() { 0x80 } else { 0 };
        let len = b.data.len();
        out.push(last | (b.kind & 0x7f));
        out.push((len >> 16) as u8);
        out.push((len >> 8) as u8);
        out.push(len as u8);
        out.extend_from_slice(&b.data);
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyReport {
    pub source_len: u64,
    pub copy_len: u64,
    /// True when the audio frames sit at the same offset as in the source (padding absorbed the tag).
    pub audio_in_place: bool,
    /// True when the source already carried an SMFM block, which the copy replaced.
    pub replaced: bool,
}

fn temp_path_for(dst: &Path) -> PathBuf {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dst.with_file_name(format!(".{name}.flint-partial"))
}

/// Copy `src` to `dst` with `payload` as its SMFM block. The source is never written.
pub fn copy_with_smfm(src: &Path, dst: &Path, payload: &[u8]) -> Result<CopyReport, Error> {
    let mut input = BufReader::new(File::open(src)?);
    let layout = read_layout(&mut input)?;
    let blocks = with_smfm(&layout.blocks, payload)?;
    let meta = encode_metadata(&layout.prefix, &blocks)?;
    let source_len = input.get_ref().metadata()?.len();

    let tmp = temp_path_for(dst);
    let result = (|| -> Result<u64, Error> {
        let mut out = BufWriter::new(File::create(&tmp)?);
        out.write_all(&meta)?;
        input.seek(SeekFrom::Start(layout.audio_offset))?;
        io::copy(&mut input, &mut out)?;
        let file = out.into_inner().map_err(|e| Error::Io(e.into_error()))?;
        file.sync_all()?;
        Ok(file.metadata()?.len())
    })();
    let copy_len = match result {
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
        audio_in_place: meta.len() as u64 == layout.audio_offset,
        replaced: layout.smfm().is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn streaminfo() -> Block {
        Block { kind: TYPE_STREAMINFO, data: vec![0x11; 34] }
    }
    fn comment() -> Block {
        Block { kind: 4, data: b"\x05\0\0\0flint\0\0\0\0".to_vec() }
    }
    fn padding(n: usize) -> Block {
        Block { kind: TYPE_PADDING, data: vec![0; n] }
    }
    const AUDIO: &[u8] = b"\xff\xf8audio-frames-stand-in";

    fn file(prefix: &[u8], blocks: &[Block]) -> Vec<u8> {
        let mut f = encode_metadata(prefix, blocks).unwrap();
        f.extend_from_slice(AUDIO);
        f
    }

    fn parse(bytes: &[u8]) -> Layout {
        read_layout(&mut Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn reads_blocks_and_audio_offset() {
        let bytes = file(&[], &[streaminfo(), comment(), padding(100)]);
        let l = parse(&bytes);
        assert_eq!(l.blocks, vec![streaminfo(), comment(), padding(100)]);
        assert_eq!(&bytes[l.audio_offset as usize..], AUDIO);
        assert!(l.smfm().is_none());
    }

    #[test]
    fn padding_absorbs_the_tag_and_the_audio_stays_put() {
        let before = file(&[], &[streaminfo(), comment(), padding(8192)]);
        let l = parse(&before);
        let payload = vec![0xab; 6444];
        let blocks = with_smfm(&l.blocks, &payload).unwrap();
        let after = [encode_metadata(&[], &blocks).unwrap(), AUDIO.to_vec()].concat();
        let l2 = parse(&after);
        assert_eq!(l2.audio_offset, l.audio_offset, "the audio moved although padding could absorb the tag");
        assert_eq!(l2.smfm(), Some(&payload[..]));
        assert_eq!(&after[l2.audio_offset as usize..], AUDIO);
        // SMFM sits before the padding, and the padding shrank by exactly the block's size.
        assert!(l2.blocks[2].is_smfm());
        assert_eq!(l2.blocks[3].data.len(), 8192 - (4 + 4 + 6444));
    }

    #[test]
    fn exact_fit_removes_the_padding_block() {
        let payload = vec![1u8; 100];
        let l = parse(&file(&[], &[streaminfo(), padding(4 + 4 + 100 - 4)]));
        let blocks = with_smfm(&l.blocks, &payload).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(encode_metadata(&[], &blocks).unwrap().len() as u64, l.audio_offset);
    }

    #[test]
    fn too_little_padding_keeps_it_and_moves_the_audio() {
        let l = parse(&file(&[], &[streaminfo(), padding(10)]));
        let blocks = with_smfm(&l.blocks, &[7u8; 500]).unwrap();
        assert_eq!(blocks[2].data.len(), 10);
        let after = [encode_metadata(&[], &blocks).unwrap(), AUDIO.to_vec()].concat();
        let l2 = parse(&after);
        assert_eq!(&after[l2.audio_offset as usize..], AUDIO);
    }

    #[test]
    fn no_padding_appends_and_sets_the_last_flag() {
        let l = parse(&file(&[], &[streaminfo(), comment()]));
        let blocks = with_smfm(&l.blocks, b"xyz").unwrap();
        let meta = encode_metadata(&[], &blocks).unwrap();
        let l2 = parse(&[meta, AUDIO.to_vec()].concat());
        assert_eq!(l2.blocks.len(), 3);
        assert!(l2.blocks[2].is_smfm());
        assert_eq!(l2.smfm(), Some(&b"xyz"[..]));
    }

    #[test]
    fn an_existing_smfm_block_is_replaced_not_duplicated() {
        let old = Block { kind: TYPE_APPLICATION, data: [&SMFM_ID[..], &[9u8; 50]].concat() };
        let other_app = Block { kind: TYPE_APPLICATION, data: b"riffxxxx".to_vec() };
        let l = parse(&file(&[], &[streaminfo(), old, other_app.clone(), padding(1000)]));
        let blocks = with_smfm(&l.blocks, &[3u8; 20]).unwrap();
        assert_eq!(blocks.iter().filter(|b| b.is_smfm()).count(), 1);
        assert!(blocks[1].is_smfm(), "the replacement should take the old block's place");
        assert_eq!(blocks[2], other_app, "an unrelated APPLICATION block was touched");
        // Smaller than before: the padding took the freed bytes back.
        let meta = encode_metadata(&[], &blocks).unwrap();
        assert_eq!(meta.len() as u64, l.audio_offset);
    }

    #[test]
    fn a_leading_id3v2_tag_is_kept() {
        // ID3v2.4 header, synchsafe size 20, then 20 bytes of frames.
        let mut id3 = b"ID3\x04\x00\x00\x00\x00\x00\x14".to_vec();
        id3.extend_from_slice(&[0x41; 20]);
        let l = parse(&file(&id3, &[streaminfo(), padding(64)]));
        assert_eq!(l.prefix, id3);
        let blocks = with_smfm(&l.blocks, b"ab").unwrap();
        let meta = encode_metadata(&l.prefix, &blocks).unwrap();
        assert!(meta.starts_with(&id3));
        assert_eq!(meta.len() as u64, l.audio_offset);
    }

    #[test]
    fn rejects_what_is_not_flac() {
        assert!(matches!(read_layout(&mut Cursor::new(b"RIFF....WAVEfmt ".to_vec())), Err(Error::NotFlac)));
        let mut bad = b"fLaC".to_vec();
        bad.extend_from_slice(&[0x84, 0, 0, 2, 0, 0]); // VORBIS_COMMENT first
        assert!(matches!(read_layout(&mut Cursor::new(bad)), Err(Error::NoStreamInfo)));
        let mut short = b"fLaC".to_vec();
        short.extend_from_slice(&[0x80, 0, 0, 34, 1, 2]);
        assert!(matches!(read_layout(&mut Cursor::new(short)), Err(Error::Truncated(_))));
    }

    #[test]
    fn copy_writes_a_new_file_and_leaves_the_source_alone() {
        let dir = std::env::temp_dir().join(format!("flint-flac-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.flac");
        let dst = dir.join("dst.flac");
        let original = file(&[], &[streaminfo(), comment(), padding(8192)]);
        fs::write(&src, &original).unwrap();

        let report = copy_with_smfm(&src, &dst, &[0x5a; 6000]).unwrap();
        assert_eq!(fs::read(&src).unwrap(), original, "the source was modified");
        assert!(report.audio_in_place);
        assert!(!report.replaced);
        assert_eq!(report.copy_len, report.source_len);
        let copy = fs::read(&dst).unwrap();
        let l = parse(&copy);
        assert_eq!(l.smfm().map(<[u8]>::len), Some(6000));
        assert_eq!(&copy[l.audio_offset as usize..], AUDIO);
        assert!(!temp_path_for(&dst).exists(), "the partial file was left behind");

        // Re-tagging the copy's copy replaces, never stacks.
        let dst2 = dir.join("dst2.flac");
        let again = copy_with_smfm(&dst, &dst2, &[1; 10]).unwrap();
        assert!(again.replaced);
        assert_eq!(parse(&fs::read(&dst2).unwrap()).smfm(), Some(&[1u8; 10][..]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_copy_leaves_nothing_under_either_name() {
        let dir = std::env::temp_dir().join(format!("flint-flac-fail-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("not.flac");
        fs::write(&src, b"definitely not flac").unwrap();
        let dst = dir.join("out.flac");
        assert!(copy_with_smfm(&src, &dst, b"x").is_err());
        assert!(!dst.exists());
        assert!(!temp_path_for(&dst).exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
