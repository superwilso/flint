//! Planning a transfer: what goes on which volume, what to copy, what to sweep away.
//!
//! This is the part of the Python `Sony-sync` this replaces (`build_library_plan`, `build_copy_jobs`,
//! `iter_stale_*`), with the same rules:
//!
//! * The library is a folder of album folders. An album is the unit that moves; it is never split.
//! * Albums that share a playlist are kept together on one volume, so no playlist spans two.
//! * An album already on a volume stays there if it still fits: a reshuffle would recopy gigabytes.
//! * Otherwise the album goes wherever leaves the most room, internal memory or card.
//! * Anything on a volume that the plan does not put there is stale, and swept.
//!
//! **The manifest** is what Flint adds. Sony-sync decides a file is up to date by comparing size and
//! mtime with the source; a SensMe-tagged copy is a few kilobytes larger than its source, so that test
//! would recopy every tagged track on every run, for ever. Each volume therefore carries
//! `flint-manifest.tsv`, recording for each copied file the source's size and mtime, the size the copy
//! ended up, and which analysis went into it. A file is up to date when the source still matches the
//! manifest, the copy on the device is still the size the manifest recorded, and the tag is the one
//! Flint would write now.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cache::{clean, write_atomic};

pub const AUDIO_EXT: [&str; 6] = ["flac", "wav", "mp3", "m4a", "aac", "alac"];
pub const PLAYLIST_EXT: [&str; 2] = ["m3u", "m3u8"];
/// What travels WITH an album rather than being an album: cover art, and lyrics.
///
/// Sony's Music Center transfers artwork, and Cinder reads `.lrc` beside the track it belongs to —
/// neither reaches the player if only audio is copied, which is what Flint did until now. A sidecar
/// only travels with an ALBUM that holds music, so a library with a pictures folder in it does not
/// turn into a photo transfer.
///
/// **A sidecar is never swept.** The sweep's rule is "anything the plan does not put there", and
/// applying that to these would delete art and lyrics another tool put on the player — a
/// destructive change to make on someone's behalf. They are copied, never removed.
pub const SIDECAR_EXT: [&str; 4] = ["jpg", "jpeg", "png", "lrc"];
/// FAT and exFAT keep timestamps to two seconds, so a copy's mtime can read older than its source's.
pub const MTIME_TOLERANCE_SECONDS: i64 = 2;
/// Playlists another tool owns (likesync writes these); never swept.
pub const MANAGED_PLAYLISTS: [&str; 2] = ["liked songs.m3u8", "liked songs.m3u"];
pub const MANIFEST_NAME: &str = "flint-manifest.tsv";
/// Headroom left free on each volume, so a full filesystem never stops the player writing its own
/// database. Sony-sync keeps the same kind of margin. A volume filled to the last byte is also one
/// that cannot be tidied up afterwards.
pub const HEADROOM_BYTES: u64 = 512 * 1024 * 1024;

fn has_ext(rel: &str, exts: &[&str]) -> bool {
    rel.rsplit_once('.').is_some_and(|(_, e)| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// One file in the PC library, path relative to the library root, always with `/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    pub rel: String,
    pub size: u64,
    pub mtime: i64,
}

impl SourceFile {
    pub fn folder(&self) -> &str {
        self.rel.split_once('/').map_or(self.rel.as_str(), |(f, _)| f)
    }

    /// Cover art or lyrics travelling with an album — see [`SIDECAR_EXT`].
    pub fn is_sidecar(&self) -> bool {
        has_ext(&self.rel, &SIDECAR_EXT)
    }
}

/// Is this device-relative path a sidecar? The same question as [`SourceFile::is_sidecar`], for a
/// path that came off the player rather than out of the library.
pub fn is_sidecar_path(rel: &str) -> bool {
    has_ext(rel, &SIDECAR_EXT)
}

/// A destination: the Walkman's internal memory, or its card.
#[derive(Clone, Debug)]
pub struct Volume {
    pub name: String,
    pub root: PathBuf,
    /// How many bytes of music this volume may hold.
    pub budget_bytes: u64,
}

/// What is on a volume now.
#[derive(Clone, Debug, Default)]
pub struct DeviceScan {
    /// Relative path -> (size, mtime).
    pub files: HashMap<String, (u64, i64)>,
    pub playlists: BTreeSet<String>,
    /// Flint's own temporary files (`.name.flint-partial`) that a pulled cable or a killed run
    /// left behind. A copy that fails removes its own; these are the ones nothing was alive to
    /// remove. They are hidden by their leading dot and hold the space they took, so the plan
    /// sweeps them.
    pub partials: Vec<String>,
}

/// The suffix of the temporary name every copy is written under before its rename.
pub const PARTIAL_SUFFIX: &str = ".flint-partial";

impl DeviceScan {
    pub fn folder_files(&self) -> BTreeMap<&str, Vec<&str>> {
        let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for rel in self.files.keys() {
            let folder = rel.split_once('/').map_or(rel.as_str(), |(f, _)| f);
            out.entry(folder).or_default().push(rel);
        }
        out
    }
}

/// What Flint wrote to a volume last time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub source_size: u64,
    pub source_mtime: i64,
    /// Size of the copy Flint wrote, tag included.
    pub copy_size: u64,
    /// Which analysis went into the copy: the cache's content key, or empty for an untagged copy.
    pub tag: String,
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub records: BTreeMap<String, Record>,
}

impl Manifest {
    pub fn path(volume_root: &Path) -> PathBuf {
        volume_root.join(MANIFEST_NAME)
    }

    /// Read a volume's manifest. A missing or unreadable one is an empty manifest: every file then
    /// looks new, which costs a recopy but never loses music.
    pub fn load(volume_root: &Path) -> Manifest {
        let mut m = Manifest::default();
        let Ok(text) = fs::read_to_string(Manifest::path(volume_root)) else { return m };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.splitn(5, '\t').collect();
            if f.len() != 5 {
                continue;
            }
            let (Ok(source_size), Ok(source_mtime), Ok(copy_size)) = (f[1].parse(), f[2].parse(), f[3].parse()) else {
                continue;
            };
            m.records.insert(f[0].to_string(), Record { source_size, source_mtime, copy_size, tag: f[4].to_string() });
        }
        m
    }

    pub fn save(&self, volume_root: &Path) -> io::Result<()> {
        let mut out = String::from("# flint manifest\tsource_size\tsource_mtime\tcopy_size\ttag\n");
        for (rel, r) in &self.records {
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                clean(rel),
                r.source_size,
                r.source_mtime,
                r.copy_size,
                clean(&r.tag)
            ));
        }
        write_atomic(&Manifest::path(volume_root), out.as_bytes())
    }

    /// Is the copy on the device still the one this source and this tag produce?
    fn up_to_date(&self, rel: &str, source: &SourceFile, tag: &str, on_device: Option<(u64, i64)>) -> bool {
        let Some(r) = self.records.get(rel) else { return false };
        let Some((size, _)) = on_device else { return false };
        r.source_size == source.size
            && source.mtime <= r.source_mtime + MTIME_TOLERANCE_SECONDS
            && r.copy_size == size
            && r.tag == tag
    }
}

/// One file to copy, and what will go into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub rel: String,
    /// Index into the volumes given to [`plan`].
    pub volume: usize,
    pub source_size: u64,
    pub source_mtime: i64,
    /// The analysis to tag the copy with, or empty to copy the file as it is.
    pub tag: String,
    /// Bytes this copy adds to the volume (the whole file, less whatever it replaces).
    pub extra_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Album folder -> volume index.
    pub assignments: BTreeMap<String, usize>,
    pub copies: Vec<Copy>,
    /// Files on a volume the plan does not want: (volume index, relative path).
    pub stale_files: Vec<(usize, String)>,
    pub stale_playlists: Vec<(usize, String)>,
    /// Playlist name -> (volume index, its tracks' relative paths).
    pub playlists: BTreeMap<String, (usize, Vec<String>)>,
    /// Albums that fit on no volume.
    pub skipped: Vec<String>,
}

impl Plan {
    pub fn bytes_to_copy(&self, volume: usize) -> u64 {
        self.copies.iter().filter(|c| c.volume == volume).map(|c| c.extra_bytes).sum()
    }
}

/// An album, or a set of albums a playlist ties together.
#[derive(Debug)]
struct Group {
    folders: BTreeSet<String>,
    playlists: BTreeSet<String>,
    size_bytes: u64,
}

/// Union-find over album folders: two folders in the same playlist end up in the same group.
fn group_folders(sizes: &BTreeMap<String, u64>, playlist_folders: &BTreeMap<String, BTreeSet<String>>) -> Vec<Group> {
    let mut parent: HashMap<&str, &str> = sizes.keys().map(|f| (f.as_str(), f.as_str())).collect();
    fn find<'a>(parent: &mut HashMap<&'a str, &'a str>, f: &'a str) -> &'a str {
        let mut root = f;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = f;
        while parent[cur] != root {
            let next = parent[cur];
            parent.insert(cur, root);
            cur = next;
        }
        root
    }
    for folders in playlist_folders.values() {
        let mut it = folders.iter();
        let Some(first) = it.next() else { continue };
        for f in it {
            let (a, b) = (find(&mut parent, first), find(&mut parent, f));
            if a != b {
                parent.insert(b, a);
            }
        }
    }
    let mut by_root: BTreeMap<&str, (BTreeSet<String>, BTreeSet<String>, u64)> = BTreeMap::new();
    for (folder, size) in sizes {
        let root = find(&mut parent, folder);
        let e = by_root.entry(root).or_default();
        e.0.insert(folder.clone());
        e.2 += size;
    }
    for (name, folders) in playlist_folders {
        let Some(first) = folders.iter().next() else { continue };
        let root = find(&mut parent, first);
        if let Some(e) = by_root.get_mut(root) {
            e.1.insert(name.clone());
        }
    }
    let mut groups: Vec<Group> = by_root
        .into_values()
        .map(|(folders, playlists, size_bytes)| Group { folders, playlists, size_bytes })
        .collect();
    // Playlist-tied groups first, then the largest, so the big decisions are made while there is room.
    groups.sort_by(|a, b| {
        b.playlists
            .len()
            .cmp(&a.playlists.len())
            .then(b.size_bytes.cmp(&a.size_bytes))
            .then_with(|| a.folders.iter().next().cmp(&b.folders.iter().next()))
    });
    groups
}

/// Bytes on a volume per top-level folder, counting only files inside a folder: what
/// [`preferred_volume`] asks of every group, built once per volume rather than by walking every
/// file on the volume for every group (which made planning quadratic — 36 s of CPU for 4,000
/// albums on one volume, run again for each volume and again on Copy).
fn bytes_by_folder(scan: &DeviceScan) -> HashMap<&str, u64> {
    let mut out: HashMap<&str, u64> = HashMap::new();
    for (rel, (size, _)) in &scan.files {
        if let Some((folder, _)) = rel.split_once('/') {
            *out.entry(folder).or_default() += size;
        }
    }
    out
}

/// The volume this group is mostly on already, by the number of its folders there, then bytes.
fn preferred_volume(group: &Group, volumes: &[Volume], on_volume: &[HashMap<&str, u64>]) -> Option<usize> {
    let mut best: Option<(usize, usize, u64)> = None;
    for (i, _) in volumes.iter().enumerate() {
        let by_folder = on_volume.get(i)?;
        let mut files = 0;
        let mut bytes = 0;
        for folder in &group.folders {
            if let Some(b) = by_folder.get(folder.as_str()) {
                files += 1;
                bytes += b;
            }
        }
        if files > 0 && best.is_none_or(|(_, f, b)| (files, bytes) > (f, b)) {
            best = Some((i, files, bytes));
        }
    }
    best.map(|(i, _, _)| i)
}

fn choose_volume(group: &Group, volumes: &[Volume], used: &[u64], preferred: Option<usize>) -> Option<usize> {
    let fits = |i: usize| used[i] + group.size_bytes <= volumes[i].budget_bytes;
    if let Some(p) = preferred.filter(|&p| fits(p)) {
        return Some(p);
    }
    // Whichever is left with the most room; ties go to the earlier volume (internal memory first).
    // `max_by_key` alone returns the LAST of equal keys, which sent ties to the card — so the
    // index is part of the key, reversed.
    (0..volumes.len())
        .filter(|&i| fits(i))
        .max_by_key(|&i| (volumes[i].budget_bytes - (used[i] + group.size_bytes), std::cmp::Reverse(i)))
}

/// Everything the transfer will do. `tag_for` gives the analysis key for a track, or an empty string
/// to copy it untouched; `playlist_tracks` maps each playlist to the library-relative paths it names.
pub fn plan(
    source: &[SourceFile],
    volumes: &[Volume],
    scans: &[DeviceScan],
    manifests: &[Manifest],
    playlist_tracks: &BTreeMap<String, Vec<String>>,
    tag_for: impl Fn(&SourceFile) -> String,
) -> Plan {
    let mut files_by_folder: BTreeMap<String, Vec<&SourceFile>> = BTreeMap::new();
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    // MUSIC decides whether a folder is an album and how big it is; a sidecar rides along with one.
    // Sized separately so a folder of nothing but pictures is not an album that fills a volume.
    let mut audio_bytes: BTreeMap<String, u64> = BTreeMap::new();
    for f in source {
        files_by_folder.entry(f.folder().to_string()).or_default().push(f);
        *sizes.entry(f.folder().to_string()).or_default() += f.size;
        if !f.is_sidecar() {
            *audio_bytes.entry(f.folder().to_string()).or_default() += f.size;
        }
    }
    // An album folder with no music in it never reaches the device; it is not an album.
    sizes.retain(|folder, _| audio_bytes.get(folder).is_some_and(|b| *b > 0));
    files_by_folder.retain(|f, _| sizes.contains_key(f));

    let mut playlist_folders: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, tracks) in playlist_tracks {
        let folders: BTreeSet<String> = tracks
            .iter()
            .filter_map(|t| t.split_once('/').map(|(f, _)| f.to_string()))
            .filter(|f| sizes.contains_key(f))
            .collect();
        if !folders.is_empty() {
            playlist_folders.insert(name.clone(), folders);
        }
    }

    let mut out = Plan::default();
    let mut used = vec![0u64; volumes.len()];
    let mut file_volume: HashMap<&str, usize> = HashMap::new();
    let on_volume: Vec<HashMap<&str, u64>> = scans.iter().map(bytes_by_folder).collect();

    for group in group_folders(&sizes, &playlist_folders) {
        let preferred = preferred_volume(&group, volumes, &on_volume);
        let Some(v) = choose_volume(&group, volumes, &used, preferred) else {
            out.skipped.extend(group.folders.iter().cloned());
            continue;
        };
        used[v] += group.size_bytes;
        for folder in &group.folders {
            out.assignments.insert(folder.clone(), v);
            for f in files_by_folder.get(folder).into_iter().flatten() {
                file_volume.insert(f.rel.as_str(), v);
                let tag = tag_for(f);
                let on_device = scans.get(v).and_then(|s| s.files.get(&f.rel)).copied();
                if manifests.get(v).is_some_and(|m| m.up_to_date(&f.rel, f, &tag, on_device)) {
                    continue;
                }
                let extra_bytes = f.size.saturating_sub(on_device.map_or(0, |(size, _)| size));
                out.copies.push(Copy {
                    rel: f.rel.clone(),
                    volume: v,
                    source_size: f.size,
                    source_mtime: f.mtime,
                    tag,
                    extra_bytes,
                });
            }
        }
        for name in &group.playlists {
            let tracks: Vec<String> = playlist_tracks
                .get(name)
                .into_iter()
                .flatten()
                .filter(|t| file_volume.get(t.as_str()) == Some(&v))
                .cloned()
                .collect();
            if !tracks.is_empty() {
                out.playlists.insert(name.clone(), (v, tracks));
            }
        }
    }

    // Anything on a volume that this plan does not put there, apart from Flint's own manifest.
    for (v, scan) in scans.iter().enumerate() {
        for rel in scan.files.keys() {
            if rel == MANIFEST_NAME {
                continue;
            }
            // Art and lyrics are copied, never removed: sweeping them would delete files another
            // tool — or the owner — put on the player, which is not a decision a sync should make.
            if is_sidecar_path(rel) {
                continue;
            }
            let folder = rel.split_once('/').map_or(rel.as_str(), |(f, _)| f);
            match out.assignments.get(folder) {
                Some(&assigned) if assigned == v && file_volume.contains_key(rel.as_str()) => {}
                // An album that fit nowhere stays where it is rather than being deleted.
                None if out.skipped.iter().any(|s| s == folder) => {}
                _ => out.stale_files.push((v, rel.clone())),
            }
        }
        // Flint's own leftovers. Only ever names Flint itself writes, so this can never sweep
        // anything another tool put on the player.
        for rel in &scan.partials {
            out.stale_files.push((v, rel.clone()));
        }
        for name in &scan.playlists {
            let managed = MANAGED_PLAYLISTS.iter().any(|m| m.eq_ignore_ascii_case(name));
            let wanted = out.playlists.get(name).is_some_and(|(pv, _)| *pv == v);
            if !managed && !wanted {
                out.stale_playlists.push((v, name.clone()));
            }
        }
    }
    out.stale_files.sort();
    out.stale_playlists.sort();
    out.copies.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// Every audio file under `root`, as library-relative paths.
pub fn scan_library(root: &Path) -> io::Result<Vec<SourceFile>> {
    scan_tree(root, None)
}

/// [`scan_library`], also collecting Flint's leftover temporary files into `partials` when asked —
/// one walk over the volume either way, because over USB the walk is the slow part.
fn scan_tree(root: &Path, mut partials: Option<&mut Vec<String>>) -> io::Result<Vec<SourceFile>> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if name.starts_with('.') {
                if let Some(partials) = partials.as_deref_mut() {
                    if name.ends_with(PARTIAL_SUFFIX) && entry.file_type()?.is_file() {
                        partials.push(rel);
                    }
                }
                continue;
            }
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push((entry.path(), rel));
            } else if ft.is_file() && (has_ext(&rel, &AUDIO_EXT) || has_ext(&rel, &SIDECAR_EXT)) {
                let meta = entry.metadata()?;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                out.push(SourceFile { rel, size: meta.len(), mtime });
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// What is on a volume: its audio files, and the playlists at its root.
pub fn scan_volume(root: &Path) -> io::Result<DeviceScan> {
    let mut scan = DeviceScan::default();
    for f in scan_tree(root, Some(&mut scan.partials))? {
        scan.files.insert(f.rel, (f.size, f.mtime));
    }
    scan.partials.sort();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && has_ext(&name, &PLAYLIST_EXT) {
            scan.playlists.insert(name);
        }
    }
    Ok(scan)
}

/// Read a folder of `.m3u`/`.m3u8` files into the map [`plan`] wants: playlist name -> the
/// library-relative paths it names.
///
/// Lines that do not resolve to a file inside `library` are dropped rather than failing the read:
/// a playlist exported from another tool routinely names tracks that are not in this library, and
/// refusing the whole file over one of them would make the feature unusable. A playlist that ends
/// up naming nothing is not returned at all.
///
/// A relative line means what M3U says, relative to the playlist's own folder; one that names
/// nothing there is tried against the library, as before. A file that is not UTF-8 is read as
/// Windows-1252, the code page an `.m3u` written on Windows is in.
pub fn read_playlists(dir: &Path, library: &Path) -> io::Result<BTreeMap<String, Vec<String>>> {
    let mut out = BTreeMap::new();
    let library = fs::canonicalize(library).unwrap_or_else(|_| library.to_path_buf());
    let here = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !has_ext(&name, &PLAYLIST_EXT) {
            continue;
        }
        let text = match fs::read(entry.path()) {
            Ok(b) => String::from_utf8(b).unwrap_or_else(|e| cp1252(e.as_bytes())),
            Err(e) => return Err(io::Error::new(e.kind(), format!("{name}: {e}"))),
        };
        let mut tracks = Vec::new();
        for line in text.lines() {
            let line = line.trim().trim_matches('"');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let candidate = Path::new(line);
            let full = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                fs::canonicalize(here.join(candidate)).unwrap_or_else(|_| library.join(candidate))
            };
            let full = fs::canonicalize(&full).unwrap_or(full);
            if let Ok(rel) = full.strip_prefix(&library) {
                tracks.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        if !tracks.is_empty() {
            out.insert(name, tracks);
        }
    }
    Ok(out)
}

/// What a sync was asked for: the command line's flags and the window's settings, one set of fields.
pub struct Request<'a> {
    pub library: &'a Path,
    pub volumes: &'a [PathBuf],
    /// A size for each volume in GB, in place of what it has free. `None` (or a short list) means
    /// measure it.
    pub budget_gb: &'a [Option<f64>],
    pub playlists: Option<&'a Path>,
    pub sensme: bool,
    /// Cover art and lyrics travel with their albums ([`SIDECAR_EXT`]).
    pub extras: bool,
}

/// A plan ready to show, or to hand to [`crate::apply::apply`].
pub struct Prepared {
    pub plan: Plan,
    pub volumes: Vec<Volume>,
    pub manifests: Vec<Manifest>,
    /// The bytes of music each volume holds now.
    pub on_device: Vec<u64>,
    /// Playlists the apply would write ([`crate::apply::playlists_to_write`]).
    pub pending_playlists: usize,
    /// Copies that will carry a SensMe tag.
    pub tagged: usize,
}

impl Prepared {
    /// What the plan copies and removes, and where, as one number. Show keeps it and Copy compares
    /// it, so a library or player that changed in between cannot remove files nobody was shown.
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in &self.plan.copies {
            (c.volume, &c.rel).hash(&mut h);
        }
        self.plan.stale_files.hash(&mut h);
        self.plan.stale_playlists.hash(&mut h);
        h.finish()
    }

    /// Nothing to copy, remove or write: the player already matches.
    pub fn is_empty(&self) -> bool {
        self.plan.copies.is_empty()
            && self.plan.stale_files.is_empty()
            && self.plan.stale_playlists.is_empty()
            && self.pending_playlists == 0
    }
}

/// What [`prepare`] reports on the way, for each front end to word as it likes.
pub enum Note<'a> {
    Library {
        files: usize,
        bytes: u64,
    },
    /// SensMe analysis found already inside `tracks` files, and the unread bytes left behind.
    Adopted {
        tracks: usize,
        saved: u64,
    },
    AdoptFailed(String),
    Volume {
        index: usize,
        root: &'a Path,
        on_device: u64,
        files: usize,
        budget: u64,
    },
    Playlists(usize),
}

/// Read the library and the volumes and make the plan. The one copy of this for the command line
/// and the window: the plan a copy carries out is the plan that was shown.
pub fn prepare(
    req: &Request,
    analysis: &mut crate::cache::Cache,
    note: &mut dyn FnMut(Note),
) -> Result<Prepared, String> {
    let lib = req.library;
    let mut source = scan_library(lib).map_err(|e| format!("{}: {e}", lib.display()))?;
    if !req.extras {
        source.retain(|f| !f.is_sidecar());
    }
    note(Note::Library { files: source.len(), bytes: source.iter().map(|f| f.size).sum() });
    let full = |f: &SourceFile| lib.join(f.rel.replace('/', std::path::MAIN_SEPARATOR_STR));

    // Analysis already inside the files (Music Center's tags): taken before the plan decides which
    // copies can carry one, for a library that never ran `flint scan`.
    if req.sensme {
        let paths: Vec<PathBuf> = source.iter().map(full).collect();
        match crate::musiccenter::adopt_all(&paths, analysis, |_| {}) {
            Ok((0, _)) => {}
            Ok((tracks, saved)) => {
                analysis.save().map_err(|e| e.to_string())?;
                note(Note::Adopted { tracks, saved });
            }
            Err(e) => note(Note::AdoptFailed(e.to_string())),
        }
    }

    let mut volumes = Vec::new();
    let mut scans = Vec::new();
    let mut manifests = Vec::new();
    let mut on_device = Vec::new();
    for (index, root) in req.volumes.iter().enumerate() {
        let scan = scan_volume(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let held: u64 = scan.files.values().map(|(size, _)| size).sum();
        // What this volume may hold: whatever is free now, plus what its music already occupies,
        // less the headroom — unless a size was given.
        let budget = match req.budget_gb.get(index).copied().flatten() {
            Some(gb) => (gb * 1024.0 * 1024.0 * 1024.0) as u64,
            None => match crate::space::free_bytes(root) {
                Some(free) => (free + held).saturating_sub(HEADROOM_BYTES),
                None => {
                    return Err(format!(
                        "could not read the free space on {} — is the player still connected? \
                         (a size can be given instead)",
                        root.display()
                    ))
                }
            },
        };
        note(Note::Volume { index, root, on_device: held, files: scan.files.len(), budget });
        volumes.push(Volume { name: root.display().to_string(), root: root.clone(), budget_bytes: budget });
        manifests.push(Manifest::load(root));
        scans.push(scan);
        on_device.push(held);
    }

    let playlists = match req.playlists {
        Some(dir) => read_playlists(dir, lib).map_err(|e| format!("{}: {e}", dir.display()))?,
        None => BTreeMap::new(),
    };
    if !playlists.is_empty() {
        note(Note::Playlists(playlists.len()));
    }

    // A track is tagged when the analysis cache holds its result; `flint scan` fills it.
    let tag_for = |f: &SourceFile| {
        if req.sensme {
            analysis.cached_key(&full(f)).unwrap_or_default()
        } else {
            String::new()
        }
    };
    let plan = plan(&source, &volumes, &scans, &manifests, &playlists, tag_for);
    let tagged = plan.copies.iter().filter(|c| !c.tag.is_empty()).count();
    let pending_playlists = crate::apply::playlists_to_write(&plan, &volumes).len();
    Ok(Prepared { plan, volumes, manifests, on_device, pending_playlists, tagged })
}

/// Windows-1252 to text: ASCII and Latin-1 as themselves, 0x80–0x9F through the table, and the
/// five bytes the code page leaves undefined as U+FFFD.
fn cp1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{fffd}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{fffd}', 'Ž', '\u{fffd}',
        '\u{fffd}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{fffd}', 'ž', 'Ÿ',
    ];
    bytes.iter().map(|&b| if (0x80..0xa0).contains(&b) { HIGH[usize::from(b - 0x80)] } else { char::from(b) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(rel: &str, size: u64) -> SourceFile {
        SourceFile { rel: rel.into(), size, mtime: 1000 }
    }

    fn volumes(internal: u64, card: u64) -> Vec<Volume> {
        vec![
            Volume { name: "internal".into(), root: PathBuf::from("/int"), budget_bytes: internal },
            Volume { name: "card".into(), root: PathBuf::from("/sd"), budget_bytes: card },
        ]
    }

    fn no_tags(_: &SourceFile) -> String {
        String::new()
    }

    #[test]
    fn a_playlist_keeps_its_albums_on_one_volume() {
        let source = vec![src("A/1.flac", 10), src("B/1.flac", 10), src("C/1.flac", 10)];
        let mut playlists = BTreeMap::new();
        playlists.insert("mix.m3u8".to_string(), vec!["A/1.flac".to_string(), "C/1.flac".to_string()]);
        let p =
            plan(&source, &volumes(20, 20), &[DeviceScan::default(), DeviceScan::default()], &[], &playlists, no_tags);
        assert_eq!(p.assignments["A"], p.assignments["C"], "a playlist was split");
        assert_ne!(p.assignments["A"], p.assignments["B"], "B should go where there is room");
        let (v, tracks) = &p.playlists["mix.m3u8"];
        assert_eq!(*v, p.assignments["A"]);
        assert_eq!(tracks, &["A/1.flac".to_string(), "C/1.flac".to_string()]);
        assert!(p.skipped.is_empty());
    }

    #[test]
    fn an_album_already_on_the_card_stays_there() {
        let source = vec![src("A/1.flac", 10)];
        let mut card = DeviceScan::default();
        card.files.insert("A/1.flac".into(), (10, 1000));
        let p = plan(&source, &volumes(100, 100), &[DeviceScan::default(), card], &[], &BTreeMap::new(), no_tags);
        assert_eq!(p.assignments["A"], 1, "it should not be moved to internal memory");
        // Without a manifest the copy is made again; nothing is deleted.
        assert_eq!(p.copies.len(), 1);
        assert!(p.stale_files.is_empty());
    }

    #[test]
    fn the_manifest_stops_a_tagged_copy_being_copied_again() {
        let source = vec![src("A/1.flac", 1000)];
        let mut device = DeviceScan::default();
        // The copy on the device is bigger than its source: it carries a SensMe tag.
        device.files.insert("A/1.flac".into(), (1000 + 6144, 1000));
        let mut manifest = Manifest::default();
        manifest.records.insert(
            "A/1.flac".into(),
            Record { source_size: 1000, source_mtime: 1000, copy_size: 1000 + 6144, tag: "flac-ab-1".into() },
        );
        let tag = |_: &SourceFile| "flac-ab-1".to_string();
        let p = plan(&source, &volumes(100_000, 0), &[device.clone()], &[manifest.clone()], &BTreeMap::new(), tag);
        assert!(p.copies.is_empty(), "{:?}", p.copies);
        assert!(p.stale_files.is_empty());
        // A new analysis, a changed source, or a changed copy each bring the copy back.
        let newer = |_: &SourceFile| "flac-ab-2".to_string();
        assert_eq!(
            plan(&source, &volumes(100_000, 0), &[device.clone()], &[manifest.clone()], &BTreeMap::new(), newer)
                .copies
                .len(),
            1
        );
        let edited = vec![SourceFile { rel: "A/1.flac".into(), size: 1001, mtime: 2000 }];
        assert_eq!(
            plan(&edited, &volumes(100_000, 0), &[device.clone()], &[manifest.clone()], &BTreeMap::new(), tag)
                .copies
                .len(),
            1
        );
        let mut shrunk = device.clone();
        shrunk.files.insert("A/1.flac".into(), (999, 1000));
        assert_eq!(plan(&source, &volumes(100_000, 0), &[shrunk], &[manifest], &BTreeMap::new(), tag).copies.len(), 1);
    }

    #[test]
    fn what_the_plan_does_not_want_is_stale_and_managed_playlists_are_not() {
        let source = vec![src("A/1.flac", 10)];
        let mut device = DeviceScan::default();
        device.files.insert("A/1.flac".into(), (10, 1000));
        device.files.insert("A/old.flac".into(), (10, 1000));
        device.files.insert("Gone/1.flac".into(), (10, 1000));
        device.files.insert(MANIFEST_NAME.into(), (10, 1000));
        device.playlists.insert("stale.m3u8".into());
        device.playlists.insert("Liked Songs.m3u8".into());
        let p = plan(&source, &volumes(100, 0), &[device], &[], &BTreeMap::new(), no_tags);
        assert_eq!(p.stale_files, vec![(0, "A/old.flac".to_string()), (0, "Gone/1.flac".to_string())]);
        assert_eq!(p.stale_playlists, vec![(0, "stale.m3u8".to_string())]);
    }

    #[test]
    fn an_album_that_fits_nowhere_is_skipped_and_left_alone() {
        let source = vec![src("Big/1.flac", 500), src("Small/1.flac", 10)];
        let mut device = DeviceScan::default();
        device.files.insert("Big/1.flac".into(), (500, 1000));
        let p = plan(&source, &volumes(100, 100), &[device, DeviceScan::default()], &[], &BTreeMap::new(), no_tags);
        assert_eq!(p.skipped, vec!["Big".to_string()]);
        assert!(p.stale_files.is_empty(), "a skipped album must not be swept: {:?}", p.stale_files);
        assert_eq!(p.copies.len(), 1);
        assert_eq!(p.bytes_to_copy(p.assignments["Small"]), 10);
    }

    #[test]
    fn a_manifest_survives_a_round_trip() {
        let d = std::env::temp_dir().join(format!("flint-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let mut m = Manifest::default();
        m.records.insert(
            "Artist/01 tab\tname.flac".into(),
            Record { source_size: 42, source_mtime: 7, copy_size: 48, tag: "flac-ab-1".into() },
        );
        m.save(&d).unwrap();
        let back = Manifest::load(&d);
        assert_eq!(back.records["Artist/01 tab name.flac"].copy_size, 48);
        assert_eq!(Manifest::load(&d.join("nowhere")).records.len(), 0);
        fs::remove_dir_all(&d).unwrap();
    }

    /// Audit E8: `../Album/01.flac` in `Library/Playlists/mix.m3u` is the album beside the folder,
    /// a library-relative line still works, and a Windows-1252 file is read rather than failing.
    #[test]
    fn playlist_lines_resolve_from_the_playlists_folder_and_windows_text_is_read() {
        let d = std::env::temp_dir().join(format!("flint-sync-m3u-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("Café/Album")).unwrap();
        fs::create_dir_all(d.join("Playlists")).unwrap();
        fs::write(d.join("Café/Album/01.flac"), b"xx").unwrap();
        fs::write(d.join("Café/Album/02.flac"), b"xx").unwrap();
        fs::write(d.join("Playlists/rel.m3u8"), "../Café/Album/01.flac\nCafé/Album/02.flac\n").unwrap();
        fs::write(d.join("Playlists/win.m3u"), b"..\x2fCaf\xe9/Album/01.flac\r\n").unwrap();
        let got = read_playlists(&d.join("Playlists"), &d).unwrap();
        assert_eq!(got["rel.m3u8"], vec!["Café/Album/01.flac", "Café/Album/02.flac"]);
        assert_eq!(got["win.m3u"], vec!["Café/Album/01.flac"]);
        assert_eq!(cp1252(b"\x80\x81\xe9"), "€\u{fffd}é");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn scanning_a_library_finds_audio_playlists_and_sidecars() {
        let d = std::env::temp_dir().join(format!("flint-sync-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("Artist/Album")).unwrap();
        for f in ["Artist/Album/01.flac", "Artist/Album/01.lrc", "Artist/cover.jpg", "mix.m3u8"] {
            fs::write(d.join(f), b"xx").unwrap();
        }
        let files = scan_library(&d).unwrap();
        assert_eq!(
            files.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(),
            vec!["Artist/Album/01.flac", "Artist/Album/01.lrc", "Artist/cover.jpg"],
            "art and lyrics come too — Music Center transfers artwork, and Cinder reads .lrc",
        );
        assert!(!files[0].is_sidecar() && files[1].is_sidecar() && files[2].is_sidecar());
        assert_eq!(files[0].folder(), "Artist");
        let scan = scan_volume(&d).unwrap();
        assert_eq!(scan.playlists.iter().map(String::as_str).collect::<Vec<_>>(), vec!["mix.m3u8"]);
        // `folder_files` groups out of a HashMap, so it has no order of its own to assert.
        let mut on_volume = scan.folder_files()["Artist"].clone();
        on_volume.sort_unstable();
        assert_eq!(on_volume, vec!["Artist/Album/01.flac", "Artist/Album/01.lrc", "Artist/cover.jpg"]);
        fs::remove_dir_all(&d).unwrap();
    }

    /// What the player keeps for itself is not the sync's to remove, even when the sync is given
    /// the top of the drive and the library no longer holds anything: ratings and play counts
    /// (`cinder_stats.tsv`), saved views (`cinder_views.conf`) and the playlists made on the
    /// player, with their covers (`cinder_playlists/`).
    #[test]
    fn the_players_own_files_are_never_planned_for_removal() {
        let d = std::env::temp_dir().join(format!("flint-sync-own-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("cinder_playlists")).unwrap();
        for f in ["cinder_stats.tsv", "cinder_views.conf", "cinder_playlists/night.m3u8", "cinder_playlists/night.jpg"]
        {
            fs::write(d.join(f), b"xx").unwrap();
        }
        let scan = scan_volume(&d).unwrap();
        assert!(scan.playlists.is_empty(), "only playlists at the top of the volume are the sync's");
        let p = plan(&[], &volumes(100, 0)[..1], &[scan], &[], &BTreeMap::new(), no_tags);
        assert_eq!((p.stale_files, p.stale_playlists), (vec![], vec![]));
        fs::remove_dir_all(&d).unwrap();
    }

    /// A temporary file a pulled cable left behind is swept; nothing else with a dot is touched.
    #[test]
    fn a_leftover_partial_is_swept_and_other_hidden_files_are_not() {
        let d = std::env::temp_dir().join(format!("flint-sync-partial-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("Album")).unwrap();
        fs::write(d.join("Album/01.flac"), b"xx").unwrap();
        fs::write(d.join("Album/.02.flac.flint-partial"), b"half").unwrap();
        fs::write(d.join("Album/._01.flac"), b"macOS metadata").unwrap();
        fs::write(d.join(".scrobbler.log"), b"#AUDIOSCROBBLER/1.1\n").unwrap();
        let scan = scan_volume(&d).unwrap();
        assert_eq!(scan.partials, vec!["Album/.02.flac.flint-partial".to_string()]);
        let source = vec![src("Album/01.flac", 2)];
        let p = plan(&source, &volumes(100, 0)[..1], &[scan], &[], &BTreeMap::new(), no_tags);
        assert_eq!(p.stale_files, vec![(0, "Album/.02.flac.flint-partial".to_string())]);
        fs::remove_dir_all(&d).unwrap();
    }

    /// Equal room on both volumes: the album goes to internal memory, as `choose_volume` has always
    /// said. `max_by_key` returns the last of equal keys, so it used to go to the card.
    #[test]
    fn a_tie_goes_to_internal_memory() {
        let source = vec![src("A/1.flac", 10)];
        let p = plan(
            &source,
            &volumes(50, 50),
            &[DeviceScan::default(), DeviceScan::default()],
            &[],
            &BTreeMap::new(),
            no_tags,
        );
        assert_eq!(p.assignments["A"], 0);
    }

    /// The folder index gives the same placements the per-group walk gave, on a library big
    /// enough that the walk took seconds.
    #[test]
    fn albums_already_on_a_volume_stay_there_at_scale() {
        let mut source = Vec::new();
        let (mut internal, mut card) = (DeviceScan::default(), DeviceScan::default());
        for a in 0..2000 {
            for t in 0..10 {
                let rel = format!("Album {a:04}/{t:02}.flac");
                source.push(src(&rel, 10));
                if a % 3 == 0 {
                    card.files.insert(rel, (10, 1000));
                } else if a % 3 == 1 {
                    internal.files.insert(rel, (10, 1000));
                }
            }
        }
        let started = std::time::Instant::now();
        let p = plan(&source, &volumes(1 << 40, 1 << 40), &[internal, card], &[], &BTreeMap::new(), no_tags);
        assert!(started.elapsed().as_secs() < 5, "planning 20,000 files took {:?}", started.elapsed());
        for a in 0..2000 {
            let want = match a % 3 {
                0 => Some(1),
                1 => Some(0),
                _ => None,
            };
            if let Some(v) = want {
                assert_eq!(p.assignments[&format!("Album {a:04}")], v, "Album {a:04} moved");
            }
        }
        assert!(p.stale_files.is_empty());
    }

    /// Art and lyrics go where their album goes, a folder with no music in it is not an album, and
    /// a sidecar already on the player is left alone rather than swept.
    #[test]
    fn art_and_lyrics_travel_with_their_album_and_are_never_swept() {
        let f = |rel: &str, size: u64| SourceFile { rel: rel.into(), size, mtime: 1 };
        let source = vec![
            f("Album/01.flac", 1_000_000),
            f("Album/cover.jpg", 40_000),
            f("Album/01.lrc", 2_000),
            // Nothing but pictures: not an album, and must not be planned onto a volume.
            f("Snapshots/holiday.jpg", 5_000_000),
        ];
        let volumes = vec![Volume { name: "internal".into(), root: "/int".into(), budget_bytes: 8_000_000 }];
        // The player already holds a cover another tool put there.
        let mut scan = DeviceScan::default();
        scan.files.insert("Album/other-art.png".into(), (1234, 1));
        scan.files.insert("Stale/gone.flac".into(), (999, 1));
        let plan = plan(&source, &volumes, &[scan], &[Manifest::default()], &BTreeMap::new(), |_| String::new());

        let copied: Vec<&str> = plan.copies.iter().map(|c| c.rel.as_str()).collect();
        assert_eq!(copied, vec!["Album/01.flac", "Album/01.lrc", "Album/cover.jpg"]);
        assert_eq!(plan.assignments.get("Album"), Some(&0));
        assert!(!plan.assignments.contains_key("Snapshots"), "a folder of pictures is not an album");
        // The stale sweep still removes the orphan track, and still leaves the art alone.
        let stale: Vec<&str> = plan.stale_files.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(stale, vec!["Stale/gone.flac"]);
    }
}
