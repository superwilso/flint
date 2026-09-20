//! What the buttons actually do.
//!
//! **Why this is not in `win32.rs`.** A GUI that reimplements its own tool's logic drifts from it,
//! and the drift is invisible until someone's library is on the line. So every job here is the same
//! sequence of `flint-core` calls the command line makes — the scan is `library::scan`, the plan is
//! `sync::plan`, the copy is `apply::apply` — and the only difference is that progress arrives as
//! [`Update`]s instead of `println!`. Keeping it out of the Windows file also means it is testable
//! on a machine with no Windows, and the tests below run a real sync against real folders.
//!
//! **Cancellation** is cooperative and lands between files, never inside one. `apply` writes every
//! copy to a temporary name and renames it, so a job stopped here leaves a volume with whole files
//! on it and nothing half-written.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use flint_core::{apply, cache, engine, library, lossless, musiccenter, space, sync};

use crate::Job;

/// What a job needs to know. Built from the [`crate::Model`] by the platform layer, so nothing here
/// touches the UI.
#[derive(Clone, Debug)]
pub struct Settings {
    pub library: PathBuf,
    /// In the order they were offered: internal memory, then the card.
    pub volumes: Vec<PathBuf>,
    /// A budget in GB for the volume at the same index, instead of reading the free space. The
    /// window never sets one — it is here because `space::free_bytes` only answers on Windows, so
    /// this is the only way the job below is exercised by a test on anything else, and because a
    /// volume whose free space cannot be read needs *something* to fall back on rather than an
    /// error the user cannot act on.
    pub budget_gb: Vec<Option<f64>>,
    pub playlists: Option<PathBuf>,
    /// Write SensMe tags into the copies.
    pub sensme: bool,
    /// Copy cover art and lyrics that sit beside the music.
    pub extras: bool,
    pub cache: PathBuf,
    pub jobs: usize,
    /// Where Music Center keeps its data, when it is not where it usually is.
    pub music_center: Option<PathBuf>,
}

impl Settings {
    pub fn new(library: PathBuf) -> Settings {
        Settings {
            library,
            volumes: Vec::new(),
            budget_gb: Vec::new(),
            playlists: None,
            sensme: true,
            extras: true,
            cache: cache::default_dir(),
            jobs: std::thread::available_parallelism().map_or(4, |n| n.get()),
            music_center: None,
        }
    }
}

/// Something the window should show. The platform layer turns these into a repaint.
#[derive(Clone, PartialEq, Debug)]
pub enum Update {
    /// A line for the log, and for the status line under the bar.
    Say(String),
    /// A line for the log only — the per-file chatter, which would make the status line flicker.
    Log(String),
    /// 0.0..=1.0, or `None` for a job with no measurable length.
    Progress(Option<f32>),
    /// A plan was made and shown, so COPY may be offered.
    Planned,
    /// What the library turned out to hold. The window draws it under the folder's name.
    Library(crate::LibraryFacts),
    /// What a destination holds, and what this plan would add to it. Sent twice per volume: once
    /// when it has been read, and again once the plan has decided what goes where. These numbers
    /// were always worked out here — until the window had meters, they only ever reached the user
    /// as a sentence in the log.
    Volume(usize, crate::VolumeFacts),
}

/// Run `job`. Returns the last word for the status line, or the reason it stopped.
///
/// `cancel` is checked between files. `emit` is called from this thread — the platform layer is
/// responsible for getting it to the UI thread safely.
pub fn run(job: Job, s: &Settings, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    match job {
        Job::Plan => sync_job(s, false, cancel, emit),
        Job::Apply => sync_job(s, true, cancel, emit),
        Job::Scan => scan_job(s, cancel, emit),
        Job::Import => import_job(s, emit),
        Job::Check => check_job(s, cancel, emit),
    }
}

fn stopped(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

fn open_cache(s: &Settings) -> Result<cache::Cache, String> {
    cache::Cache::open(&s.cache).map_err(|e| format!("{}: {e}", s.cache.display()))
}

/// PLAN and COPY are one function on purpose: the plan a copy carries out must be the plan that was
/// shown, and the surest way to guarantee that is for there to be one piece of code that makes it.
fn sync_job(s: &Settings, write: bool, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    if s.volumes.is_empty() {
        return Err("choose a player volume first".into());
    }
    let mut analysis = open_cache(s)?;

    emit(Update::Say(format!("reading {}", s.library.display())));
    emit(Update::Progress(None));
    let mut source = sync::scan_library(&s.library).map_err(|e| format!("{}: {e}", s.library.display()))?;
    if !s.extras {
        source.retain(|f| !f.is_sidecar());
    }
    let bytes: u64 = source.iter().map(|f| f.size).sum();
    emit(Update::Library(crate::LibraryFacts { files: source.len(), bytes }));
    emit(Update::Say(format!("{} files, {}", source.len(), space::human(bytes))));

    // Analysis already inside the files — see `musiccenter`. Taken before the plan decides which
    // copies can carry a tag, exactly as the command line does it.
    if s.sensme {
        let paths: Vec<PathBuf> =
            source.iter().map(|f| s.library.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR))).collect();
        match musiccenter::adopt_all(&paths, &mut analysis, |_| {}) {
            Ok((0, _)) => {}
            Ok((n, saved)) => {
                analysis.save().map_err(|e| e.to_string())?;
                emit(Update::Say(format!(
                    "{n} already carried Sony's analysis — taken from the files{}",
                    if saved > 0 {
                        format!(", {} of unread chunks left behind", space::human(saved))
                    } else {
                        String::new()
                    }
                )));
            }
            Err(e) => emit(Update::Log(format!("reading existing SensMe tags: {e}"))),
        }
    }

    let mut volumes = Vec::new();
    let mut scans = Vec::new();
    let mut manifests = Vec::new();
    for (i, root) in s.volumes.iter().enumerate() {
        let scan = sync::scan_volume(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let on_device: u64 = scan.files.values().map(|(size, _)| size).sum();
        // What this volume may hold: whatever is free now, plus what its music already occupies,
        // less the headroom — the same sum the command line does.
        let budget = match s.budget_gb.get(i).copied().flatten() {
            Some(gb) => (gb * 1024.0 * 1024.0 * 1024.0) as u64,
            None => match space::free_bytes(root) {
                Some(free) => (free + on_device).saturating_sub(sync::HEADROOM_BYTES),
                None => {
                    return Err(format!(
                        "could not read the free space on {} — is the player still connected?",
                        root.display()
                    ))
                }
            },
        };
        emit(Update::Volume(i, crate::VolumeFacts { on_device, budget, to_copy: 0, albums: 0 }));
        emit(Update::Log(format!(
            "{} holds {} in {} files, budget {}",
            root.display(),
            space::human(on_device),
            scan.files.len(),
            space::human(budget)
        )));
        volumes.push(sync::Volume { name: root.display().to_string(), root: root.clone(), budget_bytes: budget });
        manifests.push(sync::Manifest::load(root));
        scans.push(scan);
    }

    let playlists = match &s.playlists {
        Some(dir) => sync::read_playlists(dir, &s.library).map_err(|e| format!("{}: {e}", dir.display()))?,
        None => Default::default(),
    };
    if !playlists.is_empty() {
        emit(Update::Log(format!("{} playlists", playlists.len())));
    }

    let tag_for = |f: &sync::SourceFile| {
        if !s.sensme {
            return String::new();
        }
        let path = s.library.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        match cache::content_key(&path) {
            Ok(Some(key)) if analysis.get(&key).is_some() => key,
            _ => String::new(),
        }
    };
    let plan = sync::plan(&source, &volumes, &scans, &manifests, &playlists, tag_for);
    let tagged = plan.copies.iter().filter(|c| !c.tag.is_empty()).count();
    let pending = apply::playlists_to_write(&plan, &volumes);

    emit(Update::Say(format!(
        "{} to copy ({tagged} with SensMe), {} to remove, {} playlists",
        plan.copies.len(),
        plan.stale_files.len() + plan.stale_playlists.len(),
        pending.len()
    )));
    for (i, v) in volumes.iter().enumerate() {
        let albums = plan.assignments.values().filter(|&&a| a == i).count();
        let to_copy = plan.bytes_to_copy(i);
        emit(Update::Volume(
            i,
            crate::VolumeFacts {
                on_device: scans[i].files.values().map(|(size, _)| size).sum(),
                budget: v.budget_bytes,
                to_copy,
                albums,
            },
        ));
        emit(Update::Log(format!("{}: {albums} albums, {} to copy", v.root.display(), space::human(to_copy))));
    }
    if !plan.skipped.is_empty() {
        emit(Update::Log(format!("no room for {} albums: {}", plan.skipped.len(), plan.skipped.join(", "))));
    }

    let nothing =
        plan.copies.is_empty() && plan.stale_files.is_empty() && plan.stale_playlists.is_empty() && pending.is_empty();
    if nothing {
        emit(Update::Progress(Some(1.0)));
        return Ok("The player already matches the library — nothing to do.".into());
    }

    if !write {
        // The preview: the first few of each, which is what the terminal prints too. The whole list
        // would be thousands of lines and the pane shows the tail, so it would show only the end.
        for (v, rel) in plan.stale_files.iter().take(12) {
            emit(Update::Log(format!("would remove  {}/{rel}", volumes[*v].root.display())));
        }
        for c in plan.copies.iter().take(12) {
            emit(Update::Log(format!("would copy    {}{}", c.rel, if c.tag.is_empty() { "" } else { "  +SensMe" })));
        }
        if plan.copies.len() > 12 {
            emit(Update::Log(format!("…and {} more", plan.copies.len() - 12)));
        }
        emit(Update::Planned);
        emit(Update::Progress(Some(1.0)));
        return Ok(format!(
            "Nothing has been written. {} files would be copied — press Copy to the player.",
            plan.copies.len()
        ));
    }

    let mut cancelled = false;
    let out = apply::apply(
        &plan,
        &s.library,
        &volumes,
        &mut manifests,
        |c| analysis.blob(&c.tag).ok(),
        false,
        |ev| match ev {
            apply::Event::Copied { done, total, rel, tagged, .. } => {
                if stopped(cancel) {
                    cancelled = true;
                }
                emit(Update::Progress(Some(*done as f32 / (*total).max(1) as f32)));
                emit(Update::Say(format!("[{done}/{total}] {rel}{}", if *tagged { "  +SensMe" } else { "" })));
            }
            apply::Event::Removed { rel, .. } => emit(Update::Log(format!("removed {rel}"))),
            apply::Event::Playlist { name, tracks, .. } => {
                emit(Update::Log(format!("playlist {name} ({tracks} tracks)")))
            }
            apply::Event::Failed { what, error } => emit(Update::Log(format!("FAILED {what}: {error}"))),
        },
    )
    .map_err(|e| e.to_string())?;

    let _ = cancelled;
    emit(Update::Progress(Some(1.0)));
    let mut done = format!(
        "Copied {} files ({}), {} tagged, {} removed, {} playlists written.",
        out.copied,
        space::human(out.bytes),
        out.tagged,
        out.removed,
        out.playlists
    );
    if out.failed > 0 {
        done.push_str(&format!(" {} failed — see the log.", out.failed));
    }
    if stopped(cancel) {
        done.push_str(" Stopped early.");
    }
    Ok(done)
}

fn scan_job(s: &Settings, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    let mut c = open_cache(s)?;
    emit(Update::Progress(None));
    emit(Update::Say("looking for Sony's analysis engine…".into()));
    // No engine is not a failure: everything already cached or already tagged still counts, and
    // saying so is more use than an error box. Music Center is a big install to demand of someone
    // who only wanted to know how many tracks they have.
    let engine = match engine::Engine::locate() {
        Ok(e) => {
            emit(Update::Log("found Music Center's engine".into()));
            Some(e)
        }
        Err(e) => {
            emit(Update::Log(format!("no engine: {e}")));
            emit(Update::Log(
                "tracks that already carry an analysis will still be taken; nothing new can be computed".into(),
            ));
            None
        }
    };
    let mut stop_note = false;
    let report = library::scan(&s.library, &mut c, engine.as_ref(), s.jobs, |ev| match ev {
        library::Event::Planned { total, cached, queued, adopted, .. } => {
            emit(Update::Say(format!(
                "{total} tracks: {cached} already analysed, {adopted} taken from the files, {queued} to do"
            )));
        }
        library::Event::Analysed { done, queued, path, bpm, .. } => {
            emit(Update::Progress(Some(*done as f32 / (*queued).max(1) as f32)));
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            emit(Update::Say(match bpm {
                Some(b) => format!("[{done}/{queued}] {name}  {b:.0} BPM"),
                None => format!("[{done}/{queued}] {name}"),
            }));
            if stopped(cancel) && !stop_note {
                stop_note = true;
                emit(Update::Log("stopping after the tracks already started…".into()));
            }
        }
        library::Event::Failed { path, error, .. } => {
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            emit(Update::Log(format!("FAILED {name}: {error}")));
        }
    })
    .map_err(|e| format!("{}: {e}", s.library.display()))?;
    c.save().map_err(|e| e.to_string())?;
    emit(Update::Progress(Some(1.0)));
    Ok(format!(
        "{} tracks: {} analysed now, {} taken from the files, {} already known, {} failed.",
        report.total, report.analysed, report.adopted, report.cached, report.failed
    ))
}

fn import_job(s: &Settings, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    let mut c = open_cache(s)?;
    emit(Update::Progress(None));
    let dir = match s.music_center.clone().or_else(musiccenter::data_dir) {
        Some(d) => d,
        None => {
            return Err("Music Center for PC was not found. Install it, or point Flint at its \
                        data folder with --from on the command line."
                .into())
        }
    };
    if !dir.is_dir() {
        return Err(format!("{}: not a folder", dir.display()));
    }
    emit(Update::Say(format!("reading {}", dir.display())));
    let mut saved: u64 = 0;
    let report = musiccenter::import(&dir, &mut c, |path, was, now| {
        saved += (was - now) as u64;
        emit(Update::Log(format!("{was:>8} -> {now:<6} bytes  {}", short(path))));
    })
    .map_err(|e| format!("{}: {e}", dir.display()))?;
    c.save().map_err(|e| e.to_string())?;
    emit(Update::Progress(Some(1.0)));
    let mut word = format!(
        "{} analyses in Music Center's cache: {} taken, {} already known, {} whose track could not be found.",
        report.cached, report.imported, report.already, report.missing
    );
    if saved > 0 {
        word.push_str(&format!(" {} of unread chunks left behind.", space::human(saved)));
    }
    Ok(word)
}

fn check_job(s: &Settings, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    let mut store = lossless::Store::open(&s.cache).map_err(|e| format!("{}: {e}", s.cache.display()))?;
    emit(Update::Progress(None));
    emit(Update::Say("looking for FFmpeg…".into()));
    let ffmpeg = engine::locate_ffmpeg()?;
    let mut stop_note = false;
    let checked = library::check(&s.library, &mut store, &ffmpeg, s.jobs, |ev| match ev {
        library::CheckEvent::Planned { total, stored, queued, skipped } => {
            emit(Update::Say(format!("{total} FLACs: {stored} already checked, {queued} to check, {skipped} skipped")));
        }
        library::CheckEvent::Measured { done, queued, path, measurement } => {
            emit(Update::Progress(Some(*done as f32 / (*queued).max(1) as f32)));
            let findings = lossless::findings(measurement);
            let label = findings.first().map_or("ok", |f| f.label());
            emit(Update::Say(format!("[{done}/{queued}] {label:<9} {}", short(path))));
            if stopped(cancel) && !stop_note {
                stop_note = true;
                emit(Update::Log("stopping after the files already started…".into()));
            }
        }
        library::CheckEvent::Failed { done, queued, path, error } => {
            emit(Update::Log(format!("[{done}/{queued}] FAILED {}: {error}", short(path))));
        }
    })
    .map_err(|e| format!("{}: {e}", s.library.display()))?;
    store.save().map_err(|e| e.to_string())?;

    // The log gets every flagged file, because that list IS the answer; the terminal prints the
    // same thing. A clean library prints nothing and says so in one line.
    let mut flagged = 0;
    for c in &checked {
        let findings = lossless::findings(&c.measurement);
        if findings.is_empty() {
            continue;
        }
        flagged += 1;
        let labels: Vec<&str> = findings.iter().map(|f| f.label()).collect();
        emit(Update::Log(format!("{}  —  {}", short(&c.path), labels.join(", "))));
    }
    emit(Update::Progress(Some(1.0)));
    Ok(match flagged {
        0 => format!("Checked {} FLACs; none look like anything but the lossless audio they claim.", checked.len()),
        n => format!("Checked {} FLACs; {n} are worth a closer look — see the log.", checked.len()),
    })
}

/// Where a library folder's own name says it is, for the window's title bar.
pub fn short(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flint-gui-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A minimal FLAC: the magic, one STREAMINFO block, and a frame's worth of nothing. Enough for
    /// the scan to treat it as a track and for a copy to be a copy; it is never decoded here.
    fn write_flac(path: &Path, extra: usize) {
        let mut out = b"fLaC".to_vec();
        out.push(0x80); // last block, type 0 (STREAMINFO)
        out.extend_from_slice(&[0, 0, 34]);
        out.extend_from_slice(&[0u8; 34]);
        out.extend_from_slice(&vec![0xAAu8; extra]);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, out).unwrap();
    }

    fn settings(library: PathBuf, volume: PathBuf, cache: PathBuf) -> Settings {
        // A stated budget, because free space is only readable on Windows and these run anywhere.
        Settings { volumes: vec![volume], budget_gb: vec![Some(1.0)], cache, jobs: 1, ..Settings::new(library) }
    }

    /// The window's PLAN writes nothing, and its COPY writes what the plan said. This is the one
    /// property the whole "Copy is only offered for a plan you have seen" rule rests on.
    #[test]
    fn plan_writes_nothing_and_copy_writes_the_plan() {
        let root = tmp("sync");
        let library = root.join("music");
        let volume = root.join("player");
        fs::create_dir_all(&volume).unwrap();
        write_flac(&library.join("Artist - Album/01 One.flac"), 2048);
        write_flac(&library.join("Artist - Album/02 Two.flac"), 2048);
        fs::write(library.join("Artist - Album/cover.jpg"), b"jpeg").unwrap();

        let s = settings(library.clone(), volume.clone(), root.join("cache"));
        let cancel = AtomicBool::new(false);

        let mut planned = false;
        let mut lines = Vec::new();
        let mut library_facts = None;
        let mut volume_facts = None;
        let word = run(Job::Plan, &s, &cancel, &mut |u| match u {
            Update::Planned => planned = true,
            Update::Say(l) | Update::Log(l) => lines.push(l),
            Update::Library(f) => library_facts = Some(f),
            Update::Volume(i, f) => volume_facts = Some((i, f)),
            Update::Progress(_) => {}
        })
        .unwrap();
        assert!(planned, "a plan that shows something must enable COPY");
        // The meters are drawn from these, so a plan that does not report them draws empty
        // troughs over a real transfer.
        let lib = library_facts.expect("the window is told what the library holds");
        assert_eq!(lib.files, 3, "one FLAC, one MP3 and the cover");
        assert!(lib.bytes > 0);
        let (index, vol) = volume_facts.expect("the window is told what the volume holds");
        assert_eq!(index, 0);
        assert!(vol.budget > 0, "a volume with no budget draws as full");
        assert_eq!(vol.to_copy, lib.bytes, "everything in the library is going to the one volume");
        assert_eq!(vol.albums, 1);
        assert!(word.contains("Nothing has been written"), "{word}");
        assert_eq!(fs::read_dir(&volume).unwrap().count(), 0, "PLAN wrote to the volume");
        assert!(lines.iter().any(|l| l.contains("would copy")), "{lines:#?}");

        let word = run(Job::Apply, &s, &cancel, &mut |_| {}).unwrap();
        assert!(word.starts_with("Copied 3 files"), "{word}");
        assert!(volume.join("Artist - Album/01 One.flac").is_file());
        assert!(volume.join("Artist - Album/cover.jpg").is_file(), "the extras should travel");
        assert!(volume.join(sync::MANIFEST_NAME).is_file());

        // Run again: nothing left to do, and COPY is not offered because there is no plan to show.
        let mut planned = false;
        let word = run(Job::Plan, &s, &cancel, &mut |u| {
            if u == Update::Planned {
                planned = true;
            }
        })
        .unwrap();
        assert!(word.contains("nothing to do"), "{word}");
        assert!(!planned, "there is nothing to copy, so COPY must stay dark");
        fs::remove_dir_all(&root).unwrap();
    }

    /// Turning the extras switch off leaves the art on the PC. The switch has to reach the plan,
    /// not just the window.
    #[test]
    fn the_extras_switch_reaches_the_plan() {
        let root = tmp("extras");
        let library = root.join("music");
        let volume = root.join("player");
        fs::create_dir_all(&volume).unwrap();
        write_flac(&library.join("Artist - Album/01 One.flac"), 1024);
        fs::write(library.join("Artist - Album/cover.jpg"), b"jpeg").unwrap();

        let mut s = settings(library, volume.clone(), root.join("cache"));
        s.extras = false;
        run(Job::Apply, &s, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(volume.join("Artist - Album/01 One.flac").is_file());
        assert!(!volume.join("Artist - Album/cover.jpg").exists(), "extras were off");
        fs::remove_dir_all(&root).unwrap();
    }

    /// A sync with no destination says so rather than doing something surprising.
    #[test]
    fn a_sync_with_nowhere_to_go_is_an_error() {
        let root = tmp("nowhere");
        let s = Settings { volumes: Vec::new(), ..Settings::new(root.join("music")) };
        let e = run(Job::Plan, &s, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.contains("volume"), "{e}");
        let _ = fs::remove_dir_all(&root);
    }

    /// Analysing a library with no engine installed is a *result*, not a failure: it says what it
    /// could not do and reports what it does know. The window would otherwise show an error box to
    /// everyone who has not installed Music Center, which is most people.
    #[test]
    fn analysing_without_an_engine_reports_rather_than_fails() {
        let root = tmp("noengine");
        let library = root.join("music");
        write_flac(&library.join("Artist - Album/01 One.flac"), 512);
        let s = settings(library, root.join("player"), root.join("cache"));
        let mut lines = Vec::new();
        let word = run(Job::Scan, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::Say(l) | Update::Log(l) = u {
                lines.push(l);
            }
        });
        let word = word.expect("no engine must not fail the job");
        assert!(word.contains("1 tracks") || word.contains("1 track"), "{word}");
        assert!(lines.iter().any(|l| l.contains("no engine")), "{lines:#?}");
        fs::remove_dir_all(&root).unwrap();
    }
}
