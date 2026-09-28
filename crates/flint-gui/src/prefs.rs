//! What the window remembers between runs: the folders, the two switches and the theme.
//!
//! Until 0.2 the window forgot everything when it closed, so every session began by choosing the
//! music folder and the player's drive again — the most repetitive thing the window asked of anyone,
//! and the first thing it asked. `gui.conf` sits beside the analysis cache, so everything Flint
//! keeps is in one folder, and it is plain `key=value` lines anyone can read or delete.
//!
//! A drive letter is remembered like any other path. If the player comes back as a different letter
//! the card simply names the old one, and choosing it again is the same one click it always was.

use crate::{Model, ThemePref};
use std::path::{Path, PathBuf};

/// The file's name inside `flint_core::cache::default_dir()`.
pub const FILE: &str = "gui.conf";

pub fn path() -> PathBuf {
    flint_core::cache::default_dir().join(FILE)
}

/// The lines to write for `m`. Paths with a newline in them cannot be written and are left out.
pub fn render(m: &Model) -> String {
    let mut out = String::from("# Flint's window: what it remembers between runs\n");
    let mut put = |k: &str, p: &Option<PathBuf>| {
        if let Some(p) = p {
            let s = p.display().to_string();
            if !s.contains(['\n', '\r']) {
                out.push_str(&format!("{k}={s}\n"));
            }
        }
    };
    put("library", &m.library);
    put("volume0", &m.volumes[0]);
    put("volume1", &m.volumes[1]);
    put("playlists", &m.playlists);
    put("palette_dir", &m.palette_dir);
    out.push_str(&format!("sensme={}\n", u8::from(m.sensme)));
    out.push_str(&format!("extras={}\n", u8::from(m.extras)));
    out.push_str(&format!("theme={}\n", m.theme.word()));
    out
}

/// Apply a `gui.conf` body to `m`. Unknown keys are ignored and a bad line is skipped: a
/// hand-edited file must never keep the window from opening.
pub fn apply(m: &mut Model, body: &str) {
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        let path = || (!v.is_empty()).then(|| PathBuf::from(v));
        match k.trim() {
            "library" => m.library = path(),
            "volume0" => m.volumes[0] = path(),
            "volume1" => m.volumes[1] = path(),
            "playlists" => m.playlists = path(),
            "palette_dir" => m.palette_dir = path(),
            "sensme" => m.sensme = v != "0",
            "extras" => m.extras = v != "0",
            "theme" => m.theme = ThemePref::from_word(v),
            _ => {}
        }
    }
}

/// Read `file` into `m`, if it is there.
pub fn load(m: &mut Model, file: &Path) {
    if let Ok(body) = std::fs::read_to_string(file) {
        apply(m, &body);
    }
}

/// Write `m` to `file`, through a temporary file and a rename so a crash cannot leave half a file.
pub fn save(m: &Model, file: &Path) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("conf.tmp");
    std::fs::write(&tmp, render(m))?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_round_trips_through_gui_conf() {
        let mut m = Model::new();
        m.library = Some("D:\\Music".into());
        m.volumes[1] = Some("F:\\".into());
        m.sensme = false;
        m.theme = ThemePref::Dark;
        let mut back = Model::new();
        apply(&mut back, &render(&m));
        assert_eq!(back.library, m.library);
        assert_eq!(back.volumes, m.volumes);
        assert_eq!(back.playlists, None);
        assert!(!back.sensme && back.extras);
        assert_eq!(back.theme, ThemePref::Dark);
    }

    #[test]
    fn a_bad_file_opens_the_window_anyway() {
        let mut m = Model::new();
        apply(&mut m, "garbage\n=\ntheme=purple\nlibrary=\nvolume0=E:\\\n");
        assert_eq!(m.theme, ThemePref::System, "an unknown theme is System");
        assert_eq!(m.library, None, "an empty path is no path");
        assert_eq!(m.volumes[0], Some("E:\\".into()));
    }

    #[test]
    fn save_and_load_use_the_disk() {
        let dir = std::env::temp_dir().join(format!("flint-prefs-{}", std::process::id()));
        let file = dir.join(FILE);
        let mut m = Model::new();
        m.playlists = Some("/lists".into());
        save(&m, &file).unwrap();
        let mut back = Model::new();
        load(&mut back, &file);
        assert_eq!(back.playlists, m.playlists);
        let _ = std::fs::remove_dir_all(dir);
    }
}
