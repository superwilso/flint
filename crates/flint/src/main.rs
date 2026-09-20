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
//! flint gui                                                       open the window (Windows)
//! flint gui-preview <out.svg> [--state name]                      draw the window to an SVG, anywhere
//! ```

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use flint_core::{apply, cache, engine, engine::Engine, flac, id3, library, lossless, musiccenter, smfmf, space, sync};

const USAGE: &str = "\
usage:
  flint scan <library folder> [--jobs N] [--cache dir]
  flint check <library folder | file.flac> [--jobs N] [--cache dir] [--all] [--verbose]
  flint analyse <track> [--out result.smfmf] [--param id=value]...
  flint tag-copy <src.flac|src.mp3> <dst> [--smfmf result.smfmf] [--param id=value]...
  flint sync <library folder> --to <volume> [--to <volume>] [--gb N]... [--playlists <folder>] [--apply] [--no-sensme]
  flint inspect <file.flac | file.mp3 | result.smfmf>
  flint import [--from <Music Center data folder>] [--cache dir]
  flint gui [--dark | --light]
  flint gui-preview <out.svg> [--state fresh|ready|planned|working|done] [--dark]";

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
    from: Option<PathBuf>,
    state: Option<String>,
    /// `--dark` / `--light`; `None` means follow Windows.
    dark: Option<bool>,
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
        from: None,
        state: None,
        dark: None,
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
            "--state" => o.state = Some(value("--state")?),
            "--dark" => o.dark = Some(true),
            "--light" => o.dark = Some(false),
            "--apply" => o.apply = true,
            "--no-sensme" => o.no_sensme = true,
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
    let source = sync::scan_library(library).map_err(|e| format!("{}: {e}", library.display()))?;
    let source_bytes: u64 = source.iter().map(|f| f.size).sum();
    println!("  {} tracks, {}", source.len(), space::human(source_bytes));

    // ANALYSIS THAT IS ALREADY IN THE FILES. Someone who runs Sony's Music Center has tags in their
    // library already, and may never run `flint scan` at all — so the tags are taken here, before
    // the plan decides which copies can carry one. Reading them costs a metadata read per file that
    // the cache does not already know, and no decode.
    if !o.no_sensme {
        let paths: Vec<PathBuf> =
            source.iter().map(|f| library.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR))).collect();
        match musiccenter::adopt_all(&paths, &mut analysis, |_| {}) {
            Ok((0, _)) => {}
            Ok((n, saved)) => {
                analysis.save().map_err(|e| e.to_string())?;
                println!(
                    "  {n} already carried Sony's analysis — taken from the files{}",
                    if saved > 0 {
                        format!(", {} of chunks the player never reads left behind", space::human(saved))
                    } else {
                        String::new()
                    }
                );
            }
            Err(e) => eprintln!("flint: reading existing SensMe tags: {e}"),
        }
    }

    let mut volumes = Vec::new();
    let mut scans = Vec::new();
    let mut manifests = Vec::new();
    for (i, root) in o.to.iter().enumerate() {
        let scan = sync::scan_volume(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let on_device: u64 = scan.files.values().map(|(size, _)| size).sum();
        // What this volume may hold: whatever is free now, plus what its music already occupies,
        // less the headroom. A --gb says so outright instead.
        let budget_bytes = match o.gb.get(i) {
            Some(gb) => (gb * 1024.0 * 1024.0 * 1024.0) as u64,
            None => match space::free_bytes(root) {
                Some(free) => (free + on_device).saturating_sub(sync::HEADROOM_BYTES),
                None => return Err(format!("could not read the free space on {}; give it as --gb N", root.display())),
            },
        };
        println!(
            "  {} holds {} in {} files, budget {}",
            root.display(),
            space::human(on_device),
            scan.files.len(),
            space::human(budget_bytes)
        );
        volumes.push(sync::Volume { name: root.display().to_string(), root: root.clone(), budget_bytes });
        manifests.push(sync::Manifest::load(root));
        scans.push(scan);
    }

    let playlists = match &o.playlists {
        Some(dir) => sync::read_playlists(dir, library).map_err(|e| format!("{}: {e}", dir.display()))?,
        None => std::collections::BTreeMap::new(),
    };
    if !playlists.is_empty() {
        println!("  {} playlists", playlists.len());
    }

    // A track is tagged when the analysis cache already holds its result; `flint scan` fills it.
    let tag_for = |f: &sync::SourceFile| {
        if o.no_sensme {
            return String::new();
        }
        let path = library.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        match cache::content_key(&path) {
            Ok(Some(key)) if analysis.get(&key).is_some() => key,
            _ => String::new(),
        }
    };
    let plan = sync::plan(&source, &volumes, &scans, &manifests, &playlists, tag_for);
    let tagged = plan.copies.iter().filter(|c| !c.tag.is_empty()).count();
    let pending_playlists = apply::playlists_to_write(&plan, &volumes);
    println!(
        "\nplan: {} to copy ({} tagged with SensMe data), {} to remove, {} playlists to write",
        plan.copies.len(),
        tagged,
        plan.stale_files.len() + plan.stale_playlists.len(),
        pending_playlists.len()
    );
    for (i, v) in volumes.iter().enumerate() {
        let albums = plan.assignments.values().filter(|&&a| a == i).count();
        println!("  {}: {albums} albums, {} to copy", v.root.display(), space::human(plan.bytes_to_copy(i)));
    }
    if !plan.skipped.is_empty() {
        println!("  no room for {} albums: {}", plan.skipped.len(), plan.skipped.join(", "));
    }
    if plan.copies.is_empty()
        && plan.stale_files.is_empty()
        && plan.stale_playlists.is_empty()
        && pending_playlists.is_empty()
    {
        println!("nothing to do.");
        return Ok(());
    }
    if !o.apply {
        println!("\nthis was a dry run; add --apply to carry it out.");
        for (v, rel) in plan.stale_files.iter().take(10) {
            println!("  would remove  {}/{rel}", volumes[*v].root.display());
        }
        for c in plan.copies.iter().take(10) {
            println!("  would copy    {}{}", c.rel, if c.tag.is_empty() { "" } else { "  +SensMe" });
        }
        return Ok(());
    }

    let out = apply::apply(
        &plan,
        library,
        &volumes,
        &mut manifests,
        |c| analysis.blob(&c.tag).ok(),
        false,
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
    use flint_gui::{LibraryFacts, Model, Phase, VolumeFacts};
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
                "would remove  E:\\Bonobo - Migration/03 Break Apart.flac",
                "would copy    Aphex Twin - Selected Ambient Works 85-92/01 Xtal.flac  +SensMe",
                "would copy    Aphex Twin - Selected Ambient Works 85-92/02 Tha.flac  +SensMe",
                "would copy    Bicep - Isles/01 Atlas.flac  +SensMe",
                "…and 2,187 more",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            m.status = "Nothing has been written. 2,190 files would be copied — press Copy to the player.".into();
        }
        "working" => {
            ready(&mut m);
            measured(&mut m);
            m.phase = Phase::Working;
            m.progress = Some(0.41);
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
            m.status = "[900/2190] Bonobo - Fragments/01 Polyghost.flac  +SensMe".into();
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
            m.progress = Some(1.0);
            m.status = "Copied 2,190 files (114.1 GB), 2,043 tagged, 1 removed, 4 playlists written.".into();
        }
        other => return Err(format!("unknown state {other}; one of fresh, ready, planned, working, done")),
    }
    Ok(m)
}

/// Draw the window to an SVG. This is how the window is designed and reviewed on a machine with no
/// Windows, and it is the same command list the window paints — see `flint-gui/src/svg.rs`.
fn gui_preview(args: &[String]) -> Result<(), String> {
    let o = opts(args)?;
    let [out] = o.pos.as_slice() else { return Err(USAGE.into()) };
    let state = o.state.as_deref().unwrap_or("planned");
    let m = preview_model(state)?;
    let theme = if o.dark.unwrap_or(false) {
        flint_gui::paint::Theme::dark()
    } else {
        flint_gui::paint::Theme::light()
    };
    let svg = flint_gui::svg::render(&m, flint_gui::W, flint_gui::H, &theme);
    fs::write(out, svg).map_err(|e| format!("{out}: {e}"))?;
    println!("{out}  ({state}, {}x{})", flint_gui::W, flint_gui::H);
    Ok(())
}
