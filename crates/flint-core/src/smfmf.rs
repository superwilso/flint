//! SMFMF — the SensMe analysis blob that Sony's engine produces and the Walkman's scanner parses.
//!
//! The blob is a run of chunks, each a 20-byte header and a payload (Cinder
//! `analysis/RE_sensme_musiccenter.md` §3, §5):
//!
//! ```text
//! [FourCC 4][family "STAE" 4][vendor "MMLW" 4][version 01 00 80 00][payload size, u32 big-endian][payload]
//! ```
//!
//! The Walkman's own parser (`libMediaStoreService.so` 0x562e0) walks exactly this framing and refuses
//! a size that runs past the end, so `parse` applies the same rule. Only a few payloads are understood:
//! `GBPM` is the tempo as a big-endian float32. The rest are carried as bytes.

use std::fmt;

pub const HEADER_LEN: usize = 20;

/// The chunk names the Walkman's scanner knows (its `.rodata`, next to the parser).
pub const KNOWN: [&[u8; 4]; 11] =
    [b"GBPM", b"STBF", b"STSA", b"STMO", b"STHF", b"STMM", b"GVNM", b"SBZT", b"STAE", b"STNM", b"VNDM"];

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Empty,
    /// A chunk header or payload runs past the end of the blob.
    Overrun {
        at: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Empty => write!(f, "empty SMFMF data"),
            Error::Overrun { at } => write!(f, "SMFMF chunk at byte {at} runs past the end"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk<'a> {
    pub fourcc: [u8; 4],
    pub family: [u8; 4],
    pub vendor: [u8; 4],
    pub version: u32,
    pub payload: &'a [u8],
}

impl Chunk<'_> {
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.fourcc).into_owned()
    }
}

pub fn parse(bytes: &[u8]) -> Result<Vec<Chunk<'_>>, Error> {
    if bytes.is_empty() {
        return Err(Error::Empty);
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.len() < HEADER_LEN {
            return Err(Error::Overrun { at });
        }
        let size = u32::from_be_bytes([rest[16], rest[17], rest[18], rest[19]]) as usize;
        if rest.len() - HEADER_LEN < size {
            return Err(Error::Overrun { at });
        }
        let arr = |r: std::ops::Range<usize>| -> [u8; 4] { rest[r].try_into().expect("4 bytes") };
        out.push(Chunk {
            fourcc: arr(0..4),
            family: arr(4..8),
            vendor: arr(8..12),
            version: u32::from_be_bytes(arr(12..16)),
            payload: &rest[HEADER_LEN..HEADER_LEN + size],
        });
        at += HEADER_LEN + size;
    }
    Ok(out)
}

pub fn encode(chunks: &[Chunk<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in chunks {
        out.extend_from_slice(&c.fourcc);
        out.extend_from_slice(&c.family);
        out.extend_from_slice(&c.vendor);
        out.extend_from_slice(&c.version.to_be_bytes());
        out.extend_from_slice(&(c.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(c.payload);
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub bytes: usize,
    pub chunks: Vec<(String, usize)>,
    pub bpm: Option<f32>,
    /// Chunk names the Walkman's scanner does not know. Non-empty means "not what the engine emits".
    pub unknown: Vec<String>,
}

pub fn summarise(bytes: &[u8]) -> Result<Summary, Error> {
    let chunks = parse(bytes)?;
    let bpm = chunks
        .iter()
        .find(|c| &c.fourcc == b"GBPM" && c.payload.len() == 4)
        .map(|c| f32::from_be_bytes(c.payload.try_into().expect("4 bytes")));
    Ok(Summary {
        bytes: bytes.len(),
        unknown: chunks.iter().filter(|c| !KNOWN.contains(&&c.fourcc)).map(Chunk::name).collect(),
        chunks: chunks.iter().map(|c| (c.name(), c.payload.len())).collect(),
        bpm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk<'a>(name: &[u8; 4], payload: &'a [u8]) -> Chunk<'a> {
        Chunk { fourcc: *name, family: *b"STAE", vendor: *b"MMLW", version: 0x0100_8000, payload }
    }

    #[test]
    fn round_trips_and_reads_the_tempo() {
        let bpm = 93.44f32.to_be_bytes();
        let stmm = vec![7u8; 5948];
        let blob = encode(&[chunk(b"GBPM", &bpm), chunk(b"STSA", &[0; 16]), chunk(b"STMM", &stmm)]);
        assert_eq!(blob.len(), 3 * HEADER_LEN + 4 + 16 + 5948);
        // The header bytes as the engine writes them (RE note §3).
        assert_eq!(&blob[..20], b"GBPMSTAEMMLW\x01\x00\x80\x00\x00\x00\x00\x04");
        let parsed = parse(&blob).unwrap();
        assert_eq!(encode(&parsed), blob);
        let s = summarise(&blob).unwrap();
        assert_eq!(s.bpm, Some(93.44));
        assert_eq!(s.chunks, vec![("GBPM".into(), 4), ("STSA".into(), 16), ("STMM".into(), 5948)]);
        assert!(s.unknown.is_empty());
    }

    #[test]
    fn refuses_overruns_the_way_the_walkman_does() {
        let mut blob = encode(&[chunk(b"GBPM", &[0; 4])]);
        blob.truncate(blob.len() - 1);
        assert_eq!(parse(&blob), Err(Error::Overrun { at: 0 }));
        let mut two = encode(&[chunk(b"GBPM", &[0; 4])]);
        two.extend_from_slice(b"STMM");
        assert_eq!(parse(&two), Err(Error::Overrun { at: 24 }));
        assert_eq!(parse(&[]), Err(Error::Empty));
    }

    #[test]
    fn flags_names_the_scanner_does_not_know() {
        let blob = encode(&[chunk(b"GBPM", &[0; 4]), chunk(b"ZZZZ", &[])]);
        assert_eq!(summarise(&blob).unwrap().unknown, vec!["ZZZZ".to_string()]);
    }
}
