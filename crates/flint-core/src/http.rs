//! The one place Flint talks to the network.
//!
//! Flint has no dependencies on purpose, and the standard library has no TLS — so this is not a
//! choice between crates, it is a choice between *whose* TLS. On Windows it is the system's:
//! WinHTTP, declared the way `flint-gui` declares `user32`, which means the proxy settings, the
//! certificate store and the TLS versions are the ones the machine is already configured with and
//! Flint owns none of them. Everywhere else — which in practice means development on Linux — it
//! shells out to `curl`, so the same code paths can be exercised without a Windows box.
//!
//! Scope is deliberately tiny: form-encoded POST and GET, text responses, one timeout. Last.fm is
//! the only thing on the other end, and a bigger client would only be more to get wrong.

use std::fmt;

/// What came back. The status is kept because Last.fm answers 4xx WITH a useful XML body, so the
/// caller has to see both — throwing on status alone would discard the error message.
pub struct Response {
    pub status: u16,
    pub body: String,
}

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error(s)
    }
}

/// `application/x-www-form-urlencoded`, the RFC 3986 way: unreserved characters pass, a space is
/// `%20` and not `+` (Last.fm's signature is computed over the RAW values, so the encoding only has
/// to survive the round trip), everything else is percent-encoded from its UTF-8 bytes.
pub fn encode_form(fields: &[(String, String)]) -> String {
    let mut out = String::new();
    for (key, value) in fields {
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&percent_encode(key));
        out.push('=');
        out.push_str(&percent_encode(value));
    }
    out
}

pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*byte as char),
            other => {
                out.push('%');
                out.push(char::from_digit(u32::from(other >> 4), 16).unwrap_or('0').to_ascii_uppercase());
                out.push(char::from_digit(u32::from(other & 0x0f), 16).unwrap_or('0').to_ascii_uppercase());
            }
        }
    }
    out
}

pub fn post_form(url: &str, fields: &[(String, String)]) -> Result<Response, Error> {
    let body = encode_form(fields);
    send(url, Some(&body))
}

pub fn get_form(url: &str, fields: &[(String, String)]) -> Result<Response, Error> {
    let query = encode_form(fields);
    let full = if query.is_empty() { url.to_string() } else { format!("{url}?{query}") };
    send(&full, None)
}

/// `https://host/path` → (host, path). Only https, because the only endpoint is Last.fm's and a
/// plain-http fallback would send a session key in clear.
fn split_url(url: &str) -> Result<(String, String), Error> {
    let rest = url.strip_prefix("https://").ok_or_else(|| Error(format!("{url}: only https URLs are supported")))?;
    match rest.find('/') {
        Some(cut) => Ok((rest[..cut].to_string(), rest[cut..].to_string())),
        None => Ok((rest.to_string(), "/".to_string())),
    }
}

#[cfg(windows)]
fn send(url: &str, body: Option<&str>) -> Result<Response, Error> {
    windows_impl::send(url, body)
}

#[cfg(not(windows))]
fn send(url: &str, body: Option<&str>) -> Result<Response, Error> {
    curl_impl::send(url, body)
}

// ── Windows: the system's own HTTP stack ───────────────────────────────────────────────────────

#[cfg(windows)]
mod windows_impl {
    use super::{split_url, Error, Response};
    use std::ffi::c_void;

    type Handle = *mut c_void;

    #[link(name = "winhttp")]
    extern "system" {
        fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> Handle;
        fn WinHttpConnect(session: Handle, host: *const u16, port: u16, reserved: u32) -> Handle;
        fn WinHttpOpenRequest(
            connect: Handle,
            verb: *const u16,
            object: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept_types: *const *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpSendRequest(
            request: Handle,
            headers: *const u16,
            headers_len: u32,
            optional: *const c_void,
            optional_len: u32,
            total_len: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(request: Handle, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            request: Handle,
            info_level: u32,
            name: *const u16,
            buffer: *mut c_void,
            buffer_len: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpQueryDataAvailable(request: Handle, available: *mut u32) -> i32;
        fn WinHttpReadData(request: Handle, buffer: *mut c_void, to_read: u32, read: *mut u32) -> i32;
        fn WinHttpCloseHandle(handle: Handle) -> i32;
        fn WinHttpSetTimeouts(handle: Handle, resolve: i32, connect: i32, send: i32, receive: i32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
    const INTERNET_DEFAULT_HTTPS_PORT: u16 = 443;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Every handle this module opens, closed in reverse order however the function leaves.
    struct Owned(Handle);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    pub fn send(url: &str, body: Option<&str>) -> Result<Response, Error> {
        let (host, path) = split_url(url)?;
        let agent = wide(concat!("flint/", env!("CARGO_PKG_VERSION")));
        let host_w = wide(&host);
        let path_w = wide(&path);
        let verb = wide(if body.is_some() { "POST" } else { "GET" });

        unsafe {
            let session = Owned(WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            ));
            if session.0.is_null() {
                return Err(Error(format!("could not start WinHTTP (error {})", GetLastError())));
            }
            // 30 s each, the same figure the Python tool used: long enough for a slow link, short
            // enough that a dead network does not look like a hang.
            WinHttpSetTimeouts(session.0, 30_000, 30_000, 30_000, 30_000);

            let connect = Owned(WinHttpConnect(session.0, host_w.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
            if connect.0.is_null() {
                return Err(Error(format!("could not reach {host} (error {})", GetLastError())));
            }

            let request = Owned(WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ));
            if request.0.is_null() {
                return Err(Error(format!("could not open the request (error {})", GetLastError())));
            }

            let headers = wide("Content-Type: application/x-www-form-urlencoded; charset=utf-8");
            let (header_ptr, header_len) = match body {
                Some(_) => (headers.as_ptr(), u32::MAX), // -1: NUL-terminated
                None => (std::ptr::null(), 0),
            };
            let bytes = body.unwrap_or("").as_bytes();
            let ok = WinHttpSendRequest(
                request.0,
                header_ptr,
                header_len,
                bytes.as_ptr().cast::<c_void>(),
                bytes.len() as u32,
                bytes.len() as u32,
                0,
            );
            if ok == 0 {
                return Err(Error(format!("could not send the request (error {})", GetLastError())));
            }
            if WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
                return Err(Error(format!("no response from {host} (error {})", GetLastError())));
            }

            let mut status: u32 = 0;
            let mut size = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&mut status as *mut u32).cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
            );

            let mut raw: Vec<u8> = Vec::new();
            loop {
                let mut available: u32 = 0;
                if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                    return Err(Error(format!("the response stopped early (error {})", GetLastError())));
                }
                if available == 0 {
                    break;
                }
                let start = raw.len();
                raw.resize(start + available as usize, 0);
                let mut read: u32 = 0;
                if WinHttpReadData(request.0, raw[start..].as_mut_ptr().cast::<c_void>(), available, &mut read) == 0 {
                    return Err(Error(format!("could not read the response (error {})", GetLastError())));
                }
                raw.truncate(start + read as usize);
                if read == 0 {
                    break;
                }
            }

            Ok(Response { status: status as u16, body: String::from_utf8_lossy(&raw).into_owned() })
        }
    }
}

// ── everywhere else: curl, so this is testable off Windows ─────────────────────────────────────

#[cfg(not(windows))]
mod curl_impl {
    use super::{split_url, Error, Response};
    use std::io::Write;
    use std::process::{Command, Stdio};

    pub fn send(url: &str, body: Option<&str>) -> Result<Response, Error> {
        // Same refusal as the Windows path: https or nothing, checked before curl is spawned.
        split_url(url)?;
        let mut command = Command::new("curl");
        command
            .arg("--silent")
            .arg("--show-error")
            .arg("--max-time")
            .arg("30")
            .arg("--user-agent")
            .arg(concat!("flint/", env!("CARGO_PKG_VERSION")))
            // The status code, on its own line, after the body: one call, both facts.
            .arg("--write-out")
            .arg("\n%{http_code}")
            .arg(url);
        if body.is_some() {
            command
                .arg("--header")
                .arg("Content-Type: application/x-www-form-urlencoded; charset=utf-8")
                .arg("--data-binary")
                .arg("@-");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error(format!("curl is needed for network access on this platform: {e}")))?;
        if let Some(text) = body {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).map_err(|e| Error(e.to_string()))?;
            }
        }
        let out = child.wait_with_output().map_err(|e| Error(e.to_string()))?;
        if !out.status.success() {
            return Err(Error(format!("curl failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let cut = text.rfind('\n').unwrap_or(text.len());
        let status = text[cut..].trim().parse::<u16>().unwrap_or(0);
        Ok(Response { status, body: text[..cut].to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_form, percent_encode, split_url};

    #[test]
    fn a_value_survives_encoding() {
        // Every character Last.fm's own signature would break on: space, ampersand, the
        // non-ASCII a tag is full of, and the plus a naive encoder would turn into a space.
        assert_eq!(percent_encode("Sigur Rós"), "Sigur%20R%C3%B3s");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(percent_encode("1+1"), "1%2B1");
        assert_eq!(percent_encode("safe-_.~"), "safe-_.~");
    }

    #[test]
    fn fields_join_in_order() {
        let fields = vec![("method".into(), "track.love".into()), ("artist".into(), "A B".into())];
        assert_eq!(encode_form(&fields), "method=track.love&artist=A%20B");
    }

    #[test]
    fn urls_split_into_host_and_path() {
        assert_eq!(
            split_url("https://ws.audioscrobbler.com/2.0/").unwrap(),
            ("ws.audioscrobbler.com".into(), "/2.0/".into())
        );
        assert_eq!(split_url("https://example.com").unwrap(), ("example.com".into(), "/".into()));
        assert!(split_url("http://example.com").is_err(), "plain http would put a session key in clear");
    }
}
