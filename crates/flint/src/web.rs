//! The window on Linux and macOS: a page in your browser, served by Flint to this computer alone.
//!
//! WHY A PAGE. The Windows window is Win32 drawn by hand; doing that again for GTK and Cocoa would
//! be two more windows to keep in step with the command line. Every computer has a browser, so
//! Flint listens on 127.0.0.1 only, with a random port and a random token in the address it opens;
//! a request without the token, or for any other host name (DNS rebinding), is refused.
//!
//! Each button runs Flint itself with the arguments a terminal would type, as a child process, and
//! shows what it prints. So the page can do nothing the command line cannot, and the other way
//! round is held by `page_parity` below: every command and option in `USAGE` is on the page.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const PAGE: &str = include_str!("page.html");
/// What the page remembers (folders, switches), beside everything else Flint keeps.
const SAVED: &str = "page.json";
/// The log kept for the page. A sync of a large library prints a line per file; the page only
/// needs the recent part.
const LOG_KEEP: usize = 400 << 10;

static BUSY: AtomicBool = AtomicBool::new(false);
static LOG: Mutex<String> = Mutex::new(String::new());
/// How the last command ended: "", "ok", "failed" or "stopped".
static LAST: Mutex<&str> = Mutex::new("");
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

fn log(s: &str) {
    let mut l = LOG.lock().unwrap();
    l.push_str(s);
    if l.len() > LOG_KEEP {
        let cut = l.len() - LOG_KEEP / 2;
        let cut = l[cut..].find('\n').map_or(cut, |i| cut + i + 1);
        l.drain(..cut);
    }
}

pub fn serve() -> Result<(), String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("127.0.0.1: {e}"))?;
    let port = l.local_addr().map_err(|e| e.to_string())?.port();
    let token = token();
    let url = format!("http://127.0.0.1:{port}/?t={token}");
    println!("Flint is open in your browser. If it is not, open this address:\n  {url}");
    println!("Keep this window open while you use it; close it to quit. `flint help` lists the commands.");
    open_browser(&url);
    for s in l.incoming().flatten() {
        let _ = handle(s, port, &token);
    }
    Ok(())
}

fn token() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let mut seed = format!("{t:?} {} {:p}", std::process::id(), &t);
    // Kernel randomness where there is some; the clock and the address above are the fallback.
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let mut b = [0u8; 16];
        if f.read_exact(&mut b).is_ok() {
            seed.push_str(&format!("{b:?}"));
        }
    }
    let a = flint_core::md5::hex(seed.as_bytes());
    let b = flint_core::md5::hex(format!("{seed}{a}").as_bytes());
    a + &b
}

fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = Command::new(opener).arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

fn handle(s: TcpStream, port: u16, token: &str) -> Result<(), String> {
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(&s);
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut words = line.split(' ');
    let (method, target) = (words.next().unwrap_or("").to_string(), words.next().unwrap_or("").to_string());
    let (mut host, mut len) = (String::new(), 0usize);
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 || h.trim().is_empty() {
            break;
        }
        let (k, v) = h.split_once(':').unwrap_or((&h, ""));
        match k.trim().to_ascii_lowercase().as_str() {
            "host" => host = v.trim().to_string(),
            "content-length" => len = v.trim().parse().unwrap_or(0).min(1 << 16),
            _ => {}
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    let body = String::from_utf8_lossy(&body).into_owned();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let authorised = (host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}"))
        && query.split('&').any(|kv| kv == format!("t={token}"));
    let mut s = s;
    if !authorised {
        return reply(&mut s, "403 Forbidden", "text/plain", "Not this page's address.");
    }
    match (method.as_str(), path) {
        ("GET", "/") => reply(&mut s, "200 OK", "text/html; charset=utf-8", PAGE),
        ("GET", "/state") => reply(&mut s, "200 OK", "application/json", &state()),
        ("GET", "/saved") => {
            let saved = std::fs::read_to_string(saved_path()).unwrap_or_else(|_| "{}".into());
            reply(&mut s, "200 OK", "application/json", &saved)
        }
        ("POST", "/saved") => {
            let p = saved_path();
            let _ = p.parent().map(std::fs::create_dir_all);
            let ok = std::fs::write(&p, &body).is_ok();
            reply(&mut s, if ok { "200 OK" } else { "500 Internal Server Error" }, "text/plain", "")
        }
        ("POST", "/run") => {
            // One argument per line, exactly as the terminal would pass them; after a NUL, what to
            // type at its prompt (the Last.fm password, which is then never kept anywhere).
            let (args, input) = body.split_once('\0').unwrap_or((&body, ""));
            let args: Vec<String> = args.lines().map(str::to_string).collect();
            if !args.first().is_some_and(|c| COMMANDS.contains(&c.as_str())) {
                return reply(&mut s, "400 Bad Request", "text/plain", "Not a command.");
            }
            if BUSY.swap(true, Ordering::SeqCst) {
                return reply(&mut s, "409 Conflict", "text/plain", "Flint is already working.");
            }
            *LAST.lock().unwrap() = "";
            let input = input.to_string();
            std::thread::spawn(move || {
                let end = run(&args, &input);
                *LAST.lock().unwrap() = end;
                BUSY.store(false, Ordering::SeqCst);
            });
            reply(&mut s, "202 Accepted", "text/plain", "")
        }
        ("POST", "/cancel") => {
            if let Some(c) = CHILD.lock().unwrap().as_mut() {
                let _ = c.kill();
                *LAST.lock().unwrap() = "stopped";
            }
            reply(&mut s, "200 OK", "text/plain", "")
        }
        _ => reply(&mut s, "404 Not Found", "text/plain", ""),
    }
}

/// The commands the page may run: the command line's own, less the developer tools and the
/// window itself.
const COMMANDS: &[&str] = &[
    "scan",
    "check",
    "sync",
    "shared",
    "lint",
    "lyrics",
    "import",
    "lastfm",
    "scrobble",
    "likes",
    "stats",
    "playlists",
    "backup",
    "restore",
];

/// Run `flint <args>` and copy what it prints into the log. Returns how it ended.
fn run(args: &[String], input: &str) -> &'static str {
    // What the page shows as the command: the password is never in `args`, so this is safe to show.
    log(&format!("\n> flint {}\n", args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")));
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log(&format!("flint: could not find itself to run: {e}\n"));
            return "failed";
        }
    };
    let child = Command::new(exe)
        .args(args)
        .stdin(if input.is_empty() { Stdio::null() } else { Stdio::piped() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            log(&format!("flint: could not start: {e}\n"));
            return "failed";
        }
    };
    if let Some(mut i) = child.stdin.take() {
        let _ = writeln!(i, "{input}");
    }
    let pump = |r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut r = BufReader::new(r);
            let mut buf = Vec::new();
            while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                log(&String::from_utf8_lossy(&buf));
                buf.clear();
            }
        })
    };
    let out = pump(Box::new(child.stdout.take().unwrap()));
    let err = pump(Box::new(child.stderr.take().unwrap()));
    *CHILD.lock().unwrap() = Some(child);
    let status = loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut c = CHILD.lock().unwrap();
        match c.as_mut().map(Child::try_wait) {
            Some(Ok(Some(st))) => break Some(st),
            Some(Ok(None)) => continue,
            _ => break None,
        }
    };
    *CHILD.lock().unwrap() = None;
    let _ = (out.join(), err.join());
    if *LAST.lock().unwrap() == "stopped" {
        log("Stopped.\n");
        return "stopped";
    }
    if status.is_some_and(|s| s.success()) {
        "ok"
    } else {
        "failed"
    }
}

fn quote(a: &str) -> String {
    if !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=,".contains(c)) {
        a.to_string()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

fn saved_path() -> PathBuf {
    flint_core::cache::default_dir().join(SAVED)
}

fn reply(s: &mut TcpStream, status: &str, kind: &str, body: &str) -> Result<(), String> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())
}

fn json(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o += "\\\"",
            '\\' => o += "\\\\",
            '\n' => o += "\\n",
            '\r' => o += "\\r",
            c if (c as u32) < 0x20 => o += &format!("\\u{:04x}", c as u32),
            c => o.push(c),
        }
    }
    o + "\""
}

/// Where a player or its card might be mounted: every folder there with a `MUSIC` folder in it.
fn players() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let parents: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec!["/Volumes".into()]
    } else {
        vec![format!("/media/{user}").into(), format!("/run/media/{user}").into(), "/media".into(), "/mnt".into()]
    };
    let mut found = vec![];
    for p in parents {
        for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
            let d = e.path();
            if has_music(&d) && !found.contains(&d) {
                found.push(d);
            }
        }
    }
    found.sort();
    found
}

fn has_music(d: &Path) -> bool {
    std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("music") && e.path().is_dir())
}

/// What the page draws from: the log, whether a command is running, and where things might be.
fn state() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let players: Vec<String> = players().iter().map(|p| json(&p.display().to_string())).collect();
    format!(
        "{{\"busy\":{},\"last\":{},\"os\":{},\"home\":{},\"players\":[{}],\"log\":{}}}",
        BUSY.load(Ordering::SeqCst),
        json(&LAST.lock().unwrap()),
        json(std::env::consts::OS),
        json(&home),
        players.join(","),
        json(&LOG.lock().unwrap())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request from another page, or without the token, gets nothing; with both it gets the page.
    #[test]
    fn only_this_address_with_its_token_is_served() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let ask = |req: String| {
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            c.write_all(req.as_bytes()).unwrap();
            let (s, _) = l.accept().unwrap();
            handle(s, port, "tok").unwrap();
            let mut out = String::new();
            c.read_to_string(&mut out).unwrap();
            out
        };
        let host = format!("127.0.0.1:{port}");
        assert!(ask(format!("GET /?t=tok HTTP/1.1\r\nHost: {host}\r\n\r\n")).contains("200 OK"));
        assert!(ask(format!("GET /?t=bad HTTP/1.1\r\nHost: {host}\r\n\r\n")).contains("403"));
        assert!(ask("GET /?t=tok HTTP/1.1\r\nHost: evil.example:80\r\n\r\n".into()).contains("403"));
        let body = "gui-preview\n/tmp/x.svg";
        let run = format!("POST /run?t=tok HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        assert!(ask(run).contains("400"));
        assert!(ask(format!("GET /state?t=tok HTTP/1.1\r\nHost: {host}\r\n\r\n")).contains("\"busy\":false"));
    }

    /// The page and the command line never drift apart: every command in `USAGE` that the page may
    /// run has a builder on the page (`// cmd <name>`), and every option of it appears in that
    /// builder. Left out on purpose: the developer tools, the window, and `--jobs`/`--cache` (the
    /// page uses every core and the default data folder, as the Windows window does).
    #[test]
    fn page_parity() {
        const LEFT_OUT: &[&str] = &["--jobs", "--cache", "--param", "--out", "--smfmf", "--dark", "--light", "--state"];
        let blocks: Vec<(&str, &str)> =
            PAGE.split("// cmd ").skip(1).map(|b| b.split_once(char::is_whitespace).unwrap_or((b, ""))).collect();
        for line in crate::USAGE.lines().filter_map(|l| l.trim().strip_prefix("flint ")) {
            let cmd = line.split_whitespace().next().unwrap();
            if !COMMANDS.contains(&cmd) {
                continue;
            }
            let (_, js) =
                blocks.iter().find(|(c, _)| *c == cmd).unwrap_or_else(|| panic!("no `// cmd {cmd}` on the page"));
            for word in line.split(|c: char| c.is_whitespace() || "[]|".contains(c)) {
                if word == "--apply" {
                    // Added by the job's Apply button, which runs only what was just previewed.
                    let row = PAGE.split(&format!("data-cmd=\"{cmd}\"")).nth(1).unwrap_or("");
                    let row = row.split("data-cmd=").next().unwrap();
                    assert!(row.contains("data-do=\"apply\""), "the page's {cmd} has no Apply button");
                } else if word.starts_with("--") && !LEFT_OUT.contains(&word) {
                    assert!(js.contains(&format!("'{word}'")), "the page's {cmd} never passes {word}");
                }
            }
            if cmd == "lastfm" {
                for sub in ["'key'", "'login'", "'status'"] {
                    assert!(js.contains(sub), "the page's lastfm never runs {sub}");
                }
            }
        }
        // …and the page runs nothing the server would refuse.
        for (c, _) in &blocks {
            assert!(COMMANDS.contains(c), "the page has `// cmd {c}`, which /run refuses");
        }
    }

    #[test]
    fn quoting_shows_what_a_terminal_would_need() {
        assert_eq!(quote("/media/me/WALKMAN/MUSIC"), "/media/me/WALKMAN/MUSIC");
        assert_eq!(quote("my music"), "'my music'");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }
}
