//! flint — the command-line tool.
//!
//! Commands:
//!
//! ```text
//! flint scan <library> [--jobs N] [--cache dir]                      analyse every track once, into the cache
//! flint check <library | file.flac> [--jobs N] [--all] [--verbose]    look for fake FLACs (lossy sources, upsampling)
//! flint analyse <track> [--out result.smfmf] [--param id=value]   run Sony's engine, summarise
//! flint tag-copy <src> <dst> [--smfmf result.smfmf]                 copy with SensMe data (FLAC, MP3)
//! flint sync <library> --to <volume> [--to <volume>] [--apply]   copy the library to the player
//! flint inspect <file.flac | file.mp3 | result.smfmf>             show blocks / frames / chunks
//! flint import [--from <Music Center data dir>]                   take analysis Music Center has already done
//! flint lastfm key|login|status                                   the Last.fm account, once
//! flint scrobble <volume> [--apply]                               send the plays in .scrobbler.log
//! flint likes <volume> [--apply] [--playlist]                     liked songs: player <-> Last.fm
//! flint stats <volume> [--to <sd card>] [--apply]                 seed the player's play counts from its scrobble log
//! flint gui                                                       open the window (Windows)
//! flint gui-preview <out.svg> [--state name]                      draw the window to an SVG, anywhere
//! ```

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use flint_core::{
    apply, cache, engine, engine::Engine, flac, id3, lastfm, lastfm_sync, library, likes, lossless, musiccenter,
    playlists, smfmf, space, stats, sync, views,
};

const USAGE: &str = "\
usage:
  flint scan <library folder> [--jobs N] [--cache dir]
  flint check <library folder | file.flac> [--jobs N] [--cache dir] [--all] [--verbose]
  flint analyse <track> [--out result.smfmf] [--param id=value]...
  flint tag-copy <src.flac|src.mp3> <dst> [--smfmf result.smfmf] [--param id=value]...
  flint sync <library folder> --to <volume> [--to <volume>] [--gb N]... [--playlists <folder>] [--apply] [--no-sensme] [--no-extras]
  flint inspect <file.flac | file.mp3 | result.smfmf>
  flint import [--from <Music Center data folder>] [--cache dir]
  flint lastfm key <api-key> <api-secret> | login <username> | status
  flint scrobble <volume> [<volume>...] [--apply]
  flint likes <volume> [<volume>...] [--apply] [--playlist]
  flint stats <volume> [--to <sd card>] [--apply]
  flint playlists <volume> [--to <PC folder>] [--library <library folder>] [--apply]
  flint gui [--dark | --light]
  flint gui-preview <out.svg> [--state fresh|ready|planned|working|done|scanning|player|check|filtered|sensme|likes|palettes|palette-new|palette-shop|settings|signed-in] [--dark]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("scan") => scan(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("sync") => sync_cmd(&args[1..]),
        Some("analyse" | "analyze") => analyse(&args[1..]),
        Some("tag-copy") => tag_copy(&args[1..]),
        Some("inspect") => inspect(&args[1..]),
        Some("import") => import(&args[1..]),
        Some("lastfm") => lastfm_cmd(&args[1..]),
        Some("scrobble") => scrobble_cmd(&args[1..]),
        Some("likes") => likes_cmd(&args[1..]),
        Some("stats") => stats_cmd(&args[1..]),
        Some("playlists") => playlists_cmd(&args[1..]),
        Some("gui") | Some("--gui") => opts(&args[1..]).and_then(|o| gui(o.dark)),
        Some("gui-preview") => gui_preview(&args[1..]),
        // Double-clicked on Windows, where there is no terminal to read the usage in: a window is
        // the only thing that can be shown, so show it. Anywhere else, with no arguments, the
        // usage is exactly what is wanted.
        None if cfg!(windows) => gui(None),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flint: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Positional arguments, plus the values of `--out`, `--smfmf` and every `--param`.
struct Opts {
    pos: Vec<String>,
    out: Option<PathBuf>,
    smfmf: Option<PathBuf>,
    cache: Option<PathBuf>,
    jobs: Option<usize>,
    params: Vec<String>,
    all: bool,
    verbose: bool,
    to: Vec<PathBuf>,
    gb: Vec<f64>,
    playlists: Option<PathBuf>,
    apply: bool,
    no_sensme: bool,
    /// Leave cover art and lyrics on the PC (the window's "extras" switch, off).
    no_extras: bool,
    from: Option<PathBuf>,
    state: Option<String>,
    /// `--dark` / `--light`; `None` means follow Windows.
    dark: Option<bool>,
    /// `--playlist`: also write `Liked Songs.m3u8`, which costs a tag read per file on the player.
    playlist: bool,
    /// `--library`: the PC's music folder, for `flint playlists` to name files in.
    library: Option<PathBuf>,
}

fn opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        pos: Vec::new(),
        out: None,
        smfmf: None,
        cache: None,
        jobs: None,
        params: Vec::new(),
        all: false,
        verbose: false,
        to: Vec::new(),
        gb: Vec::new(),
        playlists: None,
        apply: false,
        no_sensme: false,
        no_extras: false,
        from: None,
        state: None,
        dark: None,
        playlist: false,
        library: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--out" => o.out = Some(value("--out")?.into()),
            "--smfmf" => o.smfmf = Some(value("--smfmf")?.into()),
            "--param" => o.params.push(value("--param")?),
            "--cache" => o.cache = Some(value("--cache")?.into()),
            "--all" => o.all = true,
            "--to" => o.to.push(value("--to")?.into()),
            "--gb" => o.gb.push(value("--gb")?.parse().map_err(|_| "--gb needs a number".to_string())?),
            "--playlists" => o.playlists = Some(value("--playlists")?.into()),
            "--from" => o.from = Some(value("--from")?.into()),
            "--library" => o.library = Some(value("--library")?.into()),
            "--state" => o.state = Some(value("--state")?),
            "--playlist" => o.playlist = true,
            "--dark" => o.dark = Some(true),
            "--light" => o.dark = Some(false),
            "--apply" => o.apply = true,
            "--no-sensme" => o.no_sensme = true,
            "--no-extras" => o.no_extras = true,
            "--verbose" => o.verbose = true,
            "--jobs" => o.jobs = Some(value("--jobs")?.parse().map_err(|_| "--jobs needs a number".to_string())?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}\n{USAGE}")),
            _ => o.pos.push(a.clone()),
        }
    }
    Ok(o)
}

fn print_summary(bytes: &[u8]) -> Result<(), String> {
    let s = smfmf::summarise(bytes).map_err(|e| e.to_string())?;
    let chunks: Vec<String> = s.chunks.iter().map(|(n, len)| format!("{n}({len})")).collect();
    println!("  smfmf   {} bytes: {}", s.bytes, chunks.join(" "));
    if let Some(bpm) = s.bpm {
        println!("  tempo   {bpm:.2} BPM");
    }
    if !s.unknown.is_empty() {
        println!("  WARNING chunks the Walkman does not know: {}", s.unknown.join(", "));
    }
    Ok(())
}

fn run_engine(track: &Path, params: &[String]) -> Result<Vec<u8>, String> {
    let mut engine = Engine::locate()?;
    engine.params = params.to_vec();
    let a = engine.analyse(track)?;
    for key in ["engine", "frames", "feed_ms", "result88"] {
        if let Some((_, v)) = a.info.iter().find(|(k, _)| k == key) {
            println!("  {key:<9}{v}");
        }
    }
    Ok(a.smfmf)
}

fn analyse(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [track] = o.pos.as_slice() else { return Err(USAGE.into()) };
    println!("{track}");
    let bytes = run_engine(Path::new(track), &o.params)?;
    print_summary(&bytes)?;
    if let Some(out) = o.out {
        fs::write(&out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
        println!("  wrote   {}", out.display());
    }
    Ok(())
}

fn tag_copy(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [src, dst] = o.pos.as_slice() else { return Err(USAGE.into()) };
    let (src, dst) = (Path::new(src), Path::new(dst));
    if src == dst || fs::canonicalize(src).ok().zip(fs::canonicalize(dst).ok()).is_some_and(|(a, b)| a == b) {
        return Err("tag-copy never writes the source: give a different destination".into());
    }
    let payload = match &o.smfmf {
        Some(p) => fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?,
        None => match cached_result(src, o.cache.as_deref()) {
            Some(bytes) => {
                println!("  (from the analysis cache)");
                bytes
            }
            None => run_engine(src, &o.params)?,
        },
    };
    smfmf::parse(&payload).map_err(|e| format!("refusing to write malformed SMFMF: {e}"))?;
    let (source_len, copy_len, in_place, replaced, container) = match kind(src)? {
        Kind::Flac => {
            let r = flac::copy_with_smfm(src, dst, &payload).map_err(|e| format!("{}: {e}", src.display()))?;
            (r.source_len, r.copy_len, r.audio_in_place, r.replaced, "FLAC APPLICATION block SMFM")
        }
        Kind::Mp3 => {
            let r = id3::copy_with_smfmf(src, dst, &payload).map_err(|e| format!("{}: {e}", src.display()))?;
            let what = if r.created_tag { "new ID3v2.3 tag, GEOB USR_SMFMF" } else { "ID3v2 GEOB USR_SMFMF" };
            (r.source_len, r.copy_len, r.audio_in_place, r.replaced, what)
        }
    };
    println!("{} -> {}", src.display(), dst.display());
    print_summary(&payload)?;
    println!(
        "  size    {source_len} -> {copy_len} bytes, {container}{}{}",
        if in_place { " (padding absorbed it; audio not moved)" } else { " (audio moved)" },
        if replaced { ", replaced existing SensMe data" } else { "" }
    );
    Ok(())
}

/// The cached result for `track`'s audio, if a scan has seen it.
fn cached_result(track: &Path, dir: Option<&Path>) -> Option<Vec<u8>> {
    let c = cache::Cache::open(&dir.map(Path::to_path_buf).unwrap_or_else(cache::default_dir)).ok()?;
    let key = cache::content_key(track).ok()??;
    c.get(&key)?;
    c.blob(&key).ok()
}

/// Take the analysis Sony's Music Center has already done and put it in Flint's cache.
///
/// Nothing is written to Music Center's files or to the music library: this reads Music Center's own
/// per-track cache and keys each result against the audio it belongs to, so a later `scan` or `sync`
/// finds it and never runs the engine for that track. The compacting is where the size goes: what
/// Flint keeps is the part the player actually reads.
fn import(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    if !o.pos.is_empty() {
        return Err(USAGE.into());
    }
    let mc = match o.from.clone().or_else(musiccenter::data_dir) {
        Some(d) => d,
        None => {
            return Err("Music Center for PC was not found. Point at its data folder with \
                        --from \"%APPDATA%\\Sony\\Music Center\"."
                .to_string())
        }
    };
    if !mc.is_dir() {
        return Err(format!("{}: not a folder", mc.display()));
    }
    let dir = o.cache.clone().unwrap_or_else(cache::default_dir);
    let mut c = cache::Cache::open(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    println!("reading {}  (cache {})", mc.display(), dir.display());
    let mut saved: u64 = 0;
    let report = musiccenter::import(&mc, &mut c, |path, was, now| {
        saved += (was - now) as u64;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!("  {was:>8} -> {now:<6} bytes  {name}");
    })
    .map_err(|e| e.to_string())?;
    c.save().map_err(|e| e.to_string())?;
    println!(
        "{} analyses in Music Center's cache: {} imported, {} already known, {} whose track could not be found",
        report.cached, report.imported, report.already, report.missing,
    );
    if report.cached > report.mapped {
        println!(
            "  {} could not be matched to a file — Music Center's own index did not name one",
            report.cached - report.mapped
        );
    }
    if saved > 0 {
        println!("  {} of chunks the player never reads were left behind", space::human(saved));
    }
    Ok(())
}

fn scan(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [root] = o.pos.as_slice() else { return Err(USAGE.into()) };
    let dir = o.cache.clone().unwrap_or_else(cache::default_dir);
    let mut c = cache::Cache::open(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // Each job is an FFmpeg decode plus the engine, so half the cores keeps the PC usable.
    let jobs = o.jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)));
    let engine = match Engine::locate() {
        Ok(mut e) => {
            e.params = o.params.clone();
            Some(e)
        }
        Err(why) => {
            eprintln!("flint: {why}\nflint: scanning without SensMe analysis: only already-cached tracks count.");
            None
        }
    };
    println!("scanning {}  (cache {}, {} jobs)", root, dir.display(), jobs);
    let report = library::scan(Path::new(root), &mut c, engine.as_ref(), jobs, |ev| match ev {
        library::Event::Planned { total, cached, queued, skipped, adopted } => {
            println!("{total} tracks: {cached} already analysed, {queued} to analyse, {skipped} skipped");
            if *adopted > 0 {
                println!("  {adopted} carried Sony's analysis already — taken from the files, not re-run");
            }
        }
        library::Event::Analysed { done, queued, path, ms, bpm } => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let bpm = bpm.map(|b| format!("{b:6.1} BPM")).unwrap_or_default();
            println!("[{done}/{queued}] {ms:>5} ms {bpm}  {name}");
        }
        library::Event::Failed { done, queued, path, error } => {
            println!("[{done}/{queued}] FAILED {}: {error}", path.display());
        }
    })
    .map_err(|e| e.to_string())?;
    println!(
        "done in {:.0} s: {} analysed, {} taken from the files, {} cached, {} failed, {} skipped",
        report.seconds, report.analysed, report.adopted, report.cached, report.failed, report.skipped
    );
    Ok(())
}

enum Kind {
    Flac,
    Mp3,
}

/// FLAC if it parses as FLAC (an ID3v2 tag in front is allowed); MP3 if it starts with an ID3v2 tag
/// or an MPEG frame sync. Anything else is refused rather than guessed at.
fn kind(path: &Path) -> Result<Kind, String> {
    let mut r = BufReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
    if flac::read_layout(&mut r).is_ok() {
        return Ok(Kind::Flac);
    }
    let mut head = [0u8; 3];
    let head = std::io::Read::read_exact(&mut File::open(path).map_err(|e| e.to_string())?, &mut head).map(|_| head);
    match head {
        Ok(h) if &h == b"ID3" || (h[0] == 0xff && h[1] & 0xe0 == 0xe0) => Ok(Kind::Mp3),
        _ => Err(format!("{}: neither FLAC nor MP3", path.display())),
    }
}

fn inspect(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [path] = o.pos.as_slice() else { return Err(USAGE.into()) };
    let path = Path::new(path);
    let mut r = BufReader::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
    match flac::read_layout(&mut r) {
        Ok(layout) => {
            println!("{}  (audio at byte {})", path.display(), layout.audio_offset);
            if !layout.prefix.is_empty() {
                println!("  ID3v2 prefix, {} bytes", layout.prefix.len());
            }
            for b in &layout.blocks {
                let name = match b.kind {
                    0 => "STREAMINFO",
                    1 => "PADDING",
                    2 => "APPLICATION",
                    3 => "SEEKTABLE",
                    4 => "VORBIS_COMMENT",
                    5 => "CUESHEET",
                    6 => "PICTURE",
                    _ => "reserved",
                };
                let app = if b.kind == 2 && b.data.len() >= 4 {
                    format!(" id {}", String::from_utf8_lossy(&b.data[..4]))
                } else {
                    String::new()
                };
                println!("  {name:<15}{:>9} bytes{app}", b.data.len());
            }
            if let Some(p) = layout.smfm() {
                print_summary(p)?;
            }
            Ok(())
        }
        Err(flac::Error::NotFlac) if matches!(kind(path), Ok(Kind::Mp3)) => {
            let mut f = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
            let tag = id3::read_tag(&mut f).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("{}", path.display());
            match tag {
                None => println!("  no ID3v2 tag"),
                Some(t) => {
                    println!(
                        "  ID3v2.{}.{}  {} bytes (audio at byte {})",
                        t.major, t.revision, t.total_len, t.total_len
                    );
                    for fr in &t.frames {
                        println!(
                            "  {}  {:>8} bytes{}",
                            String::from_utf8_lossy(&fr.id),
                            fr.body.len(),
                            if fr.is_smfmf() { "  SensMe (USR_SMFMF)" } else { "" }
                        );
                    }
                    if let Some(p) = t.smfmf() {
                        print_summary(p)?;
                    }
                }
            }
            Ok(())
        }
        Err(flac::Error::NotFlac) => {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            println!("{}", path.display());
            print_summary(&bytes)
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn check(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [root] = o.pos.as_slice() else { return Err(USAGE.into()) };
    let root = Path::new(root);
    let dir = o.cache.clone().unwrap_or_else(cache::default_dir);
    let mut store = lossless::Store::open(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let ffmpeg = engine::locate_ffmpeg()?;
    let jobs = o.jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)));
    let single = root.is_file();
    if !single {
        println!("checking {}  (results in {}, {} jobs)", root.display(), dir.join("checks.tsv").display(), jobs);
    }
    let mut failed = 0;
    let checked = library::check(root, &mut store, &ffmpeg, jobs, |ev| match ev {
        library::CheckEvent::Planned { total, stored, queued, skipped } if !single => {
            println!("{total} FLACs: {stored} already checked, {queued} to check, {skipped} skipped");
        }
        library::CheckEvent::Measured { done, queued, path, measurement } if !single => {
            let f = lossless::findings(measurement);
            let label = f.first().map_or("ok", |f| f.label());
            println!("[{done}/{queued}] {label:<9} {}", name(path));
        }
        library::CheckEvent::Failed { done, queued, path, error } => {
            failed += 1;
            println!("[{done}/{queued}] FAILED {}: {error}", path.display());
        }
        _ => {}
    })
    .map_err(|e| e.to_string())?;

    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    let mut clean = 0;
    let mut flagged = Vec::new();
    for c in &checked {
        let f = lossless::findings(&c.measurement);
        if f.is_empty() {
            clean += 1;
        }
        for x in &f {
            *counts.entry(x.label()).or_default() += 1;
        }
        if single || o.all || !f.is_empty() {
            flagged.push((c, f));
        }
    }
    if !single && !flagged.is_empty() {
        println!();
    }
    for (c, f) in &flagged {
        let m = &c.measurement;
        println!("{}", c.path.display());
        println!(
            "  {} Hz, {} ch, {}-bit (uses {}), {:.0} s{}",
            m.rate,
            m.channels,
            m.bits,
            m.effective_bits,
            m.seconds,
            match (m.cutoff_hz, m.holes) {
                (Some(c), Some(h)) => format!(
                    ", wall at {:.1} kHz ({:.0} dB), top band gated in {:.0}% of loud frames",
                    f64::from(c) / 1000.0,
                    m.wall_db,
                    h * 100.0
                ),
                (Some(c), None) => format!(", wall at {:.1} kHz ({:.0} dB)", f64::from(c) / 1000.0, m.wall_db),
                _ => ", no lowpass wall".to_string(),
            }
        );
        if f.is_empty() {
            println!("  ok        no sign of a lossy source, upsampling or padding");
        }
        for x in f {
            println!("  {:<9} {x}", x.label());
        }
        if o.verbose {
            print_spectrum(m);
        }
    }
    if single && o.verbose && checked.iter().all(|c| c.measurement.spectrum.is_empty()) {
        println!("  (stored result; delete its line from checks.tsv to see the spectrum)");
    }
    let summary: Vec<String> = counts.iter().map(|(k, v)| format!("{v} {}", k.to_lowercase())).collect();
    println!(
        "\n{} FLACs: {clean} clean{}{}",
        checked.len(),
        if summary.is_empty() { String::new() } else { format!(", {}", summary.join(", ")) },
        if failed > 0 { format!("; {failed} could not be read") } else { String::new() }
    );
    println!("A clean result is not proof: it means none of the usual signs of a fake.");
    Ok(())
}

fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Level per kHz from 10 kHz up, relative to the 2-8 kHz average, as a bar.
fn print_spectrum(m: &lossless::Measurement) {
    if !m.spectrum.is_empty() {
        println!("  noise floor {:.0} dB under the 2-8 kHz level", -m.floor_db);
    }
    if std::env::var_os("FLINT_BANDS").is_some() {
        let from = (14_000 / lossless::BAND_HZ) as usize;
        let row: Vec<String> = m.spectrum.iter().skip(from).map(|d| format!("{d:.0}")).collect();
        println!("  bands from 14 kHz, 100 Hz each: {}", row.join(" "));
        return;
    }
    let per_khz = (1000 / lossless::BAND_HZ) as usize;
    for (i, chunk) in m.spectrum.chunks(per_khz).enumerate().skip(10) {
        let db = chunk.iter().sum::<f32>() / chunk.len() as f32;
        let bar = "#".repeat(((db + 100.0).max(0.0) / 2.5) as usize);
        println!("  {:>3} kHz {:>6.1} dB {bar}", i, db);
    }
}

fn sync_cmd(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [library] = o.pos.as_slice() else { return Err(USAGE.into()) };
    let library = Path::new(library);
    if o.to.is_empty() {
        return Err("give at least one destination: --to E:\\ (add a second --to for the card)".into());
    }
    let cache_dir = o.cache.clone().unwrap_or_else(cache::default_dir);
    let mut analysis = cache::Cache::open(&cache_dir).map_err(|e| format!("{}: {e}", cache_dir.display()))?;

    println!("reading {}", library.display());
    let budget_gb: Vec<Option<f64>> = o.gb.iter().copied().map(Some).collect();
    let req = sync::Request {
        library,
        volumes: &o.to,
        budget_gb: &budget_gb,
        playlists: o.playlists.as_deref(),
        sensme: !o.no_sensme,
        extras: !o.no_extras,
    };
    let p = sync::prepare(&req, &mut analysis, &mut |n| match n {
        sync::Note::Library { files, bytes } => println!("  {files} tracks, {}", space::human(bytes)),
        sync::Note::Adopted { tracks, saved } => println!(
            "  {tracks} already carried Sony's analysis — taken from the files{}",
            if saved > 0 {
                format!(", {} of chunks the player never reads left behind", space::human(saved))
            } else {
                String::new()
            }
        ),
        sync::Note::AdoptFailed(e) => eprintln!("flint: reading existing SensMe tags: {e}"),
        sync::Note::Volume { root, on_device, files, budget, .. } => println!(
            "  {} holds {} in {files} files, budget {}",
            root.display(),
            space::human(on_device),
            space::human(budget)
        ),
        sync::Note::Playlists(n) => println!("  {n} playlists"),
    })?;
    let nothing = p.is_empty();
    let sync::Prepared { plan, volumes, mut manifests, tagged, pending_playlists, .. } = p;
    println!(
        "\nplan: {} to copy ({} tagged with SensMe data), {} to remove, {} playlists to write",
        plan.copies.len(),
        tagged,
        plan.stale_files.len() + plan.stale_playlists.len(),
        pending_playlists
    );
    for (i, v) in volumes.iter().enumerate() {
        let albums = plan.assignments.values().filter(|&&a| a == i).count();
        println!("  {}: {albums} albums, {} to copy", v.root.display(), space::human(plan.bytes_to_copy(i)));
    }
    if !plan.skipped.is_empty() {
        println!("  no room for {} albums: {}", plan.skipped.len(), plan.skipped.join(", "));
    }
    if nothing {
        println!("nothing to do.");
        return Ok(());
    }
    if !o.apply {
        println!("\nthis was a dry run; add --apply to carry it out.");
        // Every removal, because deleting is the one thing --apply cannot take back; the copies
        // only as a sample, with the count of the rest.
        for (v, rel) in &plan.stale_files {
            println!("  would remove  {}/{rel}", volumes[*v].root.display());
        }
        for (v, name) in &plan.stale_playlists {
            println!("  would remove  playlist {name} ({})", volumes[*v].root.display());
        }
        for c in plan.copies.iter().take(10) {
            println!("  would copy    {}{}", c.rel, if c.tag.is_empty() { "" } else { "  +SensMe" });
        }
        if plan.copies.len() > 10 {
            println!("  …and {} more to copy", plan.copies.len() - 10);
        }
        println!(
            "in all: {} to copy, {} to remove",
            plan.copies.len(),
            plan.stale_files.len() + plan.stale_playlists.len()
        );
        return Ok(());
    }

    let out = apply::apply(
        &plan,
        library,
        &volumes,
        &mut manifests,
        |c| analysis.blob(&c.tag).ok(),
        false,
        || false,
        |ev| match ev {
            apply::Event::Copied { done, total, rel, tagged, .. } => {
                println!("[{done}/{total}] {rel}{}", if *tagged { "  +SensMe" } else { "" });
            }
            apply::Event::Removed { rel, .. } => println!("removed {rel}"),
            apply::Event::Playlist { name, tracks, .. } => println!("playlist {name} ({tracks} tracks)"),
            apply::Event::Failed { what, error } => println!("FAILED {what}: {error}"),
        },
    )
    .map_err(|e| e.to_string())?;
    println!(
        "\ndone: {} copied ({}, {} tagged), {} removed, {} playlists{}",
        out.copied,
        space::human(out.bytes),
        out.tagged,
        out.removed,
        out.playlists,
        if out.failed > 0 { format!(", {} failed", out.failed) } else { String::new() }
    );
    // An album that changed volume took its files with it; its ratings and play counts are keyed
    // by where the file was. Not worth failing a finished sync over: say so and carry on.
    let drives: Vec<PathBuf> = o.to.iter().map(|root| stats::drive_root(root)).collect();
    if let Err(e) = stats::follow_player(&drives, true, &mut |l| println!("{l}")) {
        eprintln!("flint: ratings and play counts were not updated: {e}");
    }
    Ok(())
}

// ── the window ─────────────────────────────────────────────────────────────────────────────────

/// Open the window. On anything but Windows this says so rather than pretending: the window is
/// Win32, and `gui-preview` is how it is looked at anywhere else.
#[cfg(windows)]
fn gui(dark: Option<bool>) -> Result<(), String> {
    flint_gui::win32::run_with(dark)
}

#[cfg(not(windows))]
fn gui(_dark: Option<bool>) -> Result<(), String> {
    Err("the window is Windows-only. Every command works here; \
         `flint gui-preview out.svg` draws a picture of the window."
        .into())
}

/// The states worth drawing. Named rather than free-form so the same five pictures come out of
/// every build, which is what makes a diff between two of them mean something.
fn preview_model(state: &str) -> Result<flint_gui::Model, String> {
    use flint_gui::{Job, LibraryFacts, Model, Running, VolumeFacts};
    const GB: u64 = 1024 * 1024 * 1024;
    let mut m = Model::new();
    let ready = |m: &mut Model| {
        m.library = Some(PathBuf::from("D:\\Music"));
        m.volumes[0] = Some(PathBuf::from("E:\\"));
        m.volumes[1] = Some(PathBuf::from("F:\\"));
        m.playlists = Some(PathBuf::from("D:\\Music\\Playlists"));
    };
    // The numbers a real plan sends back, so the preview shows the meters as they will look
    // rather than as empty troughs.
    let measured = |m: &mut Model| {
        m.source = Some(LibraryFacts { files: 3184, bytes: 214 * GB + 614 * GB / 1024 });
        m.dest[0] = Some(VolumeFacts {
            on_device: 12 * GB + 410 * GB / 1024,
            budget: 51 * GB + 717 * GB / 1024,
            to_copy: 39 * GB + 205 * GB / 1024,
            albums: 96,
        });
        m.dest[1] = Some(VolumeFacts {
            on_device: 0,
            budget: 116 * GB + 307 * GB / 1024,
            to_copy: 74 * GB + 922 * GB / 1024,
            albums: 154,
        });
    };
    match state {
        "fresh" => {}
        "ready" => ready(&mut m),
        "planned" => {
            ready(&mut m);
            measured(&mut m);
            m.planned = true;
            m.log = [
                "reading D:\\Music",
                "3,184 files, 215 GB",
                "412 already carried Sony's analysis — taken from the files, 391.2 MB of unread chunks left behind",
                "E:\\ holds 12.4 GB in 214 files, budget 51.7 GB",
                "F:\\ holds 0 B in 0 files, budget 116.3 GB",
                "4 playlists",
                "E:\\: 96 albums, 39.2 GB to copy",
                "F:\\: 154 albums, 74.9 GB to copy",
                "would copy    Aphex Twin - Selected Ambient Works 85-92/01 Xtal.flac  +SensMe",
                "would copy    Aphex Twin - Selected Ambient Works 85-92/02 Tha.flac  +SensMe",
                "would copy    Bicep - Isles/01 Atlas.flac  +SensMe",
                "would remove  E:\\Bonobo - Migration/03 Break Apart.flac",
                "in all: 2,190 to copy, 1 to remove — scroll up to see each one",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            m.status = "Nothing has been written. 2,190 files would be copied — press Copy to the player.".into();
        }
        "working" => {
            ready(&mut m);
            measured(&mut m);
            m.log = [
                "removed E:\\Bonobo - Migration/03 Break Apart.flac",
                "[896/2190] Bicep - Isles/01 Atlas.flac  +SensMe",
                "[897/2190] Bicep - Isles/02 Cazenove.flac  +SensMe",
                "[898/2190] Bicep - Isles/03 Apricots.flac  +SensMe",
                "[899/2190] Bicep - Isles/cover.jpg",
                "[900/2190] Bonobo - Fragments/01 Polyghost.flac  +SensMe",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            m.running = vec![Running {
                job: Job::Apply,
                progress: Some(0.41),
                status: "[900/2190] Bonobo - Fragments/01 Polyghost.flac  +SensMe".into(),
            }];
        }
        "done" => {
            ready(&mut m);
            measured(&mut m);
            m.log = [
                "[2189/2190] Wu-Tang Clan - Enter the Wu-Tang/11 Tearz.flac  +SensMe",
                "[2190/2190] Wu-Tang Clan - Enter the Wu-Tang/12 Wu-Tang - 7th Chamber Pt II.flac  +SensMe",
                "playlist Late night.m3u8 (64 tracks)",
                "playlist Running.m3u8 (31 tracks)",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            m.status = "Copied 2,190 files (114.1 GB), 2,043 tagged, 1 removed, 4 playlists written.".into();
        }
        // The pages behind the other tabs, with the kind of data a real read produces. Titles and
        // counts are placeholders, like every number in these previews.
        "player" | "likes" | "palettes" | "palette-new" | "palette-shop" => {
            use flint_gui::{AlbumRow, PaletteFile, PlayRow, PlayerFacts, PlaylistRow, Tab};
            ready(&mut m);
            let album = |folder: &str, volume: usize, files: usize, gb10: u64, format: &str, by_flint: bool| AlbumRow {
                folder: folder.into(),
                volume,
                files,
                bytes: gb10 * GB / 10,
                format: format.into(),
                by_flint,
                // Placeholders like every number here: a spread of rated and unrated, played and not.
                rating: match files % 4 {
                    0 => Some(5),
                    1 => Some(4),
                    2 => None,
                    _ => Some(3),
                },
                plays: (files as u32 * 7) % 40,
            };
            let play = |when: i64, track: &str, artist: &str, kind: &str| PlayRow {
                when,
                track: track.into(),
                artist: artist.into(),
                kind: kind.into(),
            };
            m.player = PlayerFacts {
                read: true,
                albums: vec![
                    album("Aphex Twin/Selected Ambient Works 85-92", 1, 13, 6, "FLAC", true),
                    album("Benjamin Francis Leftwich/After the Rain", 0, 10, 9, "FLAC", true),
                    album("Bicep/Isles", 1, 12, 6, "FLAC", true),
                    album("Bonobo/Fragments", 0, 12, 1, "AAC", true),
                    album("Burial/Untrue", 1, 13, 5, "FLAC", false),
                    album("Four Tet/Three", 1, 8, 4, "FLAC", true),
                    album("Nick Drake/Pink Moon", 0, 11, 8, "FLAC", true),
                    album("Radiohead/Kid A", 0, 10, 1, "MP3", false),
                ],
                plays: vec![
                    play(1_790_151_240, "Atlas Hands", "Benjamin Francis Leftwich", "PLAY"),
                    play(1_790_151_000, "Box of Stones", "Benjamin Francis Leftwich", "PLAY"),
                    play(1_790_109_660, "Xtal", "Aphex Twin", "PLAY"),
                    play(1_790_109_360, "Tha", "Aphex Twin", "PLAY"),
                    play(1_790_096_520, "Atlas", "Bicep", "PLAY"),
                    play(1_790_096_280, "Glue", "Bicep", "SKIP"),
                    play(1_790_033_400, "Pink Moon", "Nick Drake", "PLAY"),
                ],
                unreadable: 0,
                likes: 14,
                palettes: vec![
                    PaletteFile { volume: 0, name: "moss.palette".into(), bytes: 612 },
                    PaletteFile { volume: 0, name: "paper.palette".into(), bytes: 804 },
                    PaletteFile { volume: 0, name: "slate.palette".into(), bytes: 598 },
                ],
                rated: 37,
                counted: 212,
                views: vec![(
                    "Late favourites".into(),
                    "4 stars and up · played in the last 30 days · most played first".into(),
                )],
                playlists: vec![
                    PlaylistRow { name: "Late Night On The Bus".into(), tracks: 14, edited: true },
                    PlaylistRow { name: "Walk".into(), tracks: 9, edited: false },
                ],
            };
            m.status = "8 albums on the player, 7 plays in the log, 14 songs liked".into();
            m.tab = match state {
                "player" => Tab::Player,
                "likes" => Tab::Likes,
                _ => Tab::Palettes,
            };
            if state == "likes" {
                m.lastfm = flint_gui::Lastfm { has_key: true, user: Some("you".into()), editing: false };
                m.likes_plan = Some(flint_gui::LikesPlan {
                    liked: 16,
                    device_add: 2,
                    device_remove: 0,
                    lastfm_love: 3,
                    lastfm_unlove: 1,
                });
                m.lastfm_log = [
                    "E:\\: 14 liked on the player",
                    "last.fm: 13 loved tracks for you",
                    "merged: 16 liked; the player gains 2, loses 0; last.fm loves 3, unloves 1",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect();
            }
            if state == "palettes" || state == "palette-new" || state == "palette-shop" {
                let (pc, player) = preview_palettes();
                m.palette_dir = Some(PathBuf::from("D:\\Walkman\\Palettes"));
                m.palette_rows = flint_core::palette::compare(&pc, &player);
                m.status = "palettes: 1 new, 1 changed, 1 refused".into();
            }
            // The shop, as the repository's list reads: Slate and Moss already in the folder (Slate
            // on the player too), a light one and Cinder's Sony to install, and Dusk.
            if state == "palette-shop" {
                use flint_core::palette::{Have, SharedPalette, EXAMPLES};
                let (pc, player) = preview_palettes();
                let mut shared: Vec<(String, String)> =
                    pc.iter().filter(|(f, _)| f != "fog.palette").cloned().collect();
                shared.extend(player.iter().filter(|(f, _)| f == "dusk.palette").cloned());
                shared.extend(
                    EXAMPLES
                        .iter()
                        .filter(|(id, _)| *id == "sony")
                        .map(|(id, b)| (format!("{id}.palette"), b.to_string())),
                );
                shared.sort();
                let have = |list: &PaletteFiles, f: &str, body: &str| {
                    Have::of(list.iter().find(|(n, _)| n == f).map(|(_, b)| b.as_str()), body)
                };
                m.shop.items = shared
                    .iter()
                    .map(|(f, b)| SharedPalette {
                        folder: have(&pc, f, b),
                        player: have(&player, f, b),
                        ..SharedPalette::new(f, b)
                    })
                    .collect();
                m.shop.open = true;
                m.shop.read = true;
                m.status = format!("{} shared palettes", m.shop.items.len());
            }
            if state == "palette-new" {
                m.draft = flint_gui::Draft::from_start(2);
                m.draft.name = "Late Night".into();
                m.draft.hex[4] = "#b0a89c".into();
                m.focus = Some(flint_gui::Field::Colour(4));
            }
        }
        "check" | "filtered" => {
            use flint_gui::{CheckRow, Tab};
            ready(&mut m);
            let row = |v: &str, f: &str, why: &str| CheckRow { verdict: v.into(), file: f.into(), why: why.into() };
            m.findings = vec![
                row("LOSSY", "Burial/Untrue/02 Archangel.flac", "content stops dead at 16.0 kHz, like a lossy encoder"),
                row(
                    "UPSAMPLED",
                    "Nick Drake/Pink Moon/01 Pink Moon.flac",
                    "nothing real above 22.1 kHz: resampled from a lower rate",
                ),
                row(
                    "SUSPECT",
                    "Radiohead/Kid A/04 How to Disappear Completely.flac",
                    "content stops at 19.6 kHz: a high-bitrate lossy encoder, or the master",
                ),
                row(
                    "PADDED",
                    "Benjamin Francis Leftwich/After the Rain/07 Shine.flac",
                    "24-bit file holding 16-bit samples",
                ),
                row("DAMAGED", "Four Tet/Three/06 Skater.flac", "FFmpeg reported 3 error line(s) decoding it"),
            ];
            m.checked = Some(3184);
            m.status = "Checked 3,184 FLACs; 5 are worth a closer look.".into();
            m.tab = Tab::Check;
            if state == "filtered" {
                m.check_verdict = Some(0);
                m.check_filter = "burial".into();
                m.focus = Some(flint_gui::Field::CheckFilter);
            }
        }
        // A scan in the background, from the page it was started on, and from Sync: the answer to
        // "why can't I press anything while it scans" is that almost everything still presses.
        "sensme" | "scanning" => {
            ready(&mut m);
            m.sensme_log = [
                "found Music Center's engine",
                "3,184 tracks: 2,701 already analysed, 27 taken from the files, 456 to do",
                "[1/456] 01 Says.flac  104 BPM",
                "[2/456] 02 Hammers.flac  121 BPM",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            m.running = vec![Running {
                job: Job::Scan,
                progress: Some(2.0 / 456.0),
                status: "[2/456] 02 Hammers.flac  121 BPM".into(),
            }];
            m.tab = if state == "sensme" { flint_gui::Tab::SensMe } else { flint_gui::Tab::Sync };
        }
        "settings" | "signed-in" => {
            ready(&mut m);
            m.cache_dir = "C:\\Users\\you\\AppData\\Local\\flint".into();
            if state == "signed-in" {
                m.lastfm = flint_gui::Lastfm { has_key: true, user: Some("you".into()), editing: false };
            }
            m.tab = flint_gui::Tab::Settings;
        }
        other => {
            return Err(format!(
                "unknown state {other}; one of fresh, ready, planned, working, done, scanning, \
                 player, check, filtered, sensme, likes, palettes, palette-new, palette-shop, settings, \
                 signed-in"
            ))
        }
    }
    Ok(m)
}

type PaletteFiles = Vec<(String, String)>;

/// The Palettes preview's two folders, `(file, contents)`: one palette of every kind the page shows
/// — the same on both, new, changed, on the player only, and refused. Slate and Paper are Cinder's
/// own shipped files; the rest are made up, and checked by the same rules as a real one.
fn preview_palettes() -> (PaletteFiles, PaletteFiles) {
    const SLATE: &str = "name = Slate\nday.bg = #0e1116\nday.panel = #141820\nday.line = #232a35\n\
        day.ink = #e6ebf2\nday.dim = #8d97a5\nday.faint = #58616e\nnight.bg = #000000\n\
        night.panel = #0a0c10\nnight.line = #151a21\nnight.ink = #8a93a0\nnight.dim = #57606c\n\
        night.faint = #373d46\n";
    const PAPER: &str = "name = Paper\nday.bg = #f4f1ea\nday.panel = #ebe6dc\nday.line = #d6d0c4\n\
        day.ink = #1c1a17\nday.dim = #5f5a52\nday.faint = #8f887d\nnight.bg = #000000\n\
        night.panel = #0a0908\nnight.line = #161310\nnight.ink = #8d8170\nnight.dim = #5b5347\n\
        night.faint = #3b362d\nday.accent = #c4471a\nday.accent_ink = #ffffff\n\
        day.row_select = #e9e2d4\nnight.accent = #863810\nnight.accent_ink = #000000\n\
        night.row_select = #0f0c0a\n";
    const MOSS: &str = "name = Moss\nday.bg = #0f130f\nday.panel = #151a15\nday.line = #243024\n\
        day.ink = #e4efe4\nday.dim = #8ea08e\nday.faint = #5a685a\nnight.bg = #000000\n\
        night.panel = #0a0d0a\nnight.line = #151c15\nnight.ink = #8a9a8a\nnight.dim = #576557\n\
        night.faint = #374237\n";
    const DUSK: &str = "name = Dusk\nday.bg = #13101a\nday.panel = #1a1622\nday.line = #2a2436\n\
        day.ink = #ece6f4\nday.dim = #9a90aa\nday.faint = #635a72\nnight.bg = #000000\n\
        night.panel = #0c0a10\nnight.line = #1a1621\nnight.ink = #948aa3\nnight.dim = #5f576c\n\
        night.faint = #3d3746\n";
    // Grey on grey: readable on its author's bright monitor, not on the player in daylight.
    const FOG: &str = "name = Fog\nday.bg = #9a9a9a\nday.panel = #a4a4a4\nday.line = #8c8c8c\n\
        day.ink = #d8d8d8\nday.dim = #c0c0c0\nday.faint = #b0b0b0\nnight.bg = #000000\n\
        night.panel = #0a0a0a\nnight.line = #151515\nnight.ink = #8a8a8a\nnight.dim = #575757\n\
        night.faint = #373737\n";
    let files = |list: &[(&str, &str)]| list.iter().map(|(f, b)| (f.to_string(), b.to_string())).collect();
    let old_moss = MOSS.replace("#8ea08e", "#8a9a8a");
    (
        files(&[("slate.palette", SLATE), ("paper.palette", PAPER), ("moss.palette", MOSS), ("fog.palette", FOG)]),
        files(&[("slate.palette", SLATE), ("moss.palette", &old_moss), ("dusk.palette", DUSK)]),
    )
}

/// Draw the window to an SVG. This is how the window is designed and reviewed on a machine with no
/// Windows, and it is the same command list the window paints — see `flint-gui/src/svg.rs`.
fn gui_preview(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [out] = o.pos.as_slice() else { return Err(USAGE.into()) };
    let state = o.state.as_deref().unwrap_or("planned");
    let m = preview_model(state)?;
    let theme =
        if o.dark.unwrap_or(false) { flint_gui::paint::Theme::dark() } else { flint_gui::paint::Theme::light() };
    let svg = flint_gui::svg::render(&m, flint_gui::W, flint_gui::H, &theme);
    fs::write(out, svg).map_err(|e| format!("{out}: {e}"))?;
    println!("{out}  ({state}, {}x{})", flint_gui::W, flint_gui::H);
    Ok(())
}

// ── Last.fm: credentials, plays and likes ──────────────────────────────────────────────────────

/// `flint lastfm key|login|status …` — everything about the account, and nothing that syncs.
fn lastfm_cmd(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("key") => {
            let [api_key, api_secret] = &args[1..] else {
                return Err("usage: flint lastfm key <api-key> <api-secret>\n\
                            create a key at https://www.last.fm/api/account/create"
                    .into());
            };
            let mut creds = lastfm::Credentials::load();
            creds.api_key = api_key.trim().to_string();
            creds.api_secret = api_secret.trim().to_string();
            let path = creds.save().map_err(|e| format!("could not save the credentials: {e}"))?;
            println!("saved to {}", path.display());
            println!("next: flint lastfm login <your last.fm username>");
            Ok(())
        }
        Some("login") => {
            let [username] = &args[1..] else { return Err("usage: flint lastfm login <username>".into()) };
            let mut client = lastfm::Client::new(lastfm::Credentials::load()).map_err(|e| e.to_string())?;
            let password = read_password(&format!("Last.fm password for {username}: "))?;
            if password.is_empty() {
                return Err("no password given — nothing sent".into());
            }
            let (name, _) = client.authenticate(username.trim(), &password).map_err(|e| e.to_string())?;
            let path = client.creds.save().map_err(|e| format!("could not save the session: {e}"))?;
            println!("signed in as {name}; session key saved to {}", path.display());
            println!("the password was not stored — revoke the session at https://www.last.fm/settings/applications");
            Ok(())
        }
        Some("status") => {
            let creds = lastfm::Credentials::load();
            println!("credentials: {}", lastfm::Credentials::path().display());
            println!("  api key    {}", if creds.api_key.is_empty() { "—".into() } else { masked(&creds.api_key) });
            println!(
                "  api secret {}",
                if creds.api_secret.is_empty() { "—".into() } else { masked(&creds.api_secret) }
            );
            println!(
                "  session    {}",
                if creds.session_key.is_empty() { "—".into() } else { masked(&creds.session_key) }
            );
            println!("  username   {}", if creds.username.is_empty() { "—" } else { &creds.username });
            if !creds.is_ready() {
                println!("\nnot ready yet:");
                if creds.api_key.is_empty() || creds.api_secret.is_empty() {
                    println!("  flint lastfm key <api-key> <api-secret>     (https://www.last.fm/api/account/create)");
                }
                if creds.session_key.is_empty() {
                    println!("  flint lastfm login <username>");
                }
                return Ok(());
            }
            // Ready means it should answer. Asking is the only way to know the key still works.
            let mut client = lastfm::Client::new(creds).map_err(|e| e.to_string())?;
            match client.loved_tracks(|_, _, _| {}) {
                Ok(loved) => println!("\nLast.fm answers: {} loved track(s)", thousands(loved.len() as u64)),
                Err(e) => println!("\nLast.fm did not answer: {e}"),
            }
            Ok(())
        }
        _ => Err("usage: flint lastfm key <api-key> <api-secret> | login <username> | status".into()),
    }
}

/// `flint scrobble <volume>… [--apply]` — the plays the player recorded, sent to Last.fm.
fn scrobble_cmd(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let roots: Vec<PathBuf> = if o.pos.is_empty() { o.to.clone() } else { o.pos.iter().map(PathBuf::from).collect() };
    if roots.is_empty() {
        return Err("give the player's drive: flint scrobble E:\\ [--apply]".into());
    }
    let report =
        lastfm_sync::scrobble(&roots, lastfm::Credentials::load(), o.apply, &|| false, &mut |l| println!("{l}"))?;
    if !o.apply && report.found > 0 {
        println!("\nnothing was sent. Add --apply to scrobble {} play(s).", thousands(report.found as u64));
    }
    Ok(())
}

/// `flint likes <volume>… [--apply]` — the player's liked songs and Last.fm's loved tracks, kept
/// in step in both directions.
fn likes_cmd(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let roots: Vec<PathBuf> = if o.pos.is_empty() { o.to.clone() } else { o.pos.iter().map(PathBuf::from).collect() };
    if roots.is_empty() {
        return Err("give the player's drive: flint likes E:\\ [--to F:\\] [--apply]".into());
    }
    lastfm_sync::likes(
        &roots,
        lastfm::Credentials::load(),
        &likes::State::path(),
        o.apply,
        o.playlist,
        &|| false,
        &mut |l| println!("{l}"),
    )?;
    if !o.apply {
        println!("\nnothing was written. Add --apply to make these changes.");
    }
    Ok(())
}

/// `flint stats <volume> [--to <sd card>] [--apply]` — what the player remembers about its tracks.
///
/// Ratings and play counts whose album has moved between internal memory and the card follow it;
/// tracks the player has never counted get their plays from the scrobble log (counts and ratings
/// the player already has are left alone); and the saved views are listed.
fn stats_cmd(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let mut roots: Vec<PathBuf> = o.pos.iter().map(PathBuf::from).collect();
    roots.extend(o.to.iter().cloned());
    if roots.is_empty() {
        return Err("give the player's drive, internal storage first: flint stats E:\\ [--to F:\\] [--apply]".into());
    }
    let followed = stats::follow_player(&roots, o.apply, &mut |l| println!("{l}"))?;
    let report = stats::seed_player(&roots, o.apply, &mut |l| println!("{l}"))?;
    let saved = views::load(&roots[0].join(views::FILE_NAME));
    if !saved.is_empty() {
        println!("saved views (smart playlists): {}", saved.len());
        for v in &saved {
            println!("  {} — {}", v.name, v.describe());
        }
    }
    if !o.apply {
        let mut todo = Vec::new();
        if report.seeded.tracks > 0 {
            todo.push(format!("seed {} track(s)", thousands(report.seeded.tracks as u64)));
        }
        if followed.moved > 0 {
            todo.push(format!(
                "move the history of {} track(s) to where the files are",
                thousands(followed.moved as u64)
            ));
        }
        if !todo.is_empty() {
            println!("\nnothing was written. Add --apply to {}.", todo.join(" and "));
        }
    }
    Ok(())
}

/// `flint playlists <volume> [--to <PC folder>] [--library <library folder>] [--apply]` — the
/// playlists made on the player. Alone it lists them. With `--to` it takes the ones the player
/// has changed back to the PC and takes their EDITED mark off.
fn playlists_cmd(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let Some(drive) = o.pos.first().map(PathBuf::from) else {
        return Err(
            "give the player's drive: flint playlists E:\\ [--to <PC folder>] [--library <library folder>] [--apply]"
                .into(),
        );
    };
    let Some(to) = o.to.first() else {
        let lists = playlists::read_player(&drive).map_err(|e| format!("{}: {e}", drive.display()))?;
        if lists.is_empty() {
            println!("no playlists have been made on this player.");
            return Ok(());
        }
        for l in &lists {
            println!(
                "  {}{} — {} track(s)",
                if l.edited.is_some() { "EDITED  " } else { "        " },
                l.name,
                l.tracks
            );
        }
        let edited = lists.iter().filter(|l| l.edited.is_some()).count();
        if edited > 0 {
            println!("\n{edited} changed on the player. Add --to <PC folder> to take them back.");
        }
        return Ok(());
    };
    let report = playlists::pull(&drive, to, o.library.as_deref(), o.apply, &mut |l| println!("{l}"))?;
    if report.edited == 0 {
        println!("{} playlist(s) on the player, none changed since a PC last took them.", report.on_player);
    } else if !o.apply {
        println!("\nnothing was written. Add --apply to take {} playlist(s) back.", report.edited);
    } else {
        println!(
            "\ndone: {} pulled to {}, {} EDITED mark(s) taken off{}",
            report.pulled,
            to.display(),
            report.cleared,
            if report.failed > 0 { format!(", {} failed", report.failed) } else { String::new() }
        );
    }
    Ok(())
}

/// Enough of a secret to recognise, not enough to use.
fn masked(value: &str) -> String {
    if value.len() <= 8 {
        return "*".repeat(value.len());
    }
    format!("{}…{}", &value[..4], &value[value.len() - 4..])
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Read a password without echoing it. Windows turns the console's echo off; everywhere else
/// `stty` does. If neither works the prompt says so rather than quietly showing the password.
fn read_password(prompt: &str) -> Result<String, String> {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let quiet = echo_off();
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if quiet {
        echo_on();
        println!();
    }
    read.map_err(|e| format!("could not read the password: {e}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(windows)]
fn echo_off() -> bool {
    windows_console::set_echo(false)
}

#[cfg(windows)]
fn echo_on() {
    windows_console::set_echo(true);
}

#[cfg(windows)]
mod windows_console {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    }

    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;

    pub fn set_echo(on: bool) -> bool {
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) == 0 {
                return false; // not a console (piped input) — nothing to hide
            }
            let next = if on { mode | ENABLE_ECHO_INPUT } else { mode & !ENABLE_ECHO_INPUT };
            SetConsoleMode(handle, next) != 0
        }
    }
}

#[cfg(not(windows))]
fn echo_off() -> bool {
    std::process::Command::new("stty")
        .arg("-echo")
        .stdin(std::process::Stdio::inherit())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn echo_on() {
    let _ = std::process::Command::new("stty").arg("echo").stdin(std::process::Stdio::inherit()).status();
}
