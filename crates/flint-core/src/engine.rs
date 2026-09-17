//! Running Sony's SensMe engine over one track: FFmpeg decodes, `sensme-helper` analyses.
//!
//! ```text
//! ffmpeg -i <track> -f s16le -ac 2 -ar 44100 -  ──pipe──►  sensme-helper <MMLib11.dll>  ──► SMFMF bytes
//! ```
//!
//! The engine (`MMLib11.dll`) is Sony's, ships with Music Center for PC, and is not redistributed:
//! Flint finds the installed copy. It is a 32-bit COM DLL, which is why the helper is a separate
//! 32-bit program rather than code in Flint itself. The helper's contract is in
//! `crates/sensme-helper/src/main.rs`.

use std::env;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

/// Where Music Center for PC 2.x installs the engine.
pub const MUSIC_CENTER_ENGINE: &str = r"C:\Program Files (x86)\Sony\Music Center\AVLib\MMLib11.dll";

#[derive(Clone, Debug)]
pub struct Engine {
    pub ffmpeg: PathBuf,
    pub helper: PathBuf,
    pub dll: PathBuf,
    /// Engine parameter overrides, `id=value`, passed through to the helper.
    pub params: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub smfmf: Vec<u8>,
    /// `key=value` lines the helper reported on stderr (engine version, timings, result 88, …).
    pub info: Vec<(String, String)>,
}

fn exe(name: &str) -> OsString {
    if cfg!(windows) {
        format!("{name}.exe").into()
    } else {
        name.into()
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    let file = exe(name);
    env::split_paths(&env::var_os("PATH")?).map(|d| d.join(&file)).find(|p| p.is_file())
}

fn from_env(var: &str) -> Option<PathBuf> {
    env::var_os(var).map(PathBuf::from).filter(|p| p.is_file())
}

/// FFmpeg alone, for work that does not need Sony's engine (the lossless check).
pub fn locate_ffmpeg() -> Result<PathBuf, String> {
    from_env("FLINT_FFMPEG").or_else(|| on_path("ffmpeg")).ok_or_else(|| {
        "FFmpeg was not found. Install it (for example `winget install Gyan.FFmpeg`) or set FLINT_FFMPEG.".to_string()
    })
}

impl Engine {
    /// Find FFmpeg, the helper and the engine. `FLINT_FFMPEG`, `FLINT_HELPER` and `FLINT_MMLIB`
    /// override each; otherwise FFmpeg comes from PATH, the helper from beside Flint's own exe, and
    /// the engine from Music Center's install folder. The error says what is missing and what to do.
    pub fn locate() -> Result<Engine, String> {
        let ffmpeg = locate_ffmpeg()?;
        let beside = env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(exe("sensme-helper"))));
        let helper = from_env("FLINT_HELPER")
            .or(beside.filter(|p| p.is_file()))
            .ok_or("sensme-helper.exe was not found next to flint.exe (or set FLINT_HELPER).")?;
        let dll = from_env("FLINT_MMLIB")
            .or_else(|| Some(PathBuf::from(MUSIC_CENTER_ENGINE)).filter(|p| p.is_file()))
            .ok_or(
                "Sony's analysis engine was not found. SensMe needs Music Center for PC installed \
                 (it provides AVLib\\MMLib11.dll), or set FLINT_MMLIB to that file.",
            )?;
        Ok(Engine { ffmpeg, helper, dll, params: Vec::new() })
    }

    pub fn analyse(&self, track: &Path) -> Result<Analysis, String> {
        let mut decode = Command::new(&self.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
            .arg(track)
            .args(["-vn", "-f", "s16le", "-acodec", "pcm_s16le", "-ac", "2", "-ar", "44100", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start FFmpeg: {e}"))?;
        let pcm = decode.stdout.take().expect("piped");
        let mut helper = Command::new(&self.helper)
            .arg(&self.dll)
            .args(&self.params)
            .stdin(Stdio::from(pcm))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start sensme-helper: {e}"))?;

        let drain = |mut r: Box<dyn Read + Send>| {
            thread::spawn(move || {
                let mut s = Vec::new();
                let _ = r.read_to_end(&mut s);
                String::from_utf8_lossy(&s).into_owned()
            })
        };
        let ffmpeg_err = drain(Box::new(decode.stderr.take().expect("piped")));
        let helper_err = drain(Box::new(helper.stderr.take().expect("piped")));
        let mut smfmf = Vec::new();
        helper
            .stdout
            .take()
            .expect("piped")
            .read_to_end(&mut smfmf)
            .map_err(|e| format!("reading the helper's result: {e}"))?;

        let helper_status = helper.wait().map_err(|e| e.to_string())?;
        let decode_status = decode.wait().map_err(|e| e.to_string())?;
        let ffmpeg_err = ffmpeg_err.join().unwrap_or_default();
        let helper_err = helper_err.join().unwrap_or_default();

        if !decode_status.success() {
            return Err(format!("FFmpeg could not decode {}: {}", track.display(), ffmpeg_err.trim()));
        }
        if !helper_status.success() {
            return Err(format!("the SensMe engine failed on {}: {}", track.display(), helper_err.trim()));
        }
        let info = helper_err
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        crate::smfmf::parse(&smfmf).map_err(|e| format!("the engine's result is malformed: {e}"))?;
        Ok(Analysis { smfmf, info })
    }
}
