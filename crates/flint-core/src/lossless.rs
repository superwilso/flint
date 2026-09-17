//! The lossless check: signs that a FLAC is not the lossless audio it claims to be.
//!
//! A "fake FLAC" is usually lossy audio (MP3, AAC, Opus) decoded and saved as FLAC, or a file dressed
//! up as more than it is: CD audio resampled to 96 kHz, or 16-bit samples padded out to 24. The
//! check decodes the track at its own rate and depth and measures four things:
//!
//! * **A lowpass wall.** Lossy encoders cut everything above a fixed frequency (LAME: about 17 kHz at
//!   128 kbit/s, 19 kHz at 192, 20.5 kHz at 320). In the track's long-term average spectrum that is a
//!   drop of tens of dB within a few hundred Hz that never comes back. Real recordings roll off
//!   gradually, or at the converter's own filter right under Nyquist, which the search ignores.
//! * **A gated top band.** Below the wall, lossy encoders switch the highest band on and off from one
//!   frame to the next (MP3's `sfb21`). `holes` is the share of loud frames whose top band is gone.
//! * **Upsampling.** A 88.2-192 kHz file whose content stops at CD or DAT bandwidth.
//! * **Padded depth.** A 24-bit file whose low bits are zero in every sample.
//!
//! None of this proves a file genuine. A clean result means none of the usual signs; a lossy source
//! that was resampled or dithered on the way can pass, and a master with its own early lowpass can
//! look suspect. Thresholds are in [`findings`], apart from the numbers that are measured and stored.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crate::cache::{clean, write_atomic};
use crate::flac;

/// Bump when a stored number would come out differently; rows from another method are measured again.
pub const METHOD: u32 = 4;

/// Width of one analysis band.
pub const BAND_HZ: u32 = 100;
/// Upper bound on analysed FFT frames per track; longer tracks are sampled evenly.
const MAX_FRAMES: u64 = 1500;
/// Walls are searched for above this frequency only.
const SEARCH_FROM_HZ: u32 = 11_000;
/// Content counts as present this far above the noise floor.
const STEP_DB: f32 = 10.0;
/// Above a wall, every band stays within this of the floor.
const FLAT_DB: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub rate: u32,
    pub channels: u8,
    pub bits: u8,
    pub samples: u64,
}

/// Rate, channels, depth and length from a STREAMINFO block's 34 bytes.
pub fn stream_info(block: &[u8]) -> Option<StreamInfo> {
    if block.len() != 34 {
        return None;
    }
    let rate = (u32::from(block[10]) << 12) | (u32::from(block[11]) << 4) | (u32::from(block[12]) >> 4);
    let channels = ((block[12] >> 1) & 0x07) + 1;
    let bits = (((block[12] & 0x01) << 4) | (block[13] >> 4)) + 1;
    let samples = (u64::from(block[13] & 0x0f) << 32) | u64::from(u32::from_be_bytes(block[14..18].try_into().ok()?));
    (rate > 0).then_some(StreamInfo { rate, channels, bits, samples })
}

/// Radix-2 FFT with a Hann window, returning power only.
struct Fft {
    n: usize,
    rev: Vec<usize>,
    cos: Vec<f32>,
    sin: Vec<f32>,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let angle = |k: usize| 2.0 * PI * k as f64 / n as f64;
        Fft {
            n,
            rev: (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect(),
            cos: (0..n / 2).map(|k| angle(k).cos() as f32).collect(),
            sin: (0..n / 2).map(|k| angle(k).sin() as f32).collect(),
            window: (0..n).map(|i| (0.5 - 0.5 * angle(i).cos()) as f32).collect(),
            re: vec![0.0; n],
            im: vec![0.0; n],
        }
    }

    /// `out[k]` = |X(k)|² for k in 0..n/2.
    fn power(&mut self, frame: &[f32], out: &mut [f32]) {
        let n = self.n;
        for (i, (x, w)) in frame.iter().zip(&self.window).enumerate() {
            self.re[self.rev[i]] = x * w;
        }
        self.im.fill(0.0);
        let (re, im) = (&mut self.re, &mut self.im);
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (c, s) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c + im[b] * s;
                    let ti = im[b] * c - re[b] * s;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
        for k in 0..n / 2 {
            out[k] = re[k] * re[k] + im[k] * im[k];
        }
    }
}

/// What the check measured for one track. Everything [`findings`] needs is stored; `spectrum` and
/// `floor_db` are only filled by a fresh measurement, for `flint check --verbose`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measurement {
    pub rate: u32,
    pub channels: u8,
    pub bits: u8,
    /// Bits actually used by the samples (0 = digital silence throughout).
    pub effective_bits: u8,
    pub seconds: f64,
    /// FFT frames analysed; too few and the spectrum means little.
    pub frames: u32,
    /// Top of the content, if it ends in a wall.
    pub cutoff_hz: Option<u32>,
    /// Size of the drop at the wall, dB.
    pub wall_db: f32,
    /// Share of loud frames whose band just under the wall has fallen to the floor (0..1).
    pub holes: Option<f32>,
    /// How far the 28 kHz-and-up region sits below the 16-20 kHz level, dB. `None` below 64 kHz,
    /// where there is no such region.
    pub ultra_gap_db: Option<f32>,
    /// The noise floor relative to the 2-8 kHz average, dB (not stored).
    pub floor_db: f32,
    /// Lines FFmpeg printed while decoding: errors in the stream itself.
    pub decode_errors: u32,
    /// Level per band relative to the 2-8 kHz average, dB.
    pub spectrum: Vec<f32>,
}

/// Streaming analysis of interleaved 32-bit samples (FFmpeg `s32le`, left-justified).
pub struct Analyser {
    info: StreamInfo,
    fft: Fft,
    band_of: Vec<usize>,
    bands: usize,
    stride: u64,
    frame_index: u64,
    mono: Vec<f32>,
    fill: usize,
    power: Vec<f32>,
    /// Band powers of each analysed frame, `bands` per frame.
    frames: Vec<f32>,
    or_bits: u32,
    sample_frames: u64,
    carry: Vec<u8>,
}

impl Analyser {
    pub fn new(info: StreamInfo) -> Analyser {
        // About 5 Hz per bin at any rate: 8192 points at 44.1/48 kHz, more above.
        let n = 8192 * (info.rate as usize).div_ceil(48_000).next_power_of_two();
        let bands = (info.rate / 2 / BAND_HZ) as usize;
        let band_of = (0..n / 2)
            .map(|k| ((k as u64 * u64::from(info.rate) / n as u64) / u64::from(BAND_HZ)) as usize)
            .map(|b| b.min(bands.saturating_sub(1)))
            .collect();
        let expected = info.samples / n as u64;
        Analyser {
            info,
            fft: Fft::new(n),
            band_of,
            bands,
            stride: expected.div_ceil(MAX_FRAMES).max(1),
            frame_index: 0,
            mono: vec![0.0; n],
            fill: 0,
            power: vec![0.0; n / 2],
            frames: Vec::new(),
            or_bits: 0,
            sample_frames: 0,
            carry: Vec::new(),
        }
    }

    /// Feed decoder output; any length, split anywhere.
    pub fn feed(&mut self, bytes: &[u8]) {
        let width = 4 * usize::from(self.info.channels);
        let mut data = bytes;
        if !self.carry.is_empty() {
            let need = width - self.carry.len();
            let take = need.min(data.len());
            self.carry.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.carry.len() < width {
                return;
            }
            let frame = std::mem::take(&mut self.carry);
            self.frame(&frame);
        }
        let whole = data.len() / width * width;
        for frame in data[..whole].chunks_exact(width) {
            self.frame(frame);
        }
        self.carry.extend_from_slice(&data[whole..]);
    }

    fn frame(&mut self, frame: &[u8]) {
        let scale = 1.0 / (f32::from(self.info.channels) * 2_147_483_648.0);
        let mut sum = 0.0f32;
        for s in frame.chunks_exact(4) {
            let v = i32::from_le_bytes(s.try_into().expect("4"));
            self.or_bits |= v as u32;
            sum += v as f32;
        }
        self.sample_frames += 1;
        self.mono[self.fill] = sum * scale;
        self.fill += 1;
        if self.fill == self.mono.len() {
            self.fill = 0;
            if self.frame_index.is_multiple_of(self.stride) && (self.frames.len() / self.bands) < 2 * MAX_FRAMES as usize {
                self.fft.power(&self.mono, &mut self.power);
                let start = self.frames.len();
                self.frames.resize(start + self.bands, 0.0);
                let row = &mut self.frames[start..];
                // Bin 0 is DC; it says nothing about bandwidth.
                for (k, p) in self.power.iter().enumerate().skip(1) {
                    row[self.band_of[k]] += p;
                }
            }
            self.frame_index += 1;
        }
    }

    pub fn finish(self, decode_errors: u32) -> Measurement {
        let info = self.info;
        let bands = self.bands;
        let nf = self.frames.len().checked_div(bands).unwrap_or(0);
        let mut m = Measurement {
            rate: info.rate,
            channels: info.channels,
            bits: info.bits,
            effective_bits: if self.or_bits == 0 { 0 } else { (32 - self.or_bits.trailing_zeros()) as u8 },
            seconds: self.sample_frames as f64 / f64::from(info.rate),
            frames: nf as u32,
            decode_errors,
            ..Measurement::default()
        };
        let band = |hz: u32| (hz / BAND_HZ) as usize;
        let (ref_lo, ref_hi) = (band(2_000), band(8_000).min(bands));
        if nf < 8 || ref_hi <= ref_lo || m.effective_bits == 0 {
            return m;
        }

        // Mean power per band over the analysed frames. Per-frame comparisons below rest on this
        // being a per-frame level, not a sum.
        let mut mean = vec![0.0f64; bands];
        for row in self.frames.chunks_exact(bands) {
            for (acc, p) in mean.iter_mut().zip(row) {
                *acc += f64::from(*p);
            }
        }
        for m in &mut mean {
            *m /= nf as f64;
        }
        let ref_power = mean[ref_lo..ref_hi].iter().sum::<f64>() / (ref_hi - ref_lo) as f64;
        if ref_power <= 0.0 {
            return m;
        }
        // Relative to the reference level, floored so digital silence does not become -inf.
        let db: Vec<f32> = mean.iter().map(|p| (10.0 * (p / ref_power).max(1e-13).log10()) as f32).collect();
        let avg = |r: std::ops::Range<usize>| db[r.clone()].iter().sum::<f32>() / r.len() as f32;

        // The noise floor: the median of the top 700 Hz, leaving out the last 300 Hz where every
        // converter's own anti-alias filter lives.
        let last = bands - band(300);
        let floor_bands = band(700);
        if last < band(SEARCH_FROM_HZ) + floor_bands {
            m.spectrum = db;
            return m;
        }
        let mut top = db[last - floor_bands..last].to_vec();
        top.sort_by(f32::total_cmp);
        let floor = top[top.len() / 2];
        m.floor_db = floor;

        // The wall: the highest band still clearly above the floor, reaching the floor within a
        // couple of bands and staying there. The steepness is what separates an encoder's lowpass
        // from music simply fading into the dither: a roll-off takes kHz to get down, a lowpass takes
        // 100-200 Hz. Resamplers' filters are gentler, so high-rate files are given 2 kHz.
        let knee_max = if info.rate >= 88_200 { band(2_000) } else { 2 };
        let c = (band(SEARCH_FROM_HZ)..last).rev().find(|&i| db[i] >= floor + STEP_DB).map(|i| i + 1);
        if let Some((c, knee)) = c.and_then(|c| (c..last).find(|&i| db[i] <= floor + FLAT_DB).map(|k| (c, k))) {
            let above = knee..last;
            let edge = avg(c - band(500)..c);
            let flat = above.len() >= band(500) && db[above].iter().all(|&d| d <= floor + FLAT_DB);
            if flat && knee - c <= knee_max && edge >= floor + STEP_DB {
                let cutoff = c as u32 * BAND_HZ;
                m.cutoff_hz = Some(cutoff);
                m.wall_db = edge - floor;

                // Gating: loud frames whose band just under the wall has fallen to the floor.
                let (t_lo, t_hi) = (band(cutoff.saturating_sub(1_500)), band(cutoff.saturating_sub(200)));
                let floor_power = ref_power * 10f64.powf(f64::from(floor + FLAT_DB) / 10.0);
                let mut loud = 0usize;
                let mut gone = 0usize;
                for row in self.frames.chunks_exact(bands) {
                    let r = row[ref_lo..ref_hi].iter().map(|&p| f64::from(p)).sum::<f64>() / (ref_hi - ref_lo) as f64;
                    if r < ref_power / 100.0 {
                        continue;
                    }
                    loud += 1;
                    let t = row[t_lo..t_hi].iter().map(|&p| f64::from(p)).sum::<f64>() / (t_hi - t_lo) as f64;
                    if t <= floor_power {
                        gone += 1;
                    }
                }
                if loud > 0 {
                    m.holes = Some(gone as f32 / loud as f32);
                }
            }
        }
        // Ultrasonic content, for high-rate files: how far the region no 44.1/48 kHz source can
        // reach sits below the top of the audible band. A resampler leaves images there rather than
        // a clean floor, so this asks how quiet it is, not how flat.
        let (u_lo, u_hi) = (band(28_000), bands.saturating_sub(band(1_000)));
        let (h_lo, h_hi) = (band(16_000), band(20_000));
        if u_hi > u_lo && h_hi <= bands {
            let mean_power = |r: std::ops::Range<usize>| mean[r.clone()].iter().sum::<f64>() / r.len() as f64;
            let (high, ultra) = (mean_power(h_lo..h_hi), mean_power(u_lo..u_hi));
            if high > 0.0 {
                m.ultra_gap_db = Some((10.0 * (high / ultra.max(high * 1e-13)).log10()) as f32);
            }
        }
        m.spectrum = db;
        m
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Finding {
    /// A lowpass wall where lossy encoders put theirs.
    Lossy { cutoff_hz: u32 },
    /// A wall at a height shared by high-bitrate lossy encoders and some masters.
    Suspect { cutoff_hz: u32 },
    /// A high-rate file whose content stops at CD or DAT bandwidth.
    Upsampled { above_hz: u32 },
    /// Fewer bits used than the file declares.
    Padded { bits: u8, effective_bits: u8 },
    /// FFmpeg reported errors in the stream.
    DecodeErrors { lines: u32 },
    /// Too short or too quiet to judge.
    Inconclusive,
}

impl Finding {
    pub fn label(&self) -> &'static str {
        match self {
            Finding::Lossy { .. } => "LOSSY",
            Finding::Suspect { .. } => "SUSPECT",
            Finding::Upsampled { .. } => "UPSAMPLED",
            Finding::Padded { .. } => "PADDED",
            Finding::DecodeErrors { .. } => "DAMAGED",
            Finding::Inconclusive => "UNSURE",
        }
    }
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let khz = |hz: &u32| f64::from(*hz) / 1000.0;
        match self {
            Finding::Lossy { cutoff_hz } => {
                write!(f, "content stops dead at {:.1} kHz, like a lossy encoder", khz(cutoff_hz))
            }
            Finding::Suspect { cutoff_hz } => {
                write!(f, "content stops at {:.1} kHz: a high-bitrate lossy encoder, or the master", khz(cutoff_hz))
            }
            Finding::Upsampled { above_hz } => {
                write!(f, "nothing real above {:.1} kHz: resampled from a lower rate", khz(above_hz))
            }
            Finding::Padded { bits, effective_bits } => {
                write!(f, "{bits}-bit file holding {effective_bits}-bit samples")
            }
            Finding::DecodeErrors { lines } => write!(f, "FFmpeg reported {lines} error line(s) decoding it"),
            Finding::Inconclusive => write!(f, "too short or too quiet to judge"),
        }
    }
}

/// At 88.2 kHz and up, a wall below this is a resampler's, not a converter's: the content came from
/// 44.1 or 48 kHz. FFmpeg's own resampler leaves its images reaching to about 26 kHz.
pub const UPSAMPLED_BELOW_HZ: u32 = 26_500;
/// Walls below this are lossy encoders' (LAME's 256 kbit/s lowpass is about 19.7 kHz).
pub const LOSSY_BELOW_HZ: u32 = 19_500;
/// Walls from `LOSSY_BELOW_HZ` up to this are suspect; above, converters' own filters live.
pub const SUSPECT_BELOW_HZ: u32 = 21_000;
/// A suspect wall with at least this share of gated frames is called lossy. Quiet treble alone can
/// gate a third of frames, so the bar is high.
pub const HOLES_LOSSY: f32 = 0.35;
/// Gating only promotes a wall this low or lower: LAME and Opus stop at 20-20.5 kHz, while walls
/// just under Nyquist are where masters and converters end anyway.
pub const HOLES_PROMOTE_BELOW_HZ: u32 = 20_500;
/// At 88.2 kHz and up, the 28 kHz-and-up region this far under the 16-20 kHz level holds nothing a
/// real recording put there.
pub const ULTRA_EMPTY_DB: f32 = 45.0;

pub fn findings(m: &Measurement) -> Vec<Finding> {
    let mut out = Vec::new();
    if m.decode_errors > 0 {
        out.push(Finding::DecodeErrors { lines: m.decode_errors });
    }
    if m.effective_bits > 0 && m.bits > 16 && m.effective_bits <= 16 {
        out.push(Finding::Padded { bits: m.bits, effective_bits: m.effective_bits });
    }
    if m.effective_bits == 0 || m.frames < 8 {
        out.push(Finding::Inconclusive);
        return out;
    }
    if m.rate >= 88_200 {
        // Either the content ends in a wall no converter would make, or the ultrasonic region is empty.
        let walled = m.cutoff_hz.filter(|&c| c <= UPSAMPLED_BELOW_HZ);
        let empty = m.ultra_gap_db.is_some_and(|g| g >= ULTRA_EMPTY_DB).then_some(28_000);
        if let Some(above_hz) = walled.or(empty) {
            out.push(Finding::Upsampled { above_hz });
        }
    }
    if let Some(cutoff_hz) = m.cutoff_hz {
        let gated = cutoff_hz < HOLES_PROMOTE_BELOW_HZ && m.holes.is_some_and(|h| h >= HOLES_LOSSY);
        if cutoff_hz < LOSSY_BELOW_HZ || gated {
            out.push(Finding::Lossy { cutoff_hz });
        } else if cutoff_hz < SUSPECT_BELOW_HZ {
            out.push(Finding::Suspect { cutoff_hz });
        }
    }
    out
}

/// Decode `track` with FFmpeg at its own rate and depth, and measure it.
pub fn measure(ffmpeg: &Path, track: &Path) -> Result<Measurement, String> {
    let mut f = BufReader::new(File::open(track).map_err(|e| e.to_string())?);
    let layout = flac::read_layout(&mut f).map_err(|e| format!("not a FLAC file: {e}"))?;
    drop(f);
    let info = stream_info(&layout.blocks[0].data).ok_or("the STREAMINFO block is malformed")?;
    let mut decode = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(track)
        .args(["-vn", "-map", "0:a:0", "-f", "s32le", "-acodec", "pcm_s32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start FFmpeg: {e}"))?;
    let mut stderr = decode.stderr.take().expect("piped");
    let errors = thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let mut analyser = Analyser::new(info);
    let mut out = decode.stdout.take().expect("piped");
    let mut buf = vec![0u8; 1 << 16];
    loop {
        match out.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => analyser.feed(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("reading FFmpeg's output: {e}")),
        }
    }
    let status = decode.wait().map_err(|e| e.to_string())?;
    let errors = errors.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("FFmpeg could not decode it: {}", errors.trim()));
    }
    Ok(analyser.finish(errors.lines().filter(|l| !l.trim().is_empty()).count() as u32))
}

/// Stored measurements, by content key: `checks.tsv` in the cache directory.
pub struct Store {
    path: PathBuf,
    rows: HashMap<String, (Measurement, String)>,
}

const HEADER: &str = "# flint lossless check, method";

impl Store {
    pub fn open(dir: &Path) -> io::Result<Store> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("checks.tsv");
        let mut rows = HashMap::new();
        if let Ok(f) = File::open(&path) {
            let mut lines = BufReader::new(f).lines();
            if lines.next().transpose()?.as_deref() == Some(format!("{HEADER} {METHOD}").as_str()) {
                for line in lines {
                    let line = line?;
                    let c: Vec<&str> = line.splitn(13, '\t').collect();
                    if c.len() != 13 {
                        continue;
                    }
                    let parsed = (|| {
                        Some(Measurement {
                            rate: c[1].parse().ok()?,
                            channels: c[2].parse().ok()?,
                            bits: c[3].parse().ok()?,
                            effective_bits: c[4].parse().ok()?,
                            seconds: c[5].parse().ok()?,
                            frames: c[6].parse().ok()?,
                            cutoff_hz: if c[7].is_empty() { None } else { Some(c[7].parse().ok()?) },
                            wall_db: c[8].parse().ok()?,
                            holes: if c[9].is_empty() { None } else { Some(c[9].parse().ok()?) },
                            decode_errors: c[10].parse().ok()?,
                            ultra_gap_db: if c[11].is_empty() { None } else { Some(c[11].parse().ok()?) },
                            ..Measurement::default()
                        })
                    })();
                    if let Some(m) = parsed {
                        rows.insert(c[0].to_string(), (m, c[12].to_string()));
                    }
                }
            }
        }
        Ok(Store { path, rows })
    }

    pub fn get(&self, key: &str) -> Option<&Measurement> {
        self.rows.get(key).map(|(m, _)| m)
    }

    pub fn put(&mut self, key: &str, m: Measurement, path: &str) {
        self.rows.insert(key.to_string(), (Measurement { spectrum: Vec::new(), floor_db: 0.0, ..m }, path.to_string()));
    }

    pub fn save(&self) -> io::Result<()> {
        let mut keys: Vec<&String> = self.rows.keys().collect();
        keys.sort();
        let mut out = format!("{HEADER} {METHOD}\n");
        for k in keys {
            let (m, path) = &self.rows[k];
            out.push_str(&format!(
                "{k}\t{}\t{}\t{}\t{}\t{:.2}\t{}\t{}\t{:.1}\t{}\t{}\t{}\t{}\n",
                m.rate,
                m.channels,
                m.bits,
                m.effective_bits,
                m.seconds,
                m.frames,
                m.cutoff_hz.map(|c| c.to_string()).unwrap_or_default(),
                m.wall_db,
                m.holes.map(|h| format!("{h:.3}")).unwrap_or_default(),
                m.decode_errors,
                m.ultra_gap_db.map(|g| format!("{g:.1}")).unwrap_or_default(),
                clean(path)
            ));
        }
        write_atomic(&self.path, out.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(rate: u32, channels: u8, bits: u8, samples: u64) -> StreamInfo {
        StreamInfo { rate, channels, bits, samples }
    }

    /// Deterministic white noise in -1..1.
    fn noise(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

    /// Feed `seconds` of noise through `shape`, which filters one sample (and may keep state).
    fn run(i: StreamInfo, seconds: f64, shift: u32, mut shape: impl FnMut(f64) -> f64) -> Measurement {
        let mut a = Analyser::new(i);
        let mut seed = 0x9e37_79b9_7f4a_7c15;
        let mut bytes = Vec::new();
        for _ in 0..(seconds * f64::from(i.rate)) as usize {
            let v = shape(noise(&mut seed) * 0.25);
            let s = ((v * f64::from(1u32 << (31 - shift))) as i32) << shift;
            for _ in 0..i.channels {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
        }
        // Odd-sized pieces, so the carry path is exercised.
        for piece in bytes.chunks(4093) {
            a.feed(piece);
        }
        a.finish(0)
    }

    /// Windowed-sinc lowpass FIR at `cutoff` Hz: a wall like an encoder's.
    fn lowpass(rate: u32, cutoff: f64) -> impl FnMut(f64) -> f64 {
        let taps = 511;
        let fc = cutoff / f64::from(rate);
        let h: Vec<f64> = (0..taps)
            .map(|i| {
                let x = i as f64 - (taps / 2) as f64;
                let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * x).sin() / (PI * x) };
                let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / (taps - 1) as f64).cos()
                    + 0.08 * (4.0 * PI * i as f64 / (taps - 1) as f64).cos();
                sinc * w
            })
            .collect();
        let mut hist = vec![0.0; taps];
        let mut pos = 0;
        move |x| {
            hist[pos] = x;
            pos = (pos + 1) % taps;
            (0..taps).map(|k| h[k] * hist[(pos + k) % taps]).sum()
        }
    }

    #[test]
    fn streaminfo_fields_decode() {
        // 44100 Hz, 2 channels, 16 bits, 0x1_0000_0001 samples.
        let mut b = vec![0u8; 34];
        b[10] = 0x0a;
        b[11] = 0xc4;
        b[12] = 0x42;
        b[13] = 0xf1;
        b[17] = 0x01;
        assert_eq!(stream_info(&b), Some(info(44_100, 2, 16, 0x1_0000_0001)));
        // 96000 Hz, 1 channel (the field is channels - 1, so zero), 24 bits.
        let v: u64 = (96_000 << 44) | (23 << 36) | 12345;
        b[10..18].copy_from_slice(&v.to_be_bytes());
        assert_eq!(stream_info(&b), Some(info(96_000, 1, 24, 12345)));
    }

    #[test]
    fn fft_puts_a_sine_in_its_bin() {
        let n = 1024;
        let mut fft = Fft::new(n);
        let frame: Vec<f32> = (0..n).map(|i| (2.0 * PI * 100.0 * i as f64 / n as f64).sin() as f32).collect();
        let mut out = vec![0.0; n / 2];
        fft.power(&frame, &mut out);
        let peak = (0..n / 2).max_by(|&a, &b| out[a].total_cmp(&out[b])).unwrap();
        assert_eq!(peak, 100);
        assert!(out[100] > 1e4 * out[110], "leakage too high");
    }

    #[test]
    fn full_band_noise_has_no_wall() {
        let m = run(info(44_100, 2, 16, 441_000), 10.0, 16, |x| x);
        assert_eq!(m.cutoff_hz, None, "{m:?}");
        assert_eq!(m.effective_bits, 16);
        assert!(findings(&m).is_empty());
    }

    #[test]
    fn a_16_khz_wall_is_lossy() {
        let m = run(info(44_100, 2, 16, 441_000), 10.0, 16, lowpass(44_100, 16_000.0));
        let c = m.cutoff_hz.expect("no wall found");
        assert!((15_500..=16_500).contains(&c), "cutoff {c}");
        assert_eq!(findings(&m), vec![Finding::Lossy { cutoff_hz: c }]);
    }

    #[test]
    fn a_20_khz_wall_is_suspect_and_upsampled_audio_is_named() {
        let m = run(info(44_100, 1, 16, 441_000), 10.0, 16, lowpass(44_100, 20_000.0));
        let c = m.cutoff_hz.expect("no wall found");
        assert!(findings(&m).contains(&Finding::Suspect { cutoff_hz: c }), "{:?}", findings(&m));
        // 16-bit samples in a 24-bit file at 96 kHz, with nothing above 21.5 kHz: a CD, upsampled.
        let m = run(info(96_000, 1, 24, 960_000), 10.0, 16, lowpass(96_000, 21_500.0));
        let f = findings(&m);
        assert!(f.iter().any(|x| matches!(x, Finding::Upsampled { .. })), "{f:?} floor {:.0}", m.floor_db);
        // A 96 kHz file with real content up to 40 kHz is not called upsampled.
        let m = run(info(96_000, 1, 24, 960_000), 10.0, 8, lowpass(96_000, 40_000.0));
        assert_eq!(findings(&m), vec![], "{:?}", m.ultra_gap_db);
        assert!(f.contains(&Finding::Padded { bits: 24, effective_bits: 16 }), "{f:?}");
    }

    #[test]
    fn padded_24_bit_is_found() {
        let m = run(info(44_100, 2, 24, 441_000), 10.0, 16, |x| x);
        assert_eq!(m.effective_bits, 16);
        assert!(findings(&m).contains(&Finding::Padded { bits: 24, effective_bits: 16 }));
        let m = run(info(44_100, 2, 24, 441_000), 10.0, 8, |x| x);
        assert_eq!(m.effective_bits, 24);
        assert!(findings(&m).is_empty());
    }

    #[test]
    fn silence_and_short_files_are_inconclusive() {
        let m = run(info(44_100, 2, 16, 44_100), 1.0, 16, |_| 0.0);
        assert_eq!(findings(&m), vec![Finding::Inconclusive]);
    }

    #[test]
    fn the_store_round_trips_and_drops_another_method() {
        let d = std::env::temp_dir().join(format!("flint-checks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut s = Store::open(&d).unwrap();
        let m = Measurement {
            rate: 44_100,
            channels: 2,
            bits: 16,
            effective_bits: 16,
            seconds: 201.5,
            frames: 1085,
            cutoff_hz: Some(16_100),
            wall_db: 48.3,
            holes: Some(0.25),
            decode_errors: 0,
            ultra_gap_db: Some(31.5),
            floor_db: -50.0,
            spectrum: vec![1.0],
        };
        s.put("flac-ab-1", m.clone(), "C:\\Music\\a\tb.flac");
        s.save().unwrap();
        let back = Store::open(&d).unwrap();
        assert_eq!(back.get("flac-ab-1"), Some(&Measurement { spectrum: Vec::new(), floor_db: 0.0, ..m }));
        let text = std::fs::read_to_string(d.join("checks.tsv")).unwrap();
        std::fs::write(d.join("checks.tsv"), text.replace(&format!("method {METHOD}"), "method 0")).unwrap();
        assert!(Store::open(&d).unwrap().get("flac-ab-1").is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
