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
}

impl DeviceScan {
    pub fn folder_files(&self) -> BTreeMap<&str, Vec<&str>> {
        let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for rel in self.files.keys() {
            let folder = rel.split_once('/').map_or(rel.as_str(), |(f, _)| f);
            out.entry(folder).or_default().push(rel);
        }
        out
    }

    fn bytes_in(&self, folder: &str) -> u64 {
        self.files
            .iter()
            .filter(|(rel, _)| rel.split_once('/').map_or(rel.as_str(), |(f, _)| f) == folder)
            .map(|(_, (size, _))| size)
            .sum()
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

/// The volume this group is mostly on already, by file count then bytes.
fn preferred_volume(group: &Group, volumes: &[Volume], scans: &[DeviceScan]) -> Option<usize> {
    let mut best: Option<(usize, usize, u64)> = None;
    for (i, _) in volumes.iter().enumerate() {
        let scan = scans.get(i)?;
        let mut files = 0;
        let mut bytes = 0;
        for folder in &group.folders {
            let n = scan.files.keys().filter(|rel| rel.starts_with(&format!("{folder}/"))).count();
            if n > 0 {
                files += 1;
                bytes += scan.bytes_in(folder);
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
    (0..volumes.len()).filter(|&i| fits(i)).max_by_key(|&i| volumes[i].budget_bytes - (used[i] + group.size_bytes))
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

    for group in group_folders(&sizes, &playlist_folders) {
        let preferred = preferred_volume(&group, volumes, scans);
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
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
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
    for f in scan_library(root)? {
        scan.files.insert(f.rel, (f.size, f.mtime));
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && has_ext(&name, &PLAYLIST_EXT) {
            scan.playlists.insert(name);
        }
    }
    Ok(scan)
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
