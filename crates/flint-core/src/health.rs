//! The player's health, read from its drive (audit G1.2).
//!
//! Everything here is a file Cinder or its installer already leaves at the top of the drive, so
//! reading it needs nothing from the player but the cable:
//!
//! * `cinder_components.conf` — `# installer:` and `# channel:`, what is installed;
//! * `cinder_home_install.log` — how the last install or uninstall ended;
//! * `cinderhome.log` — the last boot: did Cinder start, or why the launcher handed over to Sony's
//!   player, which sound library it found, the library by storage, FLACs it cannot decode, and
//!   `/data`'s free space when USB storage began.
//!
//! [`report`] is what *Copy a report* puts on the clipboard: the card, then the tails of both logs.
//! It replaces several rounds of "send me the log".
//!
//! The log lines are Cinder's `clog_`/`eprintln!` format strings (main.cpp, cinder-ffi). The two in
//! the tests marked "real" were copied from a device log; the others are the format strings filled
//! in, until a real boot's lines replace them.

use std::fs;
use std::path::Path;

pub const HOME_LOG: &str = "cinderhome.log";
pub const INSTALL_LOG: &str = "cinder_home_install.log";
pub const CONF: &str = "cinder_components.conf";
/// Lines of each log in a report.
const TAIL: usize = 80;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LastBoot {
    /// No log, or no boot in it.
    #[default]
    Unknown,
    Started,
    /// The launcher's reason for handing over to Sony's player.
    ToStock(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Health {
    pub installer: Option<String>,
    pub channel: Option<String>,
    pub last_boot: LastBoot,
    /// From the last boot that checked: Walkman One's sound library (true) or stock's.
    pub walkman_one: Option<bool>,
    /// Tracks on internal memory and on the card, as the last boot found them.
    pub tracks: Option<(u64, u64)>,
    /// FLACs wider than 24 bits, which the player cannot decode.
    pub undecodable: Option<u64>,
    pub data_free_kb: Option<u64>,
    /// The last line of the install log.
    pub install: Option<String>,
}

/// The number after `key` in `line`, if one follows.
fn number_after(line: &str, key: &str) -> Option<u64> {
    let rest = &line[line.find(key)? + key.len()..];
    let digits: String = rest.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Read the three files from `drive`. Missing files leave their fields empty.
pub fn read(drive: &Path) -> Health {
    let text = |name: &str| fs::read(drive.join(name)).map(|b| String::from_utf8_lossy(&b).into_owned()).ok();
    let mut h = Health::default();
    if let Some(conf) = text(CONF) {
        let get = |key: &str| {
            conf.lines()
                .find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().to_string()))
                .filter(|v| !v.is_empty())
        };
        h.installer = get("# installer:");
        h.channel = get("# channel:");
    }
    h.install =
        text(INSTALL_LOG).and_then(|t| t.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(String::from));
    if let Some(log) = text(HOME_LOG) {
        read_home_log(&log, &mut h);
    }
    h
}

/// Each fact is the last one logged: a boot that never got as far as logging one leaves the
/// previous boot's.
fn read_home_log(log: &str, h: &mut Health) {
    // The Clear Bass line is logged on every firmware, after the Walkman One line when there is one.
    let mut w1_this_boot = false;
    for line in log.lines() {
        if line.contains("main: start") {
            h.last_boot = LastBoot::Started;
            w1_this_boot = false;
        } else if let Some(r) =
            line.split_once("cinderhome-launch: ").and_then(|(_, r)| r.trim().strip_suffix(" -> stock"))
        {
            h.last_boot = LastBoot::ToStock(r.trim().to_string());
        } else if line.contains("sound: Walkman One sound library") {
            h.walkman_one = Some(true);
            w1_this_boot = true;
        } else if line.contains("sound: Clear Bass") && !w1_this_boot {
            h.walkman_one = Some(false);
        } else if line.contains("library by storage") {
            if let (Some(i), Some(s)) = (number_after(line, "internal"), number_after(line, "SD")) {
                h.tracks = Some((i, s));
            }
        } else if line.contains("wider than 24 bits will be skipped") {
            h.undecodable = number_after(line, "cinder-ffi:");
        } else if line.contains("status: /data free") {
            h.data_free_kb = number_after(line, "status: /data free");
        }
    }
}

/// The card, a line per fact, in words.
pub fn lines(h: &Health) -> Vec<String> {
    let mut out = Vec::new();
    out.push(match (&h.installer, &h.channel) {
        (Some(v), Some(c)) => format!("Cinder from installer {v} ({c})"),
        (Some(v), None) => format!("Cinder from installer {v}"),
        (None, Some(c)) => format!("Cinder ({c})"),
        (None, None) => "No Cinder install found on this drive".into(),
    });
    out.push(match &h.last_boot {
        LastBoot::Unknown => "Last start: not in the log".into(),
        LastBoot::Started => "Last start: Cinder started".into(),
        LastBoot::ToStock(r) => format!("Last start: handed over to Sony's player: {r}"),
    });
    if let Some(w1) = h.walkman_one {
        out.push(if w1 { "Firmware: Walkman One" } else { "Firmware: Sony's" }.into());
    }
    if let Some((i, s)) = h.tracks {
        out.push(format!("Library: {i} tracks on the player, {s} on the card"));
    }
    if let Some(n) = h.undecodable.filter(|n| *n > 0) {
        out.push(format!("{n} FLAC(s) wider than 24 bits: the player cannot play them"));
    }
    if let Some(kb) = h.data_free_kb {
        let warn = if kb < 16 * 1024 { ", low" } else { "" };
        out.push(format!("System space free: {} MB{warn}", kb / 1024));
    }
    if let Some(l) = &h.install {
        out.push(format!("Last install log line: {l}"));
    }
    out
}

/// The card and the tails of both logs, for a bug report.
pub fn report(drive: &Path, h: &Health) -> String {
    let mut out = String::from("Player health (Flint)\n");
    for l in lines(h) {
        out.push_str(&format!("- {l}\n"));
    }
    for name in [HOME_LOG, INSTALL_LOG] {
        let Ok(bytes) = fs::read(drive.join(name)) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let all: Vec<&str> = text.lines().collect();
        out.push_str(&format!("\n--- {name}, last {} of {} lines ---\n", TAIL.min(all.len()), all.len()));
        for l in &all[all.len().saturating_sub(TAIL)..] {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real, from an NW-A55 log (2026-09-23; Cinder's installer tests carry the same two).
    const START: &str = "[cinder-home]    0.000 main: start";
    const CABLE: &str = "cinderhome-launch: a USB cable was connected at boot (rung 0; /data/cinder/cable_escape_off opts out) -> stock";
    /// Cinder's format strings, filled in.
    const W1: &str =
        "[cinder-home]    2.310 sound: Walkman One sound library (no VPT, Clear Phase or DSEE AI in its chain)";
    const BASS: &str = "[cinder-home]    2.311 sound: Clear Bass not available on this sound library";
    const BASS_STOCK: &str = "[cinder-home]    2.311 sound: Clear Bass available (six-band EQ is in the chain)";
    const LIB: &str = "cinder-ffi: library by storage — internal 3463, SD 1130, unresolved 0";
    const WIDE: &str = "cinder-ffi: 2 FLAC(s) wider than 24 bits will be skipped (the player's decoder stops at 24): [\"/contents/MUSIC/a.flac\"]";
    const FREE: &str = "[cinder-home]  612.004 status: /data free 4096 KB";

    #[test]
    fn the_last_boot_and_its_facts_come_from_the_log() {
        let mut h = Health::default();
        read_home_log(&[START, W1, BASS, LIB, WIDE, FREE].join("\n"), &mut h);
        assert_eq!(h.last_boot, LastBoot::Started);
        assert_eq!(h.walkman_one, Some(true));
        assert_eq!(h.tracks, Some((3463, 1130)));
        assert_eq!(h.undecodable, Some(2));
        assert_eq!(h.data_free_kb, Some(4096));
        assert!(lines(&h).contains(&"System space free: 4 MB, low".to_string()));

        let mut h = Health::default();
        read_home_log(&[START, W1, BASS, START, BASS_STOCK, CABLE].join("\n"), &mut h);
        assert_eq!(
            h.last_boot,
            LastBoot::ToStock(
                "a USB cable was connected at boot (rung 0; /data/cinder/cable_escape_off opts out)".into()
            )
        );
        assert_eq!(h.walkman_one, Some(false), "the later boot found Sony's library");
    }

    #[test]
    fn the_drive_gives_the_card_and_the_report() {
        let d = std::env::temp_dir().join(format!("flint-health-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        assert_eq!(lines(&read(&d))[0], "No Cinder install found on this drive");
        fs::write(d.join(CONF), "# channel: stable\n# installer: 0.3.20-rc1\npower=1\n").unwrap();
        fs::write(d.join(INSTALL_LOG), "staging\ninstall: done\n\n").unwrap();
        let log: String = (0..200).map(|i| format!("line {i}\n")).collect::<String>() + START + "\n";
        fs::write(d.join(HOME_LOG), log).unwrap();
        let h = read(&d);
        assert_eq!(lines(&h)[..2], ["Cinder from installer 0.3.20-rc1 (stable)", "Last start: Cinder started"]);
        assert_eq!(h.install.as_deref(), Some("install: done"));
        let r = report(&d, &h);
        fs::remove_dir_all(&d).unwrap();
        assert!(r.contains("--- cinderhome.log, last 80 of 201 lines ---\nline 121\n"), "{r}");
        assert!(r.ends_with("--- cinder_home_install.log, last 3 of 3 lines ---\nstaging\ninstall: done\n\n"));
    }
}
