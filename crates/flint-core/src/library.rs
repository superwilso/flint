//! Scanning the PC library: find every track, and analyse the ones the cache has not seen.
//!
//! This is the step Sony's Music Center does before a transfer, done once and kept: after the first
//! scan, only new or changed audio costs anything. Work is spread over `jobs` threads; each runs one
//! FFmpeg + `sensme-helper` pair at a time. The index is saved every `SAVE_EVERY` results, so an
//! interrupted scan keeps what it finished.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::cache::{self, Cache, Entry};
use crate::engine::Engine;
use crate::lossless::{self, Measurement};
use crate::smfmf;

pub const EXTENSIONS: [&str; 2] = ["flac", "mp3"];
const SAVE_EVERY: usize = 25;

/// Every FLAC and MP3 under `root`, sorted. Hidden entries (a leading dot) are skipped, which also
/// skips Flint's own `.name.flint-partial` files.
pub fn walk(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Run `work` over `items` on up to `jobs` threads. Results arrive in completion order; join the
/// handles after draining the receiver.
fn pool<T, R>(
    items: Vec<T>,
    jobs: usize,
    work: impl Fn(T) -> R + Send + Sync + 'static,
) -> (mpsc::Receiver<R>, Vec<thread::JoinHandle<()>>)
where
    T: Send + 'static,
    R: Send + 'static,
{
    let n = jobs.max(1).min(items.len().max(1));
    let queue = Arc::new(Mutex::new(items));
    let work = Arc::new(work);
    let (tx, rx) = mpsc::channel();
    let workers = (0..n)
        .map(|_| {
            let (queue, work, tx) = (Arc::clone(&queue), Arc::clone(&work), tx.clone());
            thread::spawn(move || loop {
                let Some(item) = queue.lock().expect("queue lock").pop() else { break };
                if tx.send(work(item)).is_err() {
                    break;
                }
            })
        })
        .collect();
    (rx, workers)
}

#[derive(Debug)]
pub enum Event {
    /// Found `total` tracks; `cached` need nothing; `queued` will be analysed.
    Planned {
        total: usize,
        cached: usize,
        queued: usize,
        skipped: usize,
    },
    Analysed {
        done: usize,
        queued: usize,
        path: PathBuf,
        ms: u128,
        bpm: Option<f32>,
    },
    Failed {
        done: usize,
        queued: usize,
        path: PathBuf,
        error: String,
    },
}

#[derive(Debug, Default)]
pub struct Report {
    pub total: usize,
    pub cached: usize,
    pub analysed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub seconds: f64,
}

/// Analyse everything under `root` that the cache lacks. `engine` is only needed when something is
/// queued; pass `None` to plan without analysing (the report then counts the queue as failed).
pub fn scan(
    root: &Path,
    cache: &mut Cache,
    engine: Option<&Engine>,
    jobs: usize,
    mut on_event: impl FnMut(&Event),
) -> io::Result<Report> {
    let started = Instant::now();
    let files = walk(root)?;
    let mut report = Report { total: files.len(), ..Report::default() };
    let mut queue = Vec::new();
    for path in files {
        let label = path.to_string_lossy().into_owned();
        let Ok((size, mtime)) = cache::stat(&path) else {
            report.skipped += 1;
            continue;
        };
        if cache.fast_key(&label, size, mtime).is_some() {
            report.cached += 1;
            continue;
        }
        match cache::content_key(&path) {
            Ok(Some(key)) if cache.get(&key).is_some() => {
                cache.alias(&key, &label, size, mtime);
                report.cached += 1;
            }
            Ok(Some(key)) => queue.push((path, key, size, mtime)),
            _ => report.skipped += 1,
        }
    }
    on_event(&Event::Planned {
        total: report.total,
        cached: report.cached,
        queued: queue.len(),
        skipped: report.skipped,
    });
    let queued = queue.len();
    if queued == 0 {
        cache.save()?;
        report.seconds = started.elapsed().as_secs_f64();
        return Ok(report);
    }
    let Some(engine) = engine else {
        report.failed = queued;
        cache.save()?;
        report.seconds = started.elapsed().as_secs_f64();
        return Ok(report);
    };

    let engine = engine.clone();
    let (rx, workers) = pool(queue, jobs, move |(path, key, size, mtime)| {
        let t = Instant::now();
        let result = engine.analyse(&path);
        (path, key, size, mtime, result, t.elapsed().as_millis())
    });

    for (done, (path, key, size, mtime, result, ms)) in rx.into_iter().enumerate().map(|(i, v)| (i + 1, v)) {
        match result {
            Ok(a) => {
                let bpm = smfmf::summarise(&a.smfmf).ok().and_then(|s| s.bpm);
                let engine_version =
                    a.info.iter().find(|(k, _)| k == "engine").map(|(_, v)| v.clone()).unwrap_or_default();
                let entry = Entry {
                    key,
                    size,
                    mtime,
                    engine: engine_version,
                    bpm,
                    bytes: a.smfmf.len(),
                    path: path.to_string_lossy().into_owned(),
                };
                match cache.put(entry, &a.smfmf) {
                    Ok(()) => {
                        report.analysed += 1;
                        on_event(&Event::Analysed { done, queued, path, ms, bpm });
                    }
                    Err(e) => {
                        report.failed += 1;
                        on_event(&Event::Failed { done, queued, path, error: format!("saving the result: {e}") });
                    }
                }
                if report.analysed.is_multiple_of(SAVE_EVERY) {
                    cache.save()?;
                }
            }
            Err(error) => {
                report.failed += 1;
                on_event(&Event::Failed { done, queued, path, error });
            }
        }
    }
    for w in workers {
        let _ = w.join();
    }
    cache.save()?;
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

pub enum CheckEvent {
    Planned { total: usize, stored: usize, queued: usize, skipped: usize },
    Measured { done: usize, queued: usize, path: PathBuf, measurement: Measurement },
    Failed { done: usize, queued: usize, path: PathBuf, error: String },
}

/// One FLAC's result, fresh or stored.
pub struct Checked {
    pub path: PathBuf,
    pub measurement: Measurement,
}

/// The lossless check over every FLAC under `root` (or `root` itself, if it is a file). Stored
/// measurements are reused by content key, so a retagged or moved file is not decoded again.
pub fn check(
    root: &Path,
    store: &mut lossless::Store,
    ffmpeg: &Path,
    jobs: usize,
    mut on_event: impl FnMut(&CheckEvent),
) -> io::Result<Vec<Checked>> {
    let files: Vec<PathBuf> = if root.is_file() {
        vec![root.to_path_buf()]
    } else {
        walk(root)?
            .into_iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("flac")))
            .collect()
    };
    let total = files.len();
    let mut out = Vec::new();
    let (mut queue, mut skipped) = (Vec::new(), 0);
    for path in files {
        match cache::content_key(&path) {
            Ok(Some(key)) => match store.get(&key) {
                Some(m) => out.push(Checked { path, measurement: m.clone() }),
                None => queue.push((path, key)),
            },
            _ => skipped += 1,
        }
    }
    let queued = queue.len();
    on_event(&CheckEvent::Planned { total, stored: out.len(), queued, skipped });
    let ffmpeg = ffmpeg.to_path_buf();
    let (rx, workers) = pool(queue, jobs, move |(path, key)| {
        let result = lossless::measure(&ffmpeg, &path);
        (path, key, result)
    });
    let mut done = 0;
    for (path, key, result) in rx {
        done += 1;
        match result {
            Ok(measurement) => {
                store.put(&key, measurement.clone(), &path.to_string_lossy());
                if done % SAVE_EVERY == 0 {
                    store.save()?;
                }
                on_event(&CheckEvent::Measured { done, queued, path: path.clone(), measurement: measurement.clone() });
                out.push(Checked { path, measurement });
            }
            Err(error) => on_event(&CheckEvent::Failed { done, queued, path, error }),
        }
    }
    for w in workers {
        let _ = w.join();
    }
    store.save()?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_finds_music_case_insensitively_and_skips_hidden() {
        let d = std::env::temp_dir().join(format!("flint-walk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("Artist/Album")).unwrap();
        fs::create_dir_all(d.join(".hidden")).unwrap();
        for f in [
            "Artist/Album/01.FLAC",
            "Artist/Album/02.mp3",
            "Artist/Album/cover.jpg",
            ".hidden/x.flac",
            "Artist/Album/.03.flac.flint-partial",
            "loose.Mp3",
        ] {
            fs::write(d.join(f), b"").unwrap();
        }
        let found: Vec<String> = walk(&d)
            .unwrap()
            .iter()
            .map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(found, vec!["Artist/Album/01.FLAC", "Artist/Album/02.mp3", "loose.Mp3"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_second_scan_of_unchanged_files_analyses_nothing() {
        let d = std::env::temp_dir().join(format!("flint-rescan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("lib")).unwrap();
        let track = d.join("lib/a.mp3");
        fs::write(&track, b"\xff\xfb\x90\x64frames").unwrap();
        let mut cache = Cache::open(&d.join("cache")).unwrap();
        // First pass without an engine: the track is queued and cannot be analysed.
        let r = scan(&d.join("lib"), &mut cache, None, 2, |_| {}).unwrap();
        assert_eq!((r.total, r.cached, r.failed), (1, 0, 1));
        // Pretend it was analysed, then rescan: nothing queued, and a renamed copy is also a hit.
        let key = cache::content_key(&track).unwrap().unwrap();
        let (size, mtime) = cache::stat(&track).unwrap();
        cache
            .put(
                Entry {
                    key,
                    size,
                    mtime,
                    engine: "t".into(),
                    bpm: None,
                    bytes: 1,
                    path: track.to_string_lossy().into(),
                },
                b"x",
            )
            .unwrap();
        fs::copy(&track, d.join("lib/renamed.mp3")).unwrap();
        let r = scan(&d.join("lib"), &mut cache, None, 2, |_| {}).unwrap();
        assert_eq!((r.total, r.cached, r.failed), (2, 2, 0));
        fs::remove_dir_all(&d).unwrap();
    }
}
