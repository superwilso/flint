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
    /// The player's internal memory, by position — `volumes` loses which is which, and Cinder
    /// reads its palettes from the internal memory only.
    pub internal: Option<PathBuf>,
    /// The folder of `.palette` files on this PC.
    pub palette_dir: Option<PathBuf>,
    /// Palettes ▸ the editor ▸ `(file name, contents)` for Save.
    pub palette_draft: Option<(String, String)>,
    /// The file that draft was last saved as: the one file Save may replace.
    pub palette_saved: Option<String>,
    /// Palettes ▸ the shared palettes ▸ `(file name, contents)` for Install.
    pub shop_install: Option<(String, String)>,
    /// Settings ▸ Last.fm ▸ what was typed, for Save.
    pub api_key: String,
    pub api_secret: String,
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
            internal: None,
            palette_dir: None,
            palette_draft: None,
            palette_saved: None,
            shop_install: None,
            api_key: String::new(),
            api_secret: String::new(),
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
    /// A file the check flagged, for the Check page's table.
    Finding(crate::CheckRow),
    /// How many FLACs a finished check looked at. Sent once, after the findings, so the page can
    /// clear the previous run's rows when a new one starts.
    Checked(usize),
    /// What "Read the player" found.
    Player(Box<crate::PlayerFacts>),
    /// The playlists made on the player, read again after some were taken back.
    Playlists(Vec<crate::PlaylistRow>),
    /// The palette check's rows.
    Palettes(Vec<flint_core::palette::Row>),
    /// The editor's palette was written as this file.
    PaletteSaved(String),
    /// The shared palettes, as the shop shows them.
    Shop(Vec<flint_core::palette::SharedPalette>),
    /// A shared palette is in the folder now, and on the player too when `on_player`.
    Installed { file: String, on_player: bool },
    /// The Last.fm account changed: a key saved, a sign-in, a sign-out.
    Lastfm(crate::Lastfm),
    /// Open this page in the browser. Only the platform can.
    Open(String),
    /// The plays left in the logs after a send.
    Plays { plays: Vec<crate::PlayRow>, unreadable: usize },
    /// What Compare likes found; `None` once it has been carried out.
    Likes(Option<crate::LikesPlan>),
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
        Job::ReadPlayer => read_player_job(s, emit),
        Job::PullPlaylists => pull_playlists_job(s, emit),
        Job::CheckPalettes => palettes_job(s, false, emit),
        Job::SendPalettes => palettes_job(s, true, emit),
        Job::SavePalette => save_palette_job(s, emit),
        Job::FetchPalettes => fetch_palettes_job(s, cancel, emit),
        Job::FetchShop => shop_job(s, cancel, emit),
        Job::InstallShared => install_shared_job(s, emit),
        Job::LastfmKey => lastfm_key_job(s, emit),
        Job::LastfmSignIn => lastfm_sign_in_job(s, cancel, emit),
        Job::LastfmSignOut => lastfm_sign_out_job(s, emit),
        Job::Scrobble => scrobble_job(s, cancel, emit),
        Job::CompareLikes => likes_job(s, false, cancel, emit),
        Job::SyncLikes => likes_job(s, true, cancel, emit),
    }
}

// ── Last.fm ────────────────────────────────────────────────────────────────────────────────────
//
// The credentials live in the cache folder like everything else Flint keeps, and the window reads
// and writes the same file `flint lastfm` does.

fn lastfm_key_job(s: &Settings, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::lastfm::Credentials;
    let (key, secret) = (s.api_key.trim(), s.api_secret.trim());
    if key.is_empty() || secret.is_empty() {
        return Err("paste both the API key and the shared secret".into());
    }
    if key.chars().any(char::is_whitespace) || secret.chars().any(char::is_whitespace) {
        return Err("a key and a secret are one word each — check what was pasted".into());
    }
    let mut creds = Credentials::load_in(&s.cache);
    // A session belongs to the key that made it. A different key means signing in again.
    if creds.api_key != key {
        creds.session_key.clear();
        creds.username.clear();
    }
    creds.api_key = key.to_string();
    creds.api_secret = secret.to_string();
    let path = creds.save_in(&s.cache).map_err(|e| format!("could not save the key: {e}"))?;
    emit(Update::Log(format!("saved to {}", path.display())));
    emit(Update::Lastfm(crate::lastfm_facts(&creds)));
    Ok(if creds.is_ready() {
        "Last.fm key saved.".into()
    } else {
        "Last.fm key saved. Now sign in with Last.fm.".into()
    })
}

/// Signing in the way Last.fm asks desktop programs to: a token, its page in the browser, and then
/// asking every few seconds whether the person has allowed it yet. Stop ends the wait.
fn lastfm_sign_in_job(s: &Settings, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::lastfm::{self, Client, Credentials, Error};
    const ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(3);
    // The token is good for an hour; nobody is still on the page after ten minutes.
    const GIVE_UP: std::time::Duration = std::time::Duration::from_secs(600);
    let mut client = Client::new(Credentials::load_in(&s.cache)).map_err(|e| e.to_string())?;
    emit(Update::Progress(None));
    emit(Update::Say("asking Last.fm for a sign-in page…".into()));
    let token = client.token().map_err(|e| e.to_string())?;
    emit(Update::Open(lastfm::auth_url(&client.creds.api_key, &token)));
    emit(Update::Say("Waiting for you to allow Flint on Last.fm's page in your browser…".into()));
    let started = std::time::Instant::now();
    loop {
        let wait = std::time::Instant::now();
        while wait.elapsed() < ASK_EVERY {
            if stopped(cancel) {
                return Ok("Sign-in stopped. Nothing was saved.".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        match client.session_from_token(&token) {
            Ok((name, _)) => {
                client.creds.save_in(&s.cache).map_err(|e| format!("could not save the session: {e}"))?;
                emit(Update::Lastfm(crate::lastfm_facts(&client.creds)));
                return Ok(format!("Signed in to Last.fm as {name}."));
            }
            Err(Error::Api { code, .. }) if code == lastfm::NOT_AUTHORISED_YET => {}
            Err(e) => return Err(e.to_string()),
        }
        if started.elapsed() > GIVE_UP {
            return Err("Last.fm has not heard back from the browser in ten minutes. Press Sign in again.".into());
        }
    }
}

fn lastfm_sign_out_job(s: &Settings, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::lastfm::Credentials;
    let mut creds = Credentials::load_in(&s.cache);
    creds.session_key.clear();
    creds.username.clear();
    creds.save_in(&s.cache).map_err(|e| format!("could not save: {e}"))?;
    emit(Update::Lastfm(crate::lastfm_facts(&creds)));
    Ok("Signed out. Last.fm still lists Flint until you remove it at last.fm/settings/applications.".into())
}

fn scrobble_job(s: &Settings, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::{lastfm::Credentials, lastfm_sync};
    emit(Update::Progress(None));
    let report =
        lastfm_sync::scrobble(&s.volumes, Credentials::load_in(&s.cache), true, &|| stopped(cancel), &mut |l| {
            emit(Update::Log(l))
        })?;
    let (plays, unreadable) = plays_on(&s.volumes);
    emit(Update::Plays { plays, unreadable });
    let mut word = format!("Sent {} play(s): {} accepted", report.found, report.accepted);
    if report.ignored > 0 {
        word.push_str(&format!(", {} ignored", report.ignored));
    }
    if report.kept > 0 {
        word.push_str(&format!(", {} kept in the log — see why below", report.kept));
    }
    if let Some(why) = report.stopped {
        word.push_str(&format!(". Stopped: {why}"));
    }
    Ok(format!("{word}."))
}

fn likes_job(s: &Settings, apply: bool, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::{lastfm::Credentials, lastfm_sync};
    emit(Update::Progress(None));
    let report = lastfm_sync::likes(
        &s.volumes,
        Credentials::load_in(&s.cache),
        &s.cache.join("likes-state.tsv"),
        apply,
        false,
        &|| stopped(cancel),
        &mut |l| emit(Update::Log(l)),
    )?;
    let p = &report.plan;
    let plan = crate::LikesPlan {
        liked: p.liked.len(),
        device_add: p.device_add.len(),
        device_remove: p.device_remove.len(),
        lastfm_love: p.lastfm_love.len(),
        lastfm_unlove: p.lastfm_unlove.len(),
    };
    if !apply {
        emit(Update::Likes(Some(plan)));
        return Ok(match plan.changes() {
            0 => format!("Likes are in step: {} on both sides. Nothing to change.", plan.liked),
            n => format!("{n} change(s) would keep likes in step. Nothing has been written yet."),
        });
    }
    emit(Update::Likes(None));
    if !report.pushed && report.loved + report.unloved == 0 {
        return Ok("Nothing was changed.".into());
    }
    Ok(format!(
        "Likes in step: {} loved and {} unloved on Last.fm{}.",
        report.loved,
        report.unloved,
        if report.pushed { "; the player takes in the list on its next start" } else { "" }
    ))
}

/// The plays in each volume's `.scrobbler.log`, newest first, and how many rows could not be read.
fn plays_on(roots: &[PathBuf]) -> (Vec<crate::PlayRow>, usize) {
    let (mut plays, mut unreadable) = (Vec::new(), 0);
    for root in roots {
        if let Ok(body) = std::fs::read_to_string(root.join(".scrobbler.log")) {
            let parsed = flint_core::scrobblelog::parse(&body);
            unreadable += parsed.unreadable.len();
            plays.extend(parsed.entries.iter().map(|e| crate::PlayRow {
                when: e.timestamp,
                track: e.track.clone(),
                artist: e.artist.clone(),
                kind: if e.is_play() { "PLAY".into() } else { "SKIP".into() },
            }));
        }
    }
    plays.sort_by_key(|p| std::cmp::Reverse(p.when));
    (plays, unreadable)
}

/// The `.palette` files in `dir`, as `(file name, contents)`. A file too big to be a palette, or
/// one that cannot be read as text, is logged and left out — the player would skip it too.
fn read_palettes(dir: &Path, emit: &mut dyn FnMut(Update)) -> Vec<(String, String)> {
    use flint_core::palette::{palette_stem, MAX_BYTES};
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if palette_stem(&name).is_none() {
            continue;
        }
        if e.metadata().map(|m| m.len() > MAX_BYTES).unwrap_or(true) {
            emit(Update::Log(format!("{name}: too large to be a palette")));
            continue;
        }
        match std::fs::read_to_string(e.path()) {
            Ok(body) => out.push((name, body)),
            Err(err) => emit(Update::Log(format!("{name}: {err}"))),
        }
    }
    out.sort();
    out
}

/// CHECK and SEND are one function, like PLAN and COPY: what Send copies is what the check showed.
fn palettes_job(s: &Settings, send: bool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::palette::{compare, to_send, State, DIR_NAME};
    let pc = s.palette_dir.as_deref().map(|d| read_palettes(d, emit)).unwrap_or_default();
    let player_dir = s.internal.as_ref().map(|v| v.join(DIR_NAME));
    let player = player_dir.as_deref().map(|d| read_palettes(d, emit)).unwrap_or_default();
    let mut rows = compare(&pc, &player);
    let mut sent = 0;
    if send {
        let dir = player_dir.ok_or("choose the player's internal memory on the Sync page first")?;
        let src = s.palette_dir.as_ref().ok_or("choose the palettes folder first")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for file in to_send(&rows) {
            let body = std::fs::read(src.join(file)).map_err(|e| format!("{file}: {e}"))?;
            // Lowercase on the player: the id is the lowercased stem, and FAT will keep whatever
            // case is written — one spelling avoids two files the player treats as one.
            let dest = dir.join(file.to_ascii_lowercase());
            let tmp = dest.with_extension("palette.tmp");
            std::fs::write(&tmp, &body).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            emit(Update::Log(format!("sent {file}")));
            sent += 1;
        }
        let player = read_palettes(&dir, emit);
        rows = compare(&pc, &player);
    }
    let count = |st: State| rows.iter().filter(|r| r.state == st).count();
    let (new, changed, refused) = (count(State::New), count(State::Changed), count(State::Refused));
    emit(Update::Palettes(rows));
    let mut word =
        if send { format!("sent {sent} palette{}", if sent == 1 { "" } else { "s" }) } else { String::new() };
    if !send || new + changed + refused > 0 {
        let parts: Vec<String> = [(new, "new"), (changed, "changed"), (refused, "refused")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, w)| format!("{n} {w}"))
            .collect();
        let tail = if parts.is_empty() { "all on the player".to_string() } else { parts.join(", ") };
        word = if word.is_empty() { format!("palettes: {tail}") } else { format!("{word}; {tail}") };
    }
    Ok(word)
}

/// Write `path` whole or not at all: a temporary name, then a rename.
fn write_whole(path: &Path, body: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("palette.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Save the editor's palette into the folder, then check the folder so the table shows it.
///
/// It replaces only the file this palette was last saved as. Any other file of the same name is
/// someone's own work — a palette they downloaded or wrote by hand — and is never overwritten.
fn save_palette_job(s: &Settings, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::palette::check_file;
    let dir = s.palette_dir.as_ref().ok_or("choose the palettes folder first")?;
    let (file, body) = s.palette_draft.as_ref().ok_or("there is no palette to save")?;
    match check_file(file, body) {
        Some((_, Ok(_))) => {}
        Some((_, Err(errs))) => return Err(errs.first().cloned().unwrap_or_default()),
        None => return Err(format!("{file} is not a palette file name")),
    }
    let dest = dir.join(file);
    let ours = s.palette_saved.as_deref() == Some(file.as_str());
    if dest.exists() && !ours && std::fs::read_to_string(&dest).ok().as_deref() != Some(body.as_str()) {
        return Err(format!("{file} is already in the folder — give the palette another name"));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_whole(&dest, body.as_bytes())?;
    emit(Update::PaletteSaved(file.clone()));
    emit(Update::Log(format!("saved {}", dest.display())));
    let word = palettes_job(s, false, emit)?;
    Ok(format!("saved {file}; {word}"))
}

/// Download the shared palettes into the folder, then check it.
///
/// A file already in the folder is kept whatever the shared copy says: the folder is the person's,
/// and one they have edited must not be put back. Every download is checked with the player's rules
/// before it is written, so a palette the player would refuse never lands in the folder.
fn fetch_palettes_job(s: &Settings, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::http::get_form;
    use flint_core::palette::{check_file, shared_index, MAX_BYTES, SHARED_INDEX, SHARED_RAW};
    let dir = s.palette_dir.as_ref().ok_or("choose the palettes folder first")?;
    emit(Update::Say("reading the list of shared palettes".into()));
    let index = get_form(SHARED_INDEX, &[]).map_err(|e| format!("could not reach the shared palettes: {e}"))?;
    if index.status != 200 {
        return Err(format!("the shared palettes answered {}", index.status));
    }
    let files = shared_index(&index.body);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (mut got, mut kept, mut refused) = (0, 0, 0);
    for (i, file) in files.iter().enumerate() {
        if stopped(cancel) {
            break;
        }
        emit(Update::Progress(Some(i as f32 / files.len().max(1) as f32)));
        let dest = dir.join(file);
        if dest.exists() {
            kept += 1;
            continue;
        }
        let r = match get_form(&format!("{SHARED_RAW}{file}"), &[]) {
            Ok(r) if r.status == 200 => r,
            Ok(r) => {
                emit(Update::Log(format!("{file}: the server answered {}", r.status)));
                continue;
            }
            Err(e) => {
                emit(Update::Log(format!("{file}: {e}")));
                continue;
            }
        };
        if r.body.len() as u64 > MAX_BYTES {
            emit(Update::Log(format!("{file}: too large to be a palette")));
            refused += 1;
            continue;
        }
        if let Some((_, Err(errs))) = check_file(file, &r.body) {
            emit(Update::Log(format!("{file}: not downloaded — {}", errs.first().cloned().unwrap_or_default())));
            refused += 1;
            continue;
        }
        write_whole(&dest, r.body.as_bytes())?;
        emit(Update::Log(format!("downloaded {file}")));
        got += 1;
    }
    emit(Update::Progress(None));
    let word = palettes_job(s, false, emit)?;
    let mut parts = vec![format!("downloaded {got} shared palette{}", if got == 1 { "" } else { "s" })];
    if kept > 0 {
        parts.push(format!("{kept} already in the folder"));
    }
    if refused > 0 {
        parts.push(format!("{refused} refused"));
    }
    Ok(format!("{}; {word}", parts.join(", ")))
}

/// Read the shared list and every file on it, check each with the player's rules, and say where a
/// copy already is. Writes nothing.
fn shop_job(s: &Settings, cancel: &AtomicBool, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::http::get_form;
    use flint_core::palette::{shared_index, Have, SharedPalette, DIR_NAME, SHARED_INDEX, SHARED_RAW};
    emit(Update::Say("reading the list of shared palettes".into()));
    let index = get_form(SHARED_INDEX, &[]).map_err(|e| format!("could not reach the shared palettes: {e}"))?;
    if index.status != 200 {
        return Err(format!("the shared palettes answered {}", index.status));
    }
    let files = shared_index(&index.body);
    let player_dir = s.internal.as_ref().map(|v| v.join(DIR_NAME));
    let read = |dir: Option<&PathBuf>, file: &str| dir.and_then(|d| std::fs::read_to_string(d.join(file)).ok());
    let mut items = Vec::new();
    for (i, file) in files.iter().enumerate() {
        if stopped(cancel) {
            break;
        }
        emit(Update::Progress(Some(i as f32 / files.len().max(1) as f32)));
        let body = match get_form(&format!("{SHARED_RAW}{file}"), &[]) {
            Ok(r) if r.status == 200 => r.body,
            Ok(r) => {
                emit(Update::Log(format!("{file}: the server answered {}", r.status)));
                continue;
            }
            Err(e) => {
                emit(Update::Log(format!("{file}: {e}")));
                continue;
            }
        };
        let mut p = SharedPalette::new(file, &body);
        p.folder = Have::of(read(s.palette_dir.as_ref(), file).as_deref(), &body);
        p.player = Have::of(read(player_dir.as_ref(), file).as_deref(), &body);
        items.push(p);
    }
    emit(Update::Progress(None));
    let n = items.len();
    emit(Update::Shop(items));
    Ok(format!("{n} shared palette{}", if n == 1 { "" } else { "s" }))
}

/// Put one shared palette in the folder, then on the player when its internal memory is chosen and
/// there, then check the folder so the table agrees. A different file of the same name in the
/// folder is someone's own and is never replaced.
fn install_shared_job(s: &Settings, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::palette::{check_file, DIR_NAME};
    let dir = s.palette_dir.as_ref().ok_or("choose the palettes folder first")?;
    let (file, body) = s.shop_install.as_ref().ok_or("there is no palette to install")?;
    match check_file(file, body) {
        Some((_, Ok(_))) => {}
        Some((_, Err(errs))) => return Err(errs.first().cloned().unwrap_or_default()),
        None => return Err(format!("{file} is not a palette file name")),
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dest = dir.join(file);
    match std::fs::read_to_string(&dest) {
        Ok(have) if have == *body => {}
        Ok(_) => return Err(format!("a different {file} is already in the folder; rename it to install this one")),
        Err(_) => {
            write_whole(&dest, body.as_bytes())?;
            emit(Update::Log(format!("downloaded {file}")));
        }
    }
    let mut on_player = false;
    let mut word = format!("{file} is in the folder");
    if let Some(root) = &s.internal {
        if root.is_dir() {
            let pdir = root.join(DIR_NAME);
            std::fs::create_dir_all(&pdir).map_err(|e| format!("{}: {e}", pdir.display()))?;
            let pdest = pdir.join(file.to_ascii_lowercase());
            match std::fs::read_to_string(&pdest) {
                Ok(have) if have != *body => {
                    word.push_str("; a different one of that name is on the player, so it was left alone");
                }
                have => {
                    if have.is_err() {
                        write_whole(&pdest, body.as_bytes())?;
                        emit(Update::Log(format!("sent {file}")));
                    }
                    on_player = true;
                    word = format!("installed {file}: pick it on the player in Settings ▸ Display ▸ Palette");
                }
            }
        } else {
            word.push_str("; the player is not connected, so Send it later");
        }
    }
    emit(Update::Installed { file: file.clone(), on_player });
    palettes_job(s, false, emit)?;
    Ok(word)
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
        analysis.cached_key(&path).unwrap_or_default()
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
        // The preview. Every removal is listed, because deleting is the one thing a sync does that
        // cannot be taken back, and a list cut short would ask for a Copy nobody has checked. The
        // copies are listed up to a point — past it the count says what the list cannot. The log
        // scrolls, and it ends on the totals, so the tail it opens on says what the whole plan does.
        const COPIES_LISTED: usize = 500;
        const REMOVALS_LISTED: usize = 1_500;
        for c in plan.copies.iter().take(COPIES_LISTED) {
            emit(Update::Log(format!("would copy    {}{}", c.rel, if c.tag.is_empty() { "" } else { "  +SensMe" })));
        }
        if plan.copies.len() > COPIES_LISTED {
            emit(Update::Log(format!("…and {} more to copy", plan.copies.len() - COPIES_LISTED)));
        }
        for (v, rel) in plan.stale_files.iter().take(REMOVALS_LISTED) {
            emit(Update::Log(format!("would remove  {}/{rel}", volumes[*v].root.display())));
        }
        if plan.stale_files.len() > REMOVALS_LISTED {
            emit(Update::Log(format!("…and {} more to remove", plan.stale_files.len() - REMOVALS_LISTED)));
        }
        for (v, name) in &plan.stale_playlists {
            emit(Update::Log(format!("would remove  playlist {name} ({})", volumes[*v].root.display())));
        }
        let removes = plan.stale_files.len() + plan.stale_playlists.len();
        emit(Update::Log(format!(
            "in all: {} to copy, {} to remove{}",
            crate::thousands(plan.copies.len()),
            crate::thousands(removes),
            if plan.copies.len() + removes > 20 { " — scroll up to see each one" } else { "" }
        )));
        emit(Update::Planned);
        emit(Update::Progress(Some(1.0)));
        return Ok(format!(
            "Nothing has been written. {} files would be copied — press Copy to the player.",
            plan.copies.len()
        ));
    }

    // Stop is asked before each copy, so the one in hand finishes and nothing is half-written.
    let out = apply::apply(
        &plan,
        &s.library,
        &volumes,
        &mut manifests,
        |c| analysis.blob(&c.tag).ok(),
        false,
        || stopped(cancel),
        |ev| match ev {
            apply::Event::Copied { done, total, rel, tagged, .. } => {
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

    // An album that changed volume took its files with it; its ratings and play counts are keyed
    // by where the file was, so they are moved to where it is now. Not worth failing a finished
    // copy over: a line in the log either way.
    let drives: Vec<PathBuf> = s.volumes.iter().map(|root| flint_core::stats::drive_root(root)).collect();
    if let Err(e) = flint_core::stats::follow_player(&drives, true, &mut |l| emit(Update::Log(l.to_string()))) {
        emit(Update::Log(format!("ratings and play counts were not updated: {e}")));
    }

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
    if out.stopped {
        let left = plan.copies.len() - out.copied - out.failed;
        done.push_str(&format!(" Stopped with {left} left to copy — Show what would happen picks up from here."));
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
    // same thing. A clean library prints nothing and says so in one line. The Check page gets the
    // same list as rows: the first finding is the verdict, all of them together are the "why".
    let mut flagged = 0;
    for c in &checked {
        let findings = lossless::findings(&c.measurement);
        if findings.is_empty() {
            continue;
        }
        flagged += 1;
        let labels: Vec<&str> = findings.iter().map(|f| f.label()).collect();
        emit(Update::Log(format!("{}  —  {}", short(&c.path), labels.join(", "))));
        let file = c.path.strip_prefix(&s.library).unwrap_or(&c.path).display().to_string();
        let why: Vec<String> = findings.iter().map(|f| f.to_string()).collect();
        emit(Update::Finding(crate::CheckRow { verdict: labels[0].to_string(), file, why: why.join("; ") }));
    }
    emit(Update::Checked(checked.len()));
    emit(Update::Progress(Some(1.0)));
    Ok(match flagged {
        0 => format!("Checked {} FLACs; none look like anything but the lossless audio they claim.", checked.len()),
        n => format!("Checked {} FLACs; {n} are worth a closer look — see the log.", checked.len()),
    })
}

/// Read what is on the player, for the On the player, Likes & plays and Palettes pages. Reads only:
/// the manifests, the music folders, `.scrobbler.log`, `cinder_loved.tsv` and `cinder_palettes/`.
fn read_player_job(s: &Settings, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    if s.volumes.is_empty() {
        return Err("choose the player's drive first".into());
    }
    emit(Update::Progress(None));
    let mut facts = crate::PlayerFacts { read: true, ..Default::default() };
    for (i, root) in s.volumes.iter().enumerate() {
        emit(Update::Say(format!("reading {}", root.display())));
        let scan = sync::scan_volume(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let manifest = sync::Manifest::load(root);
        facts.albums.extend(albums_on(i, &scan, &manifest));
        facts.likes += flint_core::likes::read_loved(&flint_core::likes::Volume::new(root, "")).len();

        if let Ok(dir) = std::fs::read_dir(root.join("cinder_palettes")) {
            for e in dir.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.to_ascii_lowercase().ends_with(".palette") {
                    let bytes = e.metadata().map_or(0, |m| m.len());
                    facts.palettes.push(crate::PaletteFile { volume: i, name, bytes });
                }
            }
        }
    }
    (facts.plays, facts.unreadable) = plays_on(&s.volumes);
    facts.palettes.sort_by_key(|p| p.name.to_lowercase());
    remembered_by_the_player(s, &mut facts, emit);
    let summary = format!(
        "{} albums on the player, {} plays in the log, {} songs liked",
        crate::thousands(facts.albums.len()),
        crate::thousands(facts.plays.len()),
        crate::thousands(facts.likes)
    );
    emit(Update::Player(Box::new(facts)));
    emit(Update::Progress(Some(1.0)));
    Ok(summary)
}

/// Where the window puts the playlists it takes back: a folder of their own inside the PC's
/// playlists folder. A sync reads the playlists folder's top level only, so what lands here is
/// kept on the PC without being sent back to the player as a second copy of a playlist the player
/// already has.
pub const PULLED_FOLDER: &str = "From the player";

/// Take the playlists the player has changed back to the PC and take their EDITED mark off
/// (`flint_core::playlists::pull`). The tracks are named as the files in the music folder, so the
/// playlist opens in any player on the PC.
fn pull_playlists_job(s: &Settings, emit: &mut dyn FnMut(Update)) -> Result<String, String> {
    use flint_core::{playlists, stats};
    let Some(internal) = s.volumes.first().map(|root| stats::drive_root(root)) else {
        return Err("choose the player's drive first".into());
    };
    let Some(folder) = &s.playlists else {
        return Err("choose a playlists folder on the Sync page first: that is where they go".into());
    };
    emit(Update::Progress(None));
    let to = folder.join(PULLED_FOLDER);
    let library = (!s.library.as_os_str().is_empty()).then_some(s.library.as_path());
    let report = playlists::pull(&internal, &to, library, true, &mut |l| emit(Update::Log(l.to_string())))?;
    // Read them again: the page's count of playlists waiting is what just changed.
    let rows = playlists::read_player(&internal)
        .map_err(|e| format!("{}: {e}", internal.display()))?
        .into_iter()
        .map(|l| crate::PlaylistRow { name: l.name, tracks: l.tracks, edited: l.edited.is_some() })
        .collect();
    emit(Update::Playlists(rows));
    emit(Update::Progress(Some(1.0)));
    Ok(match (report.pulled, report.failed) {
        (0, 0) => "No playlist on the player has changed since it was last taken back.".to_string(),
        (n, 0) => format!("Took {n} playlist(s) back to {}. Their EDITED marks are off.", to.display()),
        (n, f) => {
            format!("Took {n} playlist(s) back to {}; {f} failed and kept their marks — see the log.", to.display())
        }
    })
}

/// What the player keeps for itself, on its internal memory: ratings and play counts
/// (`cinder_stats.tsv`), saved views (`cinder_views.conf`) and the playlists made on it
/// (`cinder_playlists`). The stars and plays go onto the album rows; the views and the playlists
/// are named in the log, where a list of any length can be read.
fn remembered_by_the_player(s: &Settings, facts: &mut crate::PlayerFacts, emit: &mut dyn FnMut(Update)) {
    use flint_core::{playlists, stats, views};
    let drives: Vec<PathBuf> = s.volumes.iter().map(|root| stats::drive_root(root)).collect();
    let Some(internal) = drives.first() else { return };

    let remembered = stats::Stats::load(&internal.join(stats::FILE_NAME));
    facts.rated = remembered.tracks.values().filter(|t| t.rating > 0).count();
    facts.counted = remembered.tracks.values().filter(|t| t.plays > 0).count();
    // (volume, album folder) -> its tracks' ratings and plays. A row belongs to the album whose
    // folder its file is in, on the volume the player says it is on.
    let mut by_album: std::collections::HashMap<(usize, String), (Vec<u8>, u32)> = std::collections::HashMap::new();
    for (path, stat) in &remembered.tracks {
        let Some(pc) = stats::pc_path(&drives, path) else { continue };
        let Some((v, root)) = s.volumes.iter().enumerate().find(|(_, root)| pc.starts_with(root)) else { continue };
        let Ok(rel) = pc.strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let folder = rel.rsplit_once('/').map_or(".", |(f, _)| f).to_string();
        let entry = by_album.entry((v, folder)).or_default();
        entry.0.push(stat.rating);
        entry.1 = entry.1.saturating_add(stat.plays);
    }
    for album in &mut facts.albums {
        if let Some((ratings, plays)) = by_album.get(&(album.volume, album.folder.clone())) {
            album.rating = stats::album_rating(ratings.iter().copied());
            album.plays = *plays;
        }
    }

    facts.views = views::load(&internal.join(views::FILE_NAME))
        .into_iter()
        .map(|v| {
            let rules = v.describe();
            (v.name, rules)
        })
        .collect();
    for (name, rules) in &facts.views {
        emit(Update::Log(format!("smart playlist  {name} — {rules}")));
    }
    match playlists::read_player(internal) {
        Ok(lists) => {
            for l in lists {
                emit(Update::Log(format!(
                    "playlist  {}{} — {} tracks",
                    l.name,
                    if l.edited.is_some() { "  (changed on the player)" } else { "" },
                    l.tracks
                )));
                facts.playlists.push(crate::PlaylistRow { name: l.name, tracks: l.tracks, edited: l.edited.is_some() });
            }
        }
        Err(e) => emit(Update::Log(format!("could not read the player's playlists: {e}"))),
    }
}

/// The album folders on volume `v`: a file's folder is its album, and a folder is Flint's when any
/// file in it is in the manifest.
fn albums_on(v: usize, scan: &sync::DeviceScan, manifest: &sync::Manifest) -> Vec<crate::AlbumRow> {
    use std::collections::BTreeMap;
    struct Acc {
        files: usize,
        bytes: u64,
        by_flint: bool,
        formats: BTreeMap<String, usize>,
    }
    let mut folders: BTreeMap<String, Acc> = BTreeMap::new();
    for (rel, (size, _)) in &scan.files {
        if rel == sync::MANIFEST_NAME || sync::is_sidecar_path(rel) {
            continue;
        }
        let folder = rel.rsplit_once('/').map_or(".", |(f, _)| f).to_string();
        let ext = rel.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
        let format = match ext.as_str() {
            "flac" => "FLAC",
            "mp3" => "MP3",
            "m4a" | "aac" | "mp4" => "AAC",
            "wav" => "WAV",
            "aif" | "aiff" => "AIFF",
            "dsf" | "dff" => "DSD",
            "ogg" | "opus" => "Ogg",
            _ => "other",
        };
        let acc =
            folders.entry(folder).or_insert(Acc { files: 0, bytes: 0, by_flint: false, formats: BTreeMap::new() });
        acc.files += 1;
        acc.bytes += size;
        acc.by_flint |= manifest.records.contains_key(rel);
        *acc.formats.entry(format.to_string()).or_default() += 1;
    }
    folders
        .into_iter()
        .map(|(folder, a)| crate::AlbumRow {
            folder,
            volume: v,
            files: a.files,
            bytes: a.bytes,
            format: a.formats.into_iter().max_by_key(|(_, n)| *n).map(|(f, _)| f).unwrap_or_default(),
            by_flint: a.by_flint,
            rating: None,
            plays: 0,
        })
        .collect()
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
            _ => {}
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

    /// An album the plan moves to the other volume takes its rating and its play count with it.
    /// They are kept by the player under the file's path (`cinder_stats.tsv`), so a move that left
    /// them behind would show the album unrated and unplayed after a sync.
    #[test]
    fn a_rating_follows_an_album_the_sync_moves_to_the_card() {
        let root = tmp("follow");
        let library = root.join("music");
        let (internal, card) = (root.join("internal"), root.join("card"));
        fs::create_dir_all(&internal).unwrap();
        fs::create_dir_all(&card).unwrap();
        write_flac(&library.join("Artist - Album/01 One.flac"), 2048);
        let cancel = AtomicBool::new(false);
        let with_budgets = |internal_gb: f64, card_gb: f64| Settings {
            volumes: vec![internal.clone(), card.clone()],
            budget_gb: vec![Some(internal_gb), Some(card_gb)],
            cache: root.join("cache"),
            jobs: 1,
            ..Settings::new(library.clone())
        };

        // First the album goes to internal memory, and the player rates and plays it there.
        run(Job::Apply, &with_budgets(1.0, 0.0), &cancel, &mut |_| {}).unwrap();
        assert!(internal.join("Artist - Album/01 One.flac").is_file());
        let stats = internal.join(flint_core::stats::FILE_NAME);
        fs::write(&stats, "#CINDER-STATS/1\n/contents/Artist - Album/01 One.flac\t5\t12\t900\n").unwrap();

        // Then internal memory has no room for it, so the plan moves it to the card.
        let mut lines = Vec::new();
        run(Job::Apply, &with_budgets(0.0, 1.0), &cancel, &mut |u| {
            if let Update::Log(l) = u {
                lines.push(l);
            }
        })
        .unwrap();
        assert!(card.join("Artist - Album/01 One.flac").is_file() && !internal.join("Artist - Album").exists());
        let after = fs::read_to_string(&stats).unwrap();
        assert!(after.contains("/contents_ext/Artist - Album/01 One.flac\t5\t12\t900"), "{after}");
        assert!(!after.contains("/contents/Artist"), "the old row was left behind: {after}");
        assert!(lines.iter().any(|l| l.contains("their history went with them")), "{lines:#?}");
        fs::remove_dir_all(&root).unwrap();
    }

    /// Stop during Copy stops. It used to be drawn, change nothing, and end a run that had copied
    /// every file with "Stopped early."
    #[test]
    fn a_stopped_copy_copies_nothing_more_and_says_so() {
        let root = tmp("stop");
        let library = root.join("music");
        let volume = root.join("player");
        fs::create_dir_all(&volume).unwrap();
        write_flac(&library.join("Artist - Album/01 One.flac"), 2048);
        write_flac(&library.join("Artist - Album/02 Two.flac"), 2048);
        let s = settings(library, volume.clone(), root.join("cache"));
        let cancel = AtomicBool::new(true);
        let word = run(Job::Apply, &s, &cancel, &mut |_| {}).unwrap();
        assert!(word.contains("Stopped with 2 left to copy"), "{word}");
        assert!(!volume.join("Artist - Album/01 One.flac").exists(), "a copy was made after Stop");
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

    /// "Read the player" reads a real volume: albums grouped by folder and marked Flint's from the
    /// manifest, plays newest first, likes, palettes — and writes nothing.
    #[test]
    fn reading_the_player_finds_albums_plays_likes_and_palettes() {
        let vol = tmp("readplayer");
        fs::create_dir_all(vol.join("MUSIC/A/One")).unwrap();
        fs::create_dir_all(vol.join("MUSIC/B/Two")).unwrap();
        fs::create_dir_all(vol.join("cinder_palettes")).unwrap();
        fs::write(vol.join("MUSIC/A/One/01.flac"), b"fLaC....").unwrap();
        fs::write(vol.join("MUSIC/A/One/02.flac"), b"fLaC....").unwrap();
        fs::write(vol.join("MUSIC/B/Two/01.mp3"), b"ID3.....").unwrap();
        fs::write(vol.join("cinder_palettes/moss.palette"), b"name=Moss\n").unwrap();
        fs::write(vol.join("cinder_palettes/readme.txt"), b"not a palette").unwrap();
        let mut man = sync::Manifest::default();
        man.records.insert(
            "MUSIC/A/One/01.flac".into(),
            sync::Record { source_size: 8, source_mtime: 0, copy_size: 8, tag: String::new() },
        );
        man.save(&vol).unwrap();
        fs::write(
            vol.join(".scrobbler.log"),
            "#AUDIOSCROBBLER/1.1\n#TZ/UNKNOWN\n#CLIENT/Cinder\n             A\tAl\tFirst\t1\t200\tL\t1700000000\t\n             A\tAl\tSecond\t2\t200\tS\t1700000300\t\n",
        )
        .unwrap();
        fs::write(vol.join("cinder_loved.tsv"), "A\tFirst\n").unwrap();
        let before: Vec<_> = walk(&vol);

        let mut s = Settings::new(PathBuf::new());
        s.volumes = vec![vol.clone()];
        let mut facts = None;
        let word = run(Job::ReadPlayer, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::Player(f) = u {
                facts = Some(*f);
            }
        })
        .unwrap();
        let f = facts.expect("the job reports what it read");
        assert!(f.read);
        let folders: Vec<(&str, bool, &str)> =
            f.albums.iter().map(|a| (a.folder.as_str(), a.by_flint, a.format.as_str())).collect();
        assert_eq!(folders, vec![("MUSIC/A/One", true, "FLAC"), ("MUSIC/B/Two", false, "MP3")]);
        assert_eq!(f.albums[0].files, 2);
        assert_eq!(
            f.plays.iter().map(|p| (p.track.as_str(), p.kind.as_str())).collect::<Vec<_>>(),
            vec![("Second", "SKIP"), ("First", "PLAY")],
            "newest first"
        );
        assert_eq!(f.likes, 1);
        assert_eq!(f.palettes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["moss.palette"]);
        assert!(word.contains("2 albums"), "{word}");
        assert_eq!(walk(&vol), before, "reading the player wrote to it");
        let _ = fs::remove_dir_all(&vol);
    }

    /// What the player keeps for itself reaches the page: stars and plays on the album rows, the
    /// saved views and the playlists made on it. Reading writes nothing; taking the changed
    /// playlists back writes the PC's copy, in the music folder's own paths, and only then takes
    /// the EDITED mark off the player's file.
    #[test]
    fn the_players_own_data_is_read_and_changed_playlists_are_taken_back() {
        let root = tmp("remembered");
        let (vol, library, pc) = (root.join("player"), root.join("music"), root.join("playlists"));
        fs::create_dir_all(vol.join("MUSIC/A/One")).unwrap();
        fs::create_dir_all(vol.join("MUSIC/B/Two")).unwrap();
        fs::create_dir_all(vol.join("cinder_playlists")).unwrap();
        fs::create_dir_all(library.join("A/One")).unwrap();
        for f in ["MUSIC/A/One/01.flac", "MUSIC/A/One/02.flac", "MUSIC/B/Two/01.mp3"] {
            fs::write(vol.join(f), b"fLaC....").unwrap();
        }
        fs::write(library.join("A/One/01.flac"), b"fLaC....").unwrap();
        fs::write(
            vol.join("cinder_stats.tsv"),
            "#CINDER-STATS/1\n/contents/MUSIC/A/One/01.flac\t5\t3\t900\n/contents/MUSIC/A/One/02.flac\t0\t2\t800\n\
             /contents/MUSIC/B/Two/01.mp3\t4\t0\t0\n/contents/MUSIC/Gone/01.flac\t1\t9\t1\n",
        )
        .unwrap();
        fs::write(vol.join("cinder_views.conf"), "[Late favourites]\nrating=4\nsort=plays\n").unwrap();
        let late = "#EXTM3U\n#PLAYLIST:Late Night\n#CINDER-EDITED:1790000000\n/contents/MUSIC/A/One/01.flac\n/contents/MUSIC/B/Two/01.mp3\n";
        fs::write(vol.join("cinder_playlists/late.m3u8"), late).unwrap();
        fs::write(vol.join("cinder_playlists/walk.m3u8"), "#EXTM3U\n/contents/MUSIC/A/One/02.flac\n").unwrap();
        let before: Vec<_> = walk(&vol);

        let mut s = Settings::new(library.clone());
        s.volumes = vec![vol.clone()];
        let (mut facts, mut lines) = (None, Vec::new());
        run(Job::ReadPlayer, &s, &AtomicBool::new(false), &mut |u| match u {
            Update::Player(f) => facts = Some(*f),
            Update::Log(l) => lines.push(l),
            _ => {}
        })
        .unwrap();
        let f = facts.expect("the job reports what it read");
        assert_eq!((f.rated, f.counted), (3, 3), "every row in the file, whether or not its album is still here");
        let albums: Vec<(&str, Option<u8>, u32)> =
            f.albums.iter().map(|a| (a.folder.as_str(), a.rating, a.plays)).collect();
        assert_eq!(albums, vec![("MUSIC/A/One", Some(5), 5), ("MUSIC/B/Two", Some(4), 0)]);
        assert_eq!(f.views, vec![("Late favourites".to_string(), "4 stars and up · most played first".to_string())]);
        let lists: Vec<(&str, usize, bool)> =
            f.playlists.iter().map(|p| (p.name.as_str(), p.tracks, p.edited)).collect();
        assert_eq!(lists, vec![("Late Night", 2, true), ("walk", 1, false)]);
        assert!(lines.iter().any(|l| l.starts_with("smart playlist  Late favourites")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("Late Night  (changed on the player)")), "{lines:#?}");
        assert_eq!(walk(&vol), before, "reading the player wrote to it");

        // With nowhere to put them, nothing is taken and nothing changes.
        assert!(run(Job::PullPlaylists, &s, &AtomicBool::new(false), &mut |_| {}).is_err());
        assert_eq!(walk(&vol), before);

        s.playlists = Some(pc.clone());
        let mut rows = None;
        let word = run(Job::PullPlaylists, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::Playlists(r) = u {
                rows = Some(r);
            }
        })
        .unwrap();
        assert!(word.starts_with("Took 1 playlist(s) back"), "{word}");
        assert!(rows.expect("the page is told").iter().all(|p| !p.edited), "the mark is off, so nothing is waiting");
        let on_pc = fs::read_to_string(pc.join(PULLED_FOLDER).join("late.m3u8")).unwrap();
        let in_library = std::path::absolute(library.join("A").join("One").join("01.flac")).unwrap();
        assert_eq!(on_pc, format!("#EXTM3U\n#PLAYLIST:Late Night\n{}\nB/Two/01.mp3\n", in_library.display()));
        assert_eq!(
            fs::read_to_string(vol.join("cinder_playlists/late.m3u8")).unwrap(),
            late.replace("#CINDER-EDITED:1790000000\n", "")
        );
        assert!(
            !pc.join(PULLED_FOLDER).join("walk.m3u8").exists(),
            "a playlist the player has not changed stays where it is"
        );
        // What was taken back is kept on the PC, not sent to the player again by the next sync.
        assert!(sync::read_playlists(&pc, &library).unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    /// CHECK writes nothing. SEND copies what the check called new or changed — lowercased, through
    /// a temporary name, never a refused file — and the check that follows finds it on the player.
    #[test]
    fn check_writes_nothing_and_send_copies_what_the_check_showed() {
        use flint_core::palette::{State, DIR_NAME};
        const SLATE: &str = "name = Slate\nday.bg = #0e1116\nday.panel = #141820\nday.line = #232a35\n\
            day.ink = #e6ebf2\nday.dim = #8d97a5\nday.faint = #58616e\nnight.bg = #000000\n\
            night.panel = #0a0c10\nnight.line = #151a21\nnight.ink = #8a93a0\nnight.dim = #57606c\n\
            night.faint = #373d46\n";
        let root = tmp("palettes");
        let (pc, vol) = (root.join("pc"), root.join("player"));
        fs::create_dir_all(&pc).unwrap();
        fs::create_dir_all(&vol).unwrap();
        fs::write(pc.join("Slate.palette"), SLATE).unwrap();
        fs::write(pc.join("neon.palette"), "day.ink = #0e0d0c\n").unwrap();
        let mut s = Settings::new(PathBuf::new());
        s.internal = Some(vol.clone());
        s.palette_dir = Some(pc.clone());
        let states = |rows: &[flint_core::palette::Row]| -> Vec<(String, State)> {
            rows.iter().map(|r| (r.name.clone(), r.state)).collect()
        };

        let mut rows = Vec::new();
        let word = run(Job::CheckPalettes, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::Palettes(r) = u {
                rows = r;
            }
        })
        .unwrap();
        assert_eq!(
            states(&rows),
            vec![("Cinder".into(), State::BuiltIn), ("neon".into(), State::Refused), ("Slate".into(), State::New)]
        );
        assert_eq!(word, "palettes: 1 new, 1 refused");
        assert!(!vol.join(DIR_NAME).exists(), "CHECK wrote to the player");

        let word = run(Job::SendPalettes, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::Palettes(r) = u {
                rows = r;
            }
        })
        .unwrap();
        assert_eq!(word, "sent 1 palette; 1 refused");
        let mut on_player: Vec<String> = fs::read_dir(vol.join(DIR_NAME))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into())
            .collect();
        on_player.sort();
        assert_eq!(on_player, vec!["slate.palette"], "one lowercased copy, no temporary, no refused file");
        assert_eq!(states(&rows)[2], ("Slate".into(), State::On), "the rows after SEND are read back from the player");

        s.internal = None;
        let err = run(Job::SendPalettes, &s, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(err.contains("internal memory"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    /// Download shared, against the real repository. Needs the network, so it only runs when
    /// asked: `cargo test -p flint-gui -- --ignored fetch`.
    #[test]
    #[ignore]
    fn fetch_downloads_the_shared_palettes_and_keeps_what_is_there() {
        let root = tmp("fetch");
        let dir = root.join("palettes");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("slate.palette"), "name = My Slate\n").unwrap();
        let mut s = Settings::new(PathBuf::new());
        s.palette_dir = Some(dir.clone());
        let word = run(Job::FetchPalettes, &s, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(word.contains("1 already in the folder"), "{word}");
        assert_eq!(fs::read_to_string(dir.join("slate.palette")).unwrap(), "name = My Slate\n", "kept");
        assert!(dir.join("paper.palette").is_file() && dir.join("sony.palette").is_file(), "{word}");
    }

    /// The preview lists every file a copy would delete, not the first dozen: a removal the user
    /// was never shown is a removal they did not agree to.
    #[test]
    fn the_preview_lists_every_removal() {
        let root = tmp("removals");
        let library = root.join("music");
        let volume = root.join("player");
        fs::create_dir_all(&volume).unwrap();
        for i in 0..20 {
            write_flac(&library.join(format!("Artist - Gone/{i:02} Track.flac")), 256);
        }
        write_flac(&library.join("Artist - Kept/01 One.flac"), 256);
        let s = settings(library.clone(), volume.clone(), root.join("cache"));
        let cancel = AtomicBool::new(false);
        run(Job::Plan, &s, &cancel, &mut |_| {}).unwrap();
        run(Job::Apply, &s, &cancel, &mut |_| {}).unwrap();
        fs::remove_dir_all(library.join("Artist - Gone")).unwrap();
        let mut lines = Vec::new();
        run(Job::Plan, &s, &cancel, &mut |u| {
            if let Update::Log(l) = u {
                lines.push(l)
            }
        })
        .unwrap();
        let removals = lines.iter().filter(|l| l.starts_with("would remove")).count();
        assert_eq!(removals, 20, "{lines:#?}");
        assert!(lines.last().unwrap().starts_with("in all: 0 to copy, 20 to remove"), "{lines:#?}");
    }

    #[test]
    fn save_writes_the_draft_and_never_replaces_someone_elses_file() {
        let root = tmp("palette-save");
        let dir = root.join("palettes");
        let mut s = Settings::new(PathBuf::new());
        s.palette_dir = Some(dir.clone());
        let mut d = crate::Draft::from_start(1);
        d.name = "Night Owl".into();
        s.palette_draft = Some((d.file(), d.body()));
        let mut saved = None;
        let word = run(Job::SavePalette, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::PaletteSaved(f) = u {
                saved = Some(f);
            }
        })
        .unwrap();
        assert_eq!(saved.as_deref(), Some("night-owl.palette"));
        assert!(word.starts_with("saved night-owl.palette; palettes: 1 new"), "{word}");
        let body = std::fs::read_to_string(dir.join("night-owl.palette")).unwrap();
        assert_eq!(flint_core::palette::parse("night-owl", &body).map(|p| p.name), Ok("Night Owl".into()));

        // A second draft by the same name is someone else's file as far as Save knows…
        d.hex[0] = "#101418".into();
        s.palette_draft = Some((d.file(), d.body()));
        let err = run(Job::SavePalette, &s, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(err.contains("already in the folder"), "{err}");
        // …until it is the one this draft was saved as.
        s.palette_saved = Some("night-owl.palette".into());
        run(Job::SavePalette, &s, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(std::fs::read_to_string(dir.join("night-owl.palette")).unwrap().contains("#101418"));

        // A palette the player would refuse is never written.
        d.hex[3] = "#15191e".into(); // ink on its own background
        s.palette_draft = Some((d.file(), d.body()));
        assert!(run(Job::SavePalette, &s, &AtomicBool::new(false), &mut |_| {}).is_err());
        assert!(!std::fs::read_to_string(dir.join("night-owl.palette")).unwrap().contains("#15191e"));
    }

    #[test]
    fn install_puts_a_shared_palette_in_the_folder_and_on_the_player_and_replaces_nothing() {
        use flint_core::palette::{DIR_NAME, EXAMPLES};
        let root = tmp("palette-install");
        let dir = root.join("palettes");
        let player = root.join("player");
        fs::create_dir_all(&player).unwrap();
        let mut s = Settings::new(PathBuf::new());
        s.palette_dir = Some(dir.clone());
        s.internal = Some(player.clone());
        let (id, body) = EXAMPLES[0];
        let file = format!("{id}.palette");
        s.shop_install = Some((file.clone(), body.to_string()));
        let mut said = None;
        let word = run(Job::InstallShared, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::Installed { file, on_player } = u {
                said = Some((file, on_player));
            }
        })
        .unwrap();
        assert!(word.starts_with("installed"), "{word}");
        assert_eq!(said, Some((file.clone(), true)));
        assert_eq!(fs::read_to_string(dir.join(&file)).unwrap(), body);
        assert_eq!(fs::read_to_string(player.join(DIR_NAME).join(&file)).unwrap(), body);

        // Someone's own file of that name, on the player: left alone, and said so.
        fs::write(player.join(DIR_NAME).join(&file), "mine").unwrap();
        let word = run(Job::InstallShared, &s, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(word.contains("left alone"), "{word}");
        assert_eq!(fs::read_to_string(player.join(DIR_NAME).join(&file)).unwrap(), "mine");

        // …and in the folder: refused outright.
        fs::write(dir.join(&file), "mine").unwrap();
        let err = run(Job::InstallShared, &s, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(err.contains("already in the folder"), "{err}");
        assert_eq!(fs::read_to_string(dir.join(&file)).unwrap(), "mine");

        // A player that is not plugged in: the folder only.
        let (id2, body2) = EXAMPLES[1];
        s.internal = Some(root.join("unplugged"));
        s.shop_install = Some((format!("{id2}.palette"), body2.to_string()));
        let word = run(Job::InstallShared, &s, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(word.contains("not connected"), "{word}");

        // One the player would refuse is never written.
        s.shop_install = Some(("bad.palette".into(), "name = Bad\nday.bg = #000000\nday.bg = #000000\n".into()));
        assert!(run(Job::InstallShared, &s, &AtomicBool::new(false), &mut |_| {}).is_err());
        assert!(!dir.join("bad.palette").exists());
    }

    /// Against the real repository: every shared palette comes back, checked and loading. Run by
    /// hand: `cargo test -p flint-gui -- --ignored shop`.
    #[test]
    #[ignore]
    fn shop_reads_the_shared_palettes() {
        let s = Settings::new(PathBuf::new());
        let mut items = Vec::new();
        run(Job::FetchShop, &s, &AtomicBool::new(false), &mut |u| {
            if let Update::Shop(v) = u {
                items = v;
            }
        })
        .unwrap();
        assert!(items.len() >= 3, "{items:?}");
        assert!(items.iter().all(|p| p.loads()), "{items:?}");
    }

    fn walk(dir: &Path) -> Vec<(PathBuf, u64)> {
        let mut out = Vec::new();
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push((p.clone(), fs::metadata(&p).unwrap().len()));
            }
        }
        out.sort();
        out
    }
}
