//! What a Cinder player with Wi-Fi reads from its internal memory: networks to join
//! (`cinder_wifi.txt`, taken in and deleted the next time Wi-Fi turns on), internet radio stations
//! (`cinder_radio.txt`) and the ListenBrainz token (`cinder_listenbrainz.txt`). The formats are
//! Cinder's own (its `wifi/README.md` and `cinder-ui/src/netradio.rs`); this only writes them.
//! Every `root` may be the drive or its `MUSIC` folder ([`crate::stats::drive_root`]).

use std::path::Path;

use crate::lastfm_sync::write_atomic;

pub const WIFI: &str = "cinder_wifi.txt";
pub const RADIO: &str = "cinder_radio.txt";
pub const LISTENBRAINZ: &str = "cinder_listenbrainz.txt";

/// What the three files say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Networks waiting in `cinder_wifi.txt`, by name. Once the player has taken them in, they
    /// live in its own memory, which Flint cannot read.
    pub wifi_waiting: Vec<String>,
    pub stations: Vec<Station>,
    pub listenbrainz: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Station {
    pub name: String,
    pub url: String,
    /// The file's lines that make it: its own, and an `#EXTINF` before it.
    lines: (usize, Option<usize>),
}

pub fn read(root: &Path) -> Facts {
    let text = |f: &str| std::fs::read_to_string(crate::stats::drive_root(root).join(f)).unwrap_or_default();
    Facts {
        wifi_waiting: wifi_lines(&text(WIFI)).into_iter().map(|(n, _)| n).collect(),
        stations: parse_stations(&text(RADIO)),
        listenbrainz: !text(LISTENBRAINZ).trim().is_empty(),
    }
}

fn good_url(u: &str) -> bool {
    (u.starts_with("http://") || u.starts_with("https://")) && u.len() <= 512 && u.chars().all(|c| c.is_ascii_graphic())
}

/// Cinder's reading of the file, line for line: `Name | URL`, an `#EXTINF:-1,Name` before a URL,
/// or a bare URL; at most 200.
pub fn parse_stations(text: &str) -> Vec<Station> {
    let mut out = Vec::new();
    let mut pending: Option<(String, usize)> = None;
    for (i, line) in text.lines().enumerate() {
        let l = line.trim().trim_start_matches('\u{feff}');
        if let Some(rest) = l.strip_prefix("#EXTINF:") {
            pending = rest.split_once(',').map(|(_, n)| (n.trim().to_string(), i));
            continue;
        }
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (name, url, extinf) = match l.rsplit_once('|') {
            Some((n, u)) => (n.trim().to_string(), u.trim(), None),
            None => {
                let p = pending.take();
                (p.as_ref().map(|p| p.0.clone()).unwrap_or_default(), l, p.map(|p| p.1))
            }
        };
        if !good_url(url) {
            continue;
        }
        let name = if name.is_empty() {
            url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url).to_string()
        } else {
            name.chars().take(60).collect()
        };
        out.push(Station { name, url: url.to_string(), lines: (i, extinf) });
        if out.len() == 200 {
            break;
        }
    }
    out
}

/// A name or password goes into a `|`-separated line: no `|`, no line break.
fn one_field(what: &str, s: &str) -> Result<(), String> {
    if s.contains(['|', '\n', '\r']) {
        return Err(format!("the {what} cannot hold | or a line break"));
    }
    Ok(())
}

fn with_line(path: &Path, body: String, line: &str) -> Result<(), String> {
    let mut body = body;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(line);
    body.push('\n');
    write_atomic(path, &body).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn add_station(root: &Path, name: &str, url: &str) -> Result<(), String> {
    let (name, url) = (name.trim(), url.trim());
    if !good_url(url) {
        return Err("a station's address starts http:// or https:// and has no spaces".into());
    }
    one_field("station's name", name)?;
    let path = crate::stats::drive_root(root).join(RADIO);
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    if parse_stations(&body).iter().any(|s| s.url == url) {
        return Err(format!("{url} is on the player already"));
    }
    if parse_stations(&body).len() >= 200 {
        return Err("the player shows 200 stations at most: remove one first".into());
    }
    let line = if name.is_empty() { url.to_string() } else { format!("{name} | {url}") };
    with_line(&path, body, &line)
}

/// Take station `index` (in [`Facts::stations`] order) out of the file. Returns its name.
pub fn remove_station(root: &Path, index: usize) -> Result<String, String> {
    let path = crate::stats::drive_root(root).join(RADIO);
    let body = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let st = parse_stations(&body).into_iter().nth(index).ok_or("no such station")?;
    let keep: Vec<&str> =
        body.lines().enumerate().filter(|(i, _)| *i != st.lines.0 && Some(*i) != st.lines.1).map(|(_, l)| l).collect();
    let mut out = keep.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    write_atomic(&path, &out).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(st.name)
}

/// The `Name | …` lines of `cinder_wifi.txt`, as (name, the whole line).
fn wifi_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| (l.split('|').next().unwrap_or("").trim().to_string(), l.to_string()))
        .collect()
}

/// Queue a WPA2/WPA3 network (an empty password is an open one). The same name again replaces
/// the line, so a mistyped password is put right by adding it again.
pub fn add_wifi(root: &Path, name: &str, password: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 32 {
        return Err("a network's name is 1 to 32 characters".into());
    }
    one_field("network's name", name)?;
    one_field("password", password)?;
    // The player trims each field, so these would never match the network's real password.
    if password.trim() != password {
        return Err("the player cannot take a password that starts or ends with a space".into());
    }
    if !password.is_empty() && !(8..=63).contains(&password.chars().count()) {
        return Err("a Wi-Fi password is 8 to 63 characters (none for an open network)".into());
    }
    let path = crate::stats::drive_root(root).join(WIFI);
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let kept: Vec<String> =
        body.lines().filter(|l| wifi_lines(l).first().is_none_or(|(n, _)| n != name)).map(String::from).collect();
    let line = if password.is_empty() { name.to_string() } else { format!("{name} | {password}") };
    with_line(&path, kept.join("\n"), &line)
}

/// Save the ListenBrainz token; an empty one takes it off the player.
pub fn set_listenbrainz(root: &Path, token: &str) -> Result<(), String> {
    let path = crate::stats::drive_root(root).join(LISTENBRAINZ);
    let token = token.trim();
    if token.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
            _ => Ok(()),
        };
    }
    if token.len() > 100 || !token.chars().all(|c| c.is_ascii_graphic()) {
        return Err("a ListenBrainz token is one word, from listenbrainz.org/settings".into());
    }
    write_atomic(&path, &format!("{token}\n")).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stations_networks_and_the_token_round_trip() {
        let dir = std::env::temp_dir().join(format!("flint-player-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(RADIO),
            "# mine\n#EXTINF:-1,Groove Salad\nhttps://ice.somafm.com/groovesalad\nRadio Paradise | https://stream.radioparadise.com/mp3-192",
        )
        .unwrap();
        add_station(&dir, "Jazz", "http://jazz.example/stream").unwrap();
        assert!(add_station(&dir, "Again", "http://jazz.example/stream").is_err(), "no duplicates");
        assert!(add_station(&dir, "Bad", "ftp://x").is_err());
        let names = |d: &Path| read(d).stations.into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(&dir), ["Groove Salad", "Radio Paradise", "Jazz"]);
        assert_eq!(remove_station(&dir, 0).unwrap(), "Groove Salad");
        assert_eq!(names(&dir), ["Radio Paradise", "Jazz"]);
        assert!(
            std::fs::read_to_string(dir.join(RADIO)).unwrap().starts_with("# mine\nRadio Paradise"),
            "the EXTINF went too"
        );

        add_wifi(&dir, "Home", "wrongpass1").unwrap();
        add_wifi(&dir, "Cafe", "").unwrap();
        add_wifi(&dir, "Home", "rightpass1").unwrap();
        assert!(add_wifi(&dir, "Short", "1234").is_err());
        assert!(add_wifi(&dir, "A|B", "12345678").is_err());
        assert_eq!(std::fs::read_to_string(dir.join(WIFI)).unwrap(), "Cafe\nHome | rightpass1\n");
        assert_eq!(read(&dir).wifi_waiting, ["Cafe", "Home"]);

        assert!(!read(&dir).listenbrainz);
        assert_eq!(read(&dir.join("MUSIC")), read(&dir), "the player's MUSIC folder means its drive");
        set_listenbrainz(&dir, " abc-123 ").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(LISTENBRAINZ)).unwrap(), "abc-123\n");
        set_listenbrainz(&dir, "").unwrap();
        assert!(!read(&dir).listenbrainz);
        std::fs::remove_dir_all(&dir).ok();
    }
}
