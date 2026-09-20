//! Last.fm: the only network Flint touches.
//!
//! Four calls, and each one exists because the Walkman cannot make it itself — it has no WiFi, so
//! every play and every like has to travel as a file to the PC and go out from here:
//!
//! | call | why |
//! |---|---|
//! | `auth.getMobileSession` | once, to turn a username and password into a session key |
//! | `track.scrobble` | the plays out of `.scrobbler.log`, fifty at a time |
//! | `user.getLovedTracks` | what is loved on Last.fm, to compare with the player |
//! | `track.love` / `track.unlove` | the player's likes, going the other way |
//!
//! **The signature.** Every authenticated call carries `api_sig`: the parameters sorted by name,
//! concatenated as `name value name value …` with the shared secret on the end, MD5'd. It is
//! computed over the RAW values, before any URL encoding — getting that backwards produces a
//! signature Last.fm rejects with "Invalid method signature", which says nothing about which side
//! is wrong, so [`sign`] is tested directly.
//!
//! **The password is never stored.** `authenticate` exchanges it for a session key that is good
//! until the user revokes it, and only that key is written down.

use std::time::{Duration, Instant};

use crate::http;
use crate::md5;
use crate::scrobblelog::Entry;
use crate::xml::{self, Node};

pub const API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
/// Last.fm asks for no more than five requests a second, averaged. Loves are one call each, so a
/// first push of 300 tracks takes about a minute — that is the API's floor, not this client's.
const MIN_INTERVAL: Duration = Duration::from_millis(220);
const RETRIES: u32 = 3;
/// Last.fm's own "you are going too fast" code.
const RATE_LIMITED: &str = "29";
/// The most scrobbles one `track.scrobble` call takes.
pub const BATCH: usize = 50;

#[derive(Debug)]
pub enum Error {
    /// Last.fm answered, and said no. The code is theirs; 9 is a dead session key, 4 a bad login.
    Api { code: String, message: String },
    /// Nothing usable came back: no network, a proxy, a truncated reply.
    Transport(String),
    /// Flint has not been given what it needs to make the call at all.
    NotConfigured(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Api { code, message } => write!(f, "Last.fm error {code}: {message}"),
            Error::Transport(why) => write!(f, "{why}"),
            Error::NotConfigured(why) => write!(f, "{why}"),
        }
    }
}

/// What Flint knows about the account. The API key and secret identify the *application* and come
/// from the person's own Last.fm API account; the session key identifies them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credentials {
    pub api_key: String,
    pub api_secret: String,
    pub session_key: String,
    pub username: String,
}

impl Credentials {
    pub fn is_ready(&self) -> bool {
        !self.api_key.is_empty() && !self.api_secret.is_empty() && !self.session_key.is_empty()
    }

    /// Where the file lives: beside the scan cache, so everything Flint keeps is in one directory.
    pub fn path() -> std::path::PathBuf {
        crate::cache::default_dir().join("lastfm.conf")
    }

    /// The file, then the environment on top — the same four names the Python tool used, so an
    /// existing `.env` still works if it is exported.
    pub fn load() -> Credentials {
        let mut creds = Credentials::read(&Credentials::path()).unwrap_or_default();
        for (name, field) in [
            ("LASTFM_API_KEY", &mut creds.api_key),
            ("LASTFM_API_SECRET", &mut creds.api_secret),
            ("LASTFM_SESSION_KEY", &mut creds.session_key),
            ("LASTFM_USERNAME", &mut creds.username),
        ] {
            if let Ok(value) = std::env::var(name) {
                if !value.trim().is_empty() {
                    *field = value.trim().to_string();
                }
            }
        }
        creds
    }

    pub fn read(path: &std::path::Path) -> Option<Credentials> {
        let body = std::fs::read_to_string(path).ok()?;
        Some(Credentials::parse(&body))
    }

    pub fn parse(body: &str) -> Credentials {
        let mut creds = Credentials::default();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim().to_string();
            match key.trim() {
                "api_key" => creds.api_key = value,
                "api_secret" => creds.api_secret = value,
                "session_key" => creds.session_key = value,
                "username" => creds.username = value,
                _ => {}
            }
        }
        creds
    }

    pub fn render(&self) -> String {
        format!(
            "# Flint's Last.fm credentials. The password was never stored: `session_key` is what\n\
             # `flint lastfm login` got in exchange for it, and revoking it at last.fm/settings/applications\n\
             # makes this file useless. Anyone who can read it can scrobble and love as you.\n\
             api_key={}\napi_secret={}\nsession_key={}\nusername={}\n",
            self.api_key, self.api_secret, self.session_key, self.username
        )
    }

    /// Write it out, owner-readable only where the platform has such a thing.
    pub fn save(&self) -> std::io::Result<std::path::PathBuf> {
        let path = Credentials::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, self.render())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

/// `name value name value … secret`, MD5'd. Sorted by parameter name, over the raw values.
pub fn sign(params: &[(String, String)], secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut base = String::new();
    for (name, value) in sorted {
        base.push_str(name);
        base.push_str(value);
    }
    base.push_str(secret);
    md5::hex(base.as_bytes())
}

/// How a call proves who it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Signing {
    /// Public data: the API key is enough (`user.getLovedTracks`).
    Open,
    /// Acting as the signed-in person: session key plus signature.
    AsUser,
    /// Signed with no session, which only `auth.getMobileSession` is.
    WithoutSession,
}

pub struct Client {
    pub creds: Credentials,
    last_call: Option<Instant>,
}

/// One track as Last.fm reports it loved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loved {
    pub artist: String,
    pub title: String,
    /// When it was loved, unix seconds; `0` when Last.fm did not say.
    pub when: i64,
}

/// What came back from a `track.scrobble` batch.
#[derive(Clone, Debug, Default)]
pub struct Accepted {
    pub accepted: usize,
    pub ignored: usize,
    /// `(entry, why)` for each row Last.fm would not take — a timestamp too old, an artist it
    /// refuses to file. These stay in the log.
    pub rejected: Vec<(Entry, String)>,
}

impl Client {
    pub fn new(creds: Credentials) -> Result<Client, Error> {
        if creds.api_key.is_empty() || creds.api_secret.is_empty() {
            return Err(Error::NotConfigured(
                "no Last.fm API key yet — create one at https://www.last.fm/api/account/create, \
                 then run: flint lastfm key <api-key> <api-secret>"
                    .into(),
            ));
        }
        Ok(Client { creds, last_call: None })
    }

    fn throttle(&mut self) {
        if let Some(last) = self.last_call {
            let elapsed = last.elapsed();
            if elapsed < MIN_INTERVAL {
                std::thread::sleep(MIN_INTERVAL - elapsed);
            }
        }
        self.last_call = Some(Instant::now());
    }

    /// One API call, with the retries that are worth making. `post` is required for everything
    /// that writes.
    fn call(&mut self, params: Vec<(String, String)>, signing: Signing, post: bool) -> Result<Node, Error> {
        let mut payload = params;
        payload.push(("api_key".into(), self.creds.api_key.clone()));
        match signing {
            Signing::Open => {}
            Signing::AsUser => {
                if self.creds.session_key.is_empty() {
                    return Err(Error::NotConfigured(
                        "not signed in to Last.fm — run: flint lastfm login <username>".into(),
                    ));
                }
                payload.push(("sk".into(), self.creds.session_key.clone()));
                let signature = sign(&payload, &self.creds.api_secret);
                payload.push(("api_sig".into(), signature));
            }
            // Signed, but there is no session yet — that is what the call is for. Last.fm signs
            // `auth.getMobileSession` over api_key, method, password and username and nothing else,
            // so an empty `sk` must not be in the signature or in the request.
            Signing::WithoutSession => {
                let signature = sign(&payload, &self.creds.api_secret);
                payload.push(("api_sig".into(), signature));
            }
        }

        let mut attempt = 0;
        loop {
            self.throttle();
            let sent = if post { http::post_form(API_URL, &payload) } else { http::get_form(API_URL, &payload) };
            let response = match sent {
                Ok(r) => r,
                Err(e) => {
                    if attempt < RETRIES {
                        back_off(attempt);
                        attempt += 1;
                        continue;
                    }
                    return Err(Error::Transport(format!("could not reach Last.fm: {e}")));
                }
            };
            // 429 and the 5xx family are worth another go; anything else, read the body — Last.fm
            // puts a real explanation in it even when the status is 4xx.
            if matches!(response.status, 429 | 500 | 502 | 503 | 504) && attempt < RETRIES {
                back_off(attempt);
                attempt += 1;
                continue;
            }
            let Some(root) = xml::parse(&response.body) else {
                if attempt < RETRIES {
                    back_off(attempt);
                    attempt += 1;
                    continue;
                }
                return Err(Error::Transport(format!(
                    "Last.fm returned something that is not XML (HTTP {})",
                    response.status
                )));
            };
            if root.attr("status") == Some("ok") {
                return Ok(root);
            }
            let (code, message) = match root.child("error") {
                Some(node) => (
                    node.attr("code").unwrap_or("?").to_string(),
                    node.text.trim().to_string(),
                ),
                None => ("?".into(), format!("HTTP {}", response.status)),
            };
            if code == RATE_LIMITED && attempt < RETRIES {
                back_off(attempt);
                attempt += 1;
                continue;
            }
            return Err(Error::Api { code, message });
        }
    }

    /// Exchange a password for a session key. The password is used here and nowhere else.
    pub fn authenticate(&mut self, username: &str, password: &str) -> Result<(String, String), Error> {
        let root = self.call(
            vec![
                ("method".into(), "auth.getMobileSession".into()),
                ("username".into(), username.into()),
                ("password".into(), password.into()),
            ],
            Signing::WithoutSession,
            true,
        )?;
        let session = root.child("session").ok_or_else(|| Error::Transport("no session came back".into()))?;
        let name = session.text_at("name");
        let key = session.text_at("key");
        if name.is_empty() || key.is_empty() {
            return Err(Error::Transport("the session Last.fm returned is incomplete".into()));
        }
        self.creds.username = name.clone();
        self.creds.session_key = key.clone();
        Ok((name, key))
    }

    /// Every loved track, paged 1000 at a time. `progress(page, pages, so_far)` if you want to say
    /// something while it runs.
    pub fn loved_tracks(&mut self, mut progress: impl FnMut(u32, u32, usize)) -> Result<Vec<Loved>, Error> {
        if self.creds.username.is_empty() {
            return Err(Error::NotConfigured("no Last.fm username — run: flint lastfm login <username>".into()));
        }
        let user = self.creds.username.clone();
        let mut loved = Vec::new();
        let (mut page, mut pages) = (1u32, 1u32);
        while page <= pages {
            let root = self.call(
                vec![
                    ("method".into(), "user.getLovedTracks".into()),
                    ("user".into(), user.clone()),
                    ("limit".into(), "1000".into()),
                    ("page".into(), page.to_string()),
                ],
                Signing::Open,
                false,
            )?;
            let Some(container) = root.child("lovedtracks") else { break };
            pages = container.attr("totalPages").and_then(|v| v.parse().ok()).unwrap_or(1);
            for track in container.children_named("track") {
                let title = track.text_at("name");
                let artist = track.text_at("artist/name");
                if artist.is_empty() || title.is_empty() {
                    continue;
                }
                let when = track
                    .child("date")
                    .and_then(|d| d.attr("uts"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                loved.push(Loved { artist, title, when });
            }
            progress(page, pages, loved.len());
            page += 1;
        }
        Ok(loved)
    }

    pub fn love(&mut self, artist: &str, title: &str) -> Result<(), Error> {
        self.call(
            vec![
                ("method".into(), "track.love".into()),
                ("artist".into(), artist.into()),
                ("track".into(), title.into()),
            ],
            Signing::AsUser,
            true,
        )
        .map(|_| ())
    }

    pub fn unlove(&mut self, artist: &str, title: &str) -> Result<(), Error> {
        self.call(
            vec![
                ("method".into(), "track.unlove".into()),
                ("artist".into(), artist.into()),
                ("track".into(), title.into()),
            ],
            Signing::AsUser,
            true,
        )
        .map(|_| ())
    }

    /// One `track.scrobble` batch — at most [`BATCH`] plays.
    pub fn scrobble(&mut self, entries: &[Entry]) -> Result<Accepted, Error> {
        if entries.is_empty() {
            return Ok(Accepted::default());
        }
        if entries.len() > BATCH {
            return Err(Error::Transport(format!("a scrobble batch is at most {BATCH} plays")));
        }
        let mut params = vec![("method".to_string(), "track.scrobble".to_string())];
        for (index, entry) in entries.iter().enumerate() {
            params.push((format!("artist[{index}]"), entry.artist.clone()));
            params.push((format!("track[{index}]"), entry.track.clone()));
            params.push((format!("timestamp[{index}]"), entry.timestamp.to_string()));
            if !entry.album.is_empty() {
                params.push((format!("album[{index}]"), entry.album.clone()));
            }
            if !entry.track_number.is_empty() {
                params.push((format!("trackNumber[{index}]"), entry.track_number.clone()));
            }
            if entry.duration > 0 {
                params.push((format!("duration[{index}]"), entry.duration.to_string()));
            }
        }
        let root = self.call(params, Signing::AsUser, true)?;
        let Some(scrobbles) = root.child("scrobbles") else {
            return Err(Error::Transport("Last.fm did not say what it did with the batch".into()));
        };
        let accepted = scrobbles.attr("accepted").and_then(|v| v.parse().ok()).unwrap_or(0);
        let ignored = scrobbles.attr("ignored").and_then(|v| v.parse().ok()).unwrap_or(0);
        let nodes: Vec<&Node> = scrobbles.children_named("scrobble").collect();
        if nodes.len() != entries.len() {
            // Without one result per play there is no way to know WHICH were taken, and removing
            // the wrong rows loses plays for good. Keep them all and say so.
            return Err(Error::Transport(format!(
                "Last.fm returned {} results for {} plays — none removed",
                nodes.len(),
                entries.len()
            )));
        }
        let mut rejected = Vec::new();
        for (entry, node) in entries.iter().zip(nodes) {
            let Some(message) = node.child("ignoredMessage") else { continue };
            let code = message.attr("code").unwrap_or("0");
            if code == "0" {
                continue;
            }
            let reason = match message.text.trim() {
                "" => "rejected by Last.fm".to_string(),
                text => text.to_string(),
            };
            rejected.push((entry.clone(), format!("{reason} (code {code})")));
        }
        Ok(Accepted { accepted, ignored, rejected })
    }
}

fn back_off(attempt: u32) {
    std::thread::sleep(Duration::from_secs(1u64 << attempt));
}

#[cfg(test)]
mod tests {
    use super::{sign, Credentials};

    /// The worked example from Last.fm's own signature documentation shape: sorted by name, raw
    /// values, secret on the end. Pinned against an independently computed digest.
    #[test]
    fn the_signature_is_sorted_raw_and_salted() {
        let params = vec![
            ("method".to_string(), "track.love".to_string()),
            ("artist".to_string(), "Sigur Rós".to_string()),
            ("track".to_string(), "Hoppípolla".to_string()),
            ("api_key".to_string(), "KEY".to_string()),
            ("sk".to_string(), "SESSION".to_string()),
        ];
        // api_key KEY artist Sigur Rós method track.love sk SESSION track Hoppípolla + SECRET
        let expected = crate::md5::hex(
            "api_keyKEYartistSigur Rósmethodtrack.loveskSESSIONtrackHoppípollaSECRET".as_bytes(),
        );
        assert_eq!(sign(&params, "SECRET"), expected);
    }

    /// Batched scrobbles sort as text — `artist[10]` before `artist[2]` — and both sides only have
    /// to agree with themselves, but the ordering must not depend on the order they were added.
    #[test]
    fn signing_does_not_depend_on_insertion_order() {
        let a = vec![("b".to_string(), "2".to_string()), ("a".to_string(), "1".to_string())];
        let b = vec![("a".to_string(), "1".to_string()), ("b".to_string(), "2".to_string())];
        assert_eq!(sign(&a, "s"), sign(&b, "s"));
    }

    #[test]
    fn credentials_round_trip_and_never_hold_a_password() {
        let creds = Credentials {
            api_key: "k".into(),
            api_secret: "s".into(),
            session_key: "sess".into(),
            username: "Superwils0".into(),
        };
        let rendered = creds.render();
        assert!(!rendered.lines().any(|l| l.trim_start().starts_with("password")), "no password field");
        assert_eq!(Credentials::parse(&rendered), creds);
        // A file with comments and blank lines still reads.
        assert_eq!(Credentials::parse("# note\n\napi_key = k \n").api_key, "k");
    }

    #[test]
    fn credentials_are_not_ready_until_all_three_are_there() {
        let mut creds = Credentials { api_key: "k".into(), api_secret: "s".into(), ..Credentials::default() };
        assert!(!creds.is_ready(), "no session key yet");
        creds.session_key = "sess".into();
        assert!(creds.is_ready());
    }
}
