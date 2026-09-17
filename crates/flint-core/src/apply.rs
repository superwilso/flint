//! Carrying out a plan: copy, tag, write playlists, sweep, record.
//!
//! Every write here is a temporary file and a rename, so a cable pulled mid-copy leaves either the
//! old file or the new one, never half of either. The temporary lives beside its destination, on the
//! same volume, because a rename across volumes is a copy.
//!
//! Order matters: the sweep runs first, so space freed by removals is available to the copies that
//! follow. The manifest is saved as work finishes, so an interrupted run does not forget what it
//! copied — [`crate::sync::Manifest`] explains why the manifest exists at all.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::sync::{Copy, Manifest, Plan, Record, Volume};
use crate::{flac, id3};

/// Save the manifests after this many copies, so little is lost if the run is cut short.
const SAVE_EVERY: usize = 50;

#[derive(Debug)]
pub enum Event {
    Removed { volume: usize, rel: String },
    Copied { done: usize, total: usize, rel: String, volume: usize, tagged: bool },
    Playlist { name: String, volume: usize, tracks: usize },
    Failed { what: String, error: String },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub copied: usize,
    pub tagged: usize,
    pub bytes: u64,
    pub removed: usize,
    pub playlists: usize,
    pub failed: usize,
}

fn device_path(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

fn temp_beside(dst: &Path) -> PathBuf {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    dst.with_file_name(format!(".{name}.flint-partial"))
}

/// Copy `src` to `dst` through a temporary file beside it.
fn copy_plain(src: &Path, dst: &Path) -> io::Result<u64> {
    let tmp = temp_beside(dst);
    let bytes = fs::copy(src, &tmp)?;
    fs::rename(&tmp, dst)?;
    Ok(bytes)
}

/// Copy one file to its volume, tagging it on the way when there is a tag for it.
fn copy_one(src: &Path, dst: &Path, payload: Option<&[u8]>) -> io::Result<(u64, bool)> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let ext = dst.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match (payload, ext.as_str()) {
        (Some(payload), "flac") => {
            let r = flac::copy_with_smfm(src, dst, payload).map_err(io::Error::other)?;
            Ok((r.copy_len, true))
        }
        (Some(payload), "mp3") => {
            let r = id3::copy_with_smfmf(src, dst, payload).map_err(io::Error::other)?;
            Ok((r.copy_len, true))
        }
        _ => Ok((copy_plain(src, dst)?, false)),
    }
}

/// Remove the directories a swept file leaves behind, up to (but not including) the volume root.
fn prune_empty(root: &Path, from: &Path) {
    let mut dir = from.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            return;
        }
        if fs::read_dir(&d).map(|mut e| e.next().is_some()).unwrap_or(true) {
            return;
        }
        if fs::remove_dir(&d).is_err() {
            return;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
}

/// Carry out `plan`. `payload_for` gives the SensMe result for a copy's tag, or `None` to copy the
/// file untouched. With `dry_run`, nothing is written and the events describe what would happen.
pub fn apply(
    plan: &Plan,
    source_root: &Path,
    volumes: &[Volume],
    manifests: &mut [Manifest],
    payload_for: impl Fn(&Copy) -> Option<Vec<u8>>,
    dry_run: bool,
    mut on_event: impl FnMut(&Event),
) -> io::Result<Outcome> {
    let mut out = Outcome::default();

    // The sweep first: removals free space for the copies.
    for (v, rel) in &plan.stale_files {
        let path = device_path(&volumes[*v].root, rel);
        if !dry_run {
            if let Err(e) = fs::remove_file(&path) {
                out.failed += 1;
                on_event(&Event::Failed { what: path.display().to_string(), error: e.to_string() });
                continue;
            }
            prune_empty(&volumes[*v].root, &path);
        }
        manifests.get_mut(*v).map(|m| m.records.remove(rel));
        out.removed += 1;
        on_event(&Event::Removed { volume: *v, rel: rel.clone() });
    }
    for (v, name) in &plan.stale_playlists {
        let path = volumes[*v].root.join(name);
        if !dry_run {
            if let Err(e) = fs::remove_file(&path) {
                out.failed += 1;
                on_event(&Event::Failed { what: path.display().to_string(), error: e.to_string() });
                continue;
            }
        }
        out.removed += 1;
        on_event(&Event::Removed { volume: *v, rel: name.clone() });
    }

    let total = plan.copies.len();
    for (i, c) in plan.copies.iter().enumerate() {
        let src = device_path(source_root, &c.rel);
        let dst = device_path(&volumes[c.volume].root, &c.rel);
        let payload = if c.tag.is_empty() { None } else { payload_for(c) };
        if dry_run {
            out.copied += 1;
            out.bytes += c.source_size;
            out.tagged += usize::from(payload.is_some());
            on_event(&Event::Copied {
                done: i + 1,
                total,
                rel: c.rel.clone(),
                volume: c.volume,
                tagged: payload.is_some(),
            });
            continue;
        }
        match copy_one(&src, &dst, payload.as_deref()) {
            Ok((copy_size, tagged)) => {
                out.copied += 1;
                out.bytes += copy_size;
                out.tagged += usize::from(tagged);
                if let Some(m) = manifests.get_mut(c.volume) {
                    m.records.insert(
                        c.rel.clone(),
                        Record {
                            source_size: c.source_size,
                            source_mtime: c.source_mtime,
                            copy_size,
                            tag: if tagged { c.tag.clone() } else { String::new() },
                        },
                    );
                }
                on_event(&Event::Copied { done: i + 1, total, rel: c.rel.clone(), volume: c.volume, tagged });
            }
            Err(e) => {
                out.failed += 1;
                on_event(&Event::Failed { what: c.rel.clone(), error: e.to_string() });
            }
        }
        if out.copied % SAVE_EVERY == 0 {
            save_manifests(volumes, manifests)?;
        }
    }

    for (name, (v, tracks)) in &plan.playlists {
        let path = volumes[*v].root.join(name);
        let body = playlist_body(tracks);
        if fs::read_to_string(&path).is_ok_and(|on_device| on_device == body) {
            continue;
        }
        if !dry_run {
            let tmp = temp_beside(&path);
            if let Err(e) = fs::write(&tmp, body.as_bytes()).and_then(|()| fs::rename(&tmp, &path)) {
                out.failed += 1;
                on_event(&Event::Failed { what: path.display().to_string(), error: e.to_string() });
                continue;
            }
        }
        out.playlists += 1;
        on_event(&Event::Playlist { name: name.clone(), volume: *v, tracks: tracks.len() });
    }

    if !dry_run {
        save_manifests(volumes, manifests)?;
    }
    Ok(out)
}

/// What a playlist file holds: one library-relative path per line, `/` separated, as Sony-sync wrote
/// them and as the player reads them.
fn playlist_body(tracks: &[String]) -> String {
    tracks.iter().map(|t| format!("{t}\n")).collect()
}

/// The playlists whose file on the device does not already say what the plan says.
pub fn playlists_to_write(plan: &Plan, volumes: &[Volume]) -> Vec<String> {
    plan.playlists
        .iter()
        .filter(|(name, (v, tracks))| {
            let path = volumes[*v].root.join(name);
            !fs::read_to_string(&path).is_ok_and(|on_device| on_device == playlist_body(tracks))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn save_manifests(volumes: &[Volume], manifests: &[Manifest]) -> io::Result<()> {
    for (v, m) in manifests.iter().enumerate() {
        if let Some(volume) = volumes.get(v) {
            m.save(&volume.root)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{self, DeviceScan, SourceFile};
    use std::collections::BTreeMap;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flint-apply-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A minimal FLAC: signature, a last-block STREAMINFO, then "audio".
    fn flac_bytes(marker: u8) -> Vec<u8> {
        let mut info = vec![0u8; 34];
        info[18] = marker;
        let mut out = flac::encode_metadata(&[], &[flac::Block { kind: 0, data: info }]).unwrap();
        out.extend_from_slice(b"\xff\xf8audio-frames");
        out
    }

    #[test]
    fn a_copy_is_tagged_recorded_and_not_made_twice() {
        let d = tmpdir("copy");
        let (lib, dev) = (d.join("lib"), d.join("dev"));
        fs::create_dir_all(lib.join("Artist")).unwrap();
        fs::create_dir_all(&dev).unwrap();
        fs::write(lib.join("Artist/01.flac"), flac_bytes(1)).unwrap();

        let source = sync::scan_library(&lib).unwrap();
        let volumes = vec![Volume { name: "internal".into(), root: dev.clone(), budget_bytes: 1 << 20 }];
        let mut manifests = vec![Manifest::default()];
        let payload = b"GBPMSTAEMMLW\x01\x00\x80\x00\x00\x00\x00\x04\x42\xc8\x00\x00".to_vec();
        let plan = sync::plan(&source, &volumes, &[DeviceScan::default()], &manifests, &BTreeMap::new(), |_| {
            "flac-key-1".to_string()
        });
        assert_eq!(plan.copies.len(), 1);
        let out = apply(&plan, &lib, &volumes, &mut manifests, |_| Some(payload.clone()), false, |_| {}).unwrap();
        assert_eq!((out.copied, out.tagged, out.failed), (1, 1, 0));

        let copied = dev.join("Artist/01.flac");
        let mut f = std::io::BufReader::new(fs::File::open(&copied).unwrap());
        assert_eq!(flac::read_layout(&mut f).unwrap().smfm(), Some(payload.as_slice()), "the copy has no SensMe block");
        assert!(fs::metadata(&copied).unwrap().len() > fs::metadata(lib.join("Artist/01.flac")).unwrap().len());
        assert!(sync::Manifest::path(&dev).exists());
        assert!(!dev.join("Artist/.01.flac.flint-partial").exists(), "a temporary file was left behind");

        // Planning again against the device and the saved manifest: nothing left to do.
        let scans = vec![sync::scan_volume(&dev).unwrap()];
        let manifests = vec![Manifest::load(&dev)];
        let again = sync::plan(&source, &volumes, &scans, &manifests, &BTreeMap::new(), |_| "flac-key-1".to_string());
        assert!(again.copies.is_empty(), "{:?}", again.copies);
        assert!(again.stale_files.is_empty(), "{:?}", again.stale_files);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_sweep_removes_the_file_its_empty_folders_and_its_manifest_row() {
        let d = tmpdir("sweep");
        let dev = d.join("dev");
        fs::create_dir_all(dev.join("Gone/Disc 1")).unwrap();
        fs::write(dev.join("Gone/Disc 1/01.flac"), b"x").unwrap();
        fs::write(dev.join("old.m3u8"), b"Gone/Disc 1/01.flac\n").unwrap();
        let volumes = vec![Volume { name: "internal".into(), root: dev.clone(), budget_bytes: 1 << 20 }];
        let mut manifests = vec![Manifest::default()];
        manifests[0].records.insert(
            "Gone/Disc 1/01.flac".into(),
            Record { source_size: 1, source_mtime: 1, copy_size: 1, tag: String::new() },
        );
        let plan = sync::plan(
            &[],
            &volumes,
            &[sync::scan_volume(&dev).unwrap()],
            &manifests,
            &BTreeMap::new(),
            |_: &SourceFile| String::new(),
        );
        let out = apply(&plan, &d, &volumes, &mut manifests, |_| None, false, |_| {}).unwrap();
        assert_eq!((out.removed, out.failed), (2, 0));
        assert!(!dev.join("Gone").exists(), "empty folders were left behind");
        assert!(!dev.join("old.m3u8").exists());
        assert!(manifests[0].records.is_empty());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_playlist_is_written_with_forward_slashes_and_a_dry_run_writes_nothing() {
        let d = tmpdir("playlist");
        let (lib, dev) = (d.join("lib"), d.join("dev"));
        fs::create_dir_all(lib.join("Artist")).unwrap();
        fs::create_dir_all(&dev).unwrap();
        fs::write(lib.join("Artist/01.flac"), flac_bytes(2)).unwrap();
        let source = sync::scan_library(&lib).unwrap();
        let volumes = vec![Volume { name: "internal".into(), root: dev.clone(), budget_bytes: 1 << 20 }];
        let mut playlists = BTreeMap::new();
        playlists.insert("mix.m3u8".to_string(), vec!["Artist/01.flac".to_string()]);
        let mut manifests = vec![Manifest::default()];
        let plan = sync::plan(&source, &volumes, &[DeviceScan::default()], &manifests, &playlists, |_| String::new());

        assert_eq!(playlists_to_write(&plan, &volumes), vec!["mix.m3u8".to_string()]);
        let dry = apply(&plan, &lib, &volumes, &mut manifests, |_| None, true, |_| {}).unwrap();
        assert_eq!((dry.copied, dry.playlists), (1, 1));
        assert!(!dev.join("Artist/01.flac").exists(), "a dry run wrote a file");
        assert!(!dev.join("mix.m3u8").exists(), "a dry run wrote a playlist");
        assert!(manifests[0].records.is_empty());

        apply(&plan, &lib, &volumes, &mut manifests, |_| None, false, |_| {}).unwrap();
        assert_eq!(fs::read_to_string(dev.join("mix.m3u8")).unwrap(), "Artist/01.flac\n");
        // A second run leaves an unchanged playlist alone.
        assert!(playlists_to_write(&plan, &volumes).is_empty());
        let out = apply(&plan, &lib, &volumes, &mut manifests, |_| None, false, |_| {}).unwrap();
        assert_eq!(out.playlists, 0);
        fs::remove_dir_all(&d).unwrap();
    }
}
