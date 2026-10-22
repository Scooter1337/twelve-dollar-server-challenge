mod db;
mod lufp;
mod net;
mod uring;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    borrow::Cow,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub fn number(n: u64, out: &mut Vec<u8>) {
    out.extend_from_slice(itoa::Buffer::new().format(n).as_bytes());
}
// Vectorize the common ASCII case without ever reading beyond the provided slice.
#[inline]
fn plain16(b: &[u8]) -> bool {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::*;
        let v = _mm_loadu_si128(b.as_ptr().cast());
        let bad = _mm_or_si128(
            _mm_or_si128(
                _mm_cmpeq_epi8(v, _mm_set1_epi8(34)),
                _mm_cmpeq_epi8(v, _mm_set1_epi8(92)),
            ),
            _mm_cmpeq_epi8(_mm_min_epu8(v, _mm_set1_epi8(31)), v),
        );
        _mm_movemask_epi8(bad) == 0
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        use std::arch::aarch64::*;
        let v = vld1q_u8(b.as_ptr());
        vmaxvq_u8(vorrq_u8(
            vorrq_u8(vceqq_u8(v, vdupq_n_u8(34)), vceqq_u8(v, vdupq_n_u8(92))),
            vcltq_u8(v, vdupq_n_u8(32)),
        )) == 0
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        b[..16].iter().all(|&c| c >= 32 && c != b'"' && c != b'\\')
    }
}
pub fn json_string(b: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    let mut i = 0;
    let mut start = 0;
    while i < b.len() {
        if i + 16 <= b.len() && plain16(&b[i..]) {
            i += 16;
            continue;
        }
        let c = b[i];
        if c < 32 || c == b'"' || c == b'\\' {
            out.extend_from_slice(&b[start..i]);
            match c {
                b'"' => out.extend_from_slice(b"\\\""),
                b'\\' => out.extend_from_slice(b"\\\\"),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                b'\t' => out.extend_from_slice(b"\\t"),
                _ => {
                    const HEX: &[u8] = b"0123456789abcdef";
                    out.extend_from_slice(&[
                        b'\\',
                        b'u',
                        b'0',
                        b'0',
                        HEX[(c >> 4) as usize],
                        HEX[(c & 15) as usize],
                    ]);
                }
            }
            start = i + 1;
        }
        i += 1;
    }
    out.extend_from_slice(&b[start..]);
    out.push(b'"');
}
pub fn error(out: &mut Vec<u8>, status: u16, msg: &str) -> u16 {
    out.clear();
    out.extend_from_slice(b"{\"error\":");
    json_string(msg.as_bytes(), out);
    out.push(b'}');
    status
}
fn positive(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok().filter(|v| *v > 0)
}
#[cold]
fn post_error(path: &str, status: u16, message: &str, out: &mut Vec<u8>) -> u16 {
    // Nested paths are unknown regardless of authentication. Keep this check
    // on error branches so successful requests retain their parsing sequence.
    if path
        .strip_prefix("/posts/")
        .and_then(|s| s.strip_suffix("/like"))
        .is_some_and(|s| s.contains('/'))
    {
        return error(out, 404, "not found");
    }
    error(out, status, message)
}
struct Auth {
    mac: Hmac<Sha256>,
    header: Vec<u8>,
    payload: Vec<u8>,
}
impl Auth {
    fn new(secret: &str) -> Self {
        Self {
            mac: Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap(),
            header: Vec::with_capacity(256),
            payload: Vec::with_capacity(256),
        }
    }
    fn user(&mut self, authorization: Option<&str>) -> Result<(i64, Cow<'_, str>), &'static str> {
        let token = authorization
            .and_then(|a| a.strip_prefix("Bearer "))
            .ok_or("missing bearer token")?;
        let bad = "invalid or expired token";
        if token.len() > 16384 {
            return Err(bad);
        }
        let mut pieces = token.split('.');
        let h = pieces.next().ok_or(bad)?;
        let p = pieces.next().ok_or(bad)?;
        let s = pieces.next().ok_or(bad)?;
        if pieces.next().is_some() {
            return Err(bad);
        }
        let mut sig = [0u8; 33];
        let n = URL_SAFE_NO_PAD.decode_slice(s, &mut sig).map_err(|_| bad)?;
        let mut mac = self.mac.clone();
        mac.update(&token.as_bytes()[..h.len() + 1 + p.len()]);
        mac.verify_slice(&sig[..n]).map_err(|_| bad)?;
        self.header.clear();
        URL_SAFE_NO_PAD
            .decode_vec(h, &mut self.header)
            .map_err(|_| bad)?;
        lufp::header(&self.header).map_err(|_| bad)?;
        self.payload.clear();
        URL_SAFE_NO_PAD
            .decode_vec(p, &mut self.payload)
            .map_err(|_| bad)?;
        // One validating pass retains decoded string claims, borrowing plain UTF-8.
        let claims = lufp::claims(&self.payload).map_err(|_| bad)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let exp = claims.exp.ok_or(bad)?;
        if exp <= now || claims.nbf.is_some_and(|n| n > now) {
            return Err(bad);
        }
        let payload_error = "invalid token payload";
        let sub = claims.sub.ok_or(payload_error)?;
        let id = positive(&sub).ok_or(payload_error)?;
        let name = claims.username.ok_or(payload_error)?;
        Ok((id, name))
    }
}
pub struct Frame<'a> {
    method: &'a str,
    path: &'a str,
    authorization: Option<&'a str>,
    body: Cow<'a, [u8]>,
    pub consumed: usize,
    pub close: bool,
}
pub enum Parsed<'a> {
    Incomplete { expect_continue: bool },
    Complete(Frame<'a>),
}
pub fn frame(input: &[u8]) -> Result<Parsed<'_>, ()> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);
    let end = match req.parse(input).map_err(|_| ())? {
        httparse::Status::Partial => {
            return if input.len() > 65536 {
                Err(())
            } else {
                Ok(Parsed::Incomplete {
                    expect_continue: false,
                })
            };
        }
        httparse::Status::Complete(n) => n,
    };
    let mut authorization = None;
    let mut length = None;
    let mut chunked = false;
    let mut expect_continue = false;
    let mut close = req.version != Some(1);
    for h in req.headers.iter() {
        if h.name.eq_ignore_ascii_case("authorization") {
            if authorization.is_some() {
                return Err(());
            }
            authorization = Some(std::str::from_utf8(h.value).map_err(|_| ())?);
        } else if h.name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(());
            }
            let s = std::str::from_utf8(h.value).map_err(|_| ())?.trim();
            if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
                return Err(());
            }
            length = Some(s.parse::<usize>().map_err(|_| ())?);
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked || !h.value.eq_ignore_ascii_case(b"chunked") {
                return Err(());
            }
            chunked = true;
        } else if h.name.eq_ignore_ascii_case("connection") {
            close |= h
                .value
                .split(|&c| c == b',')
                .any(|v| v.trim_ascii().eq_ignore_ascii_case(b"close"));
        } else if h.name.eq_ignore_ascii_case("expect") {
            if !h.value.eq_ignore_ascii_case(b"100-continue") {
                return Err(());
            }
            expect_continue = req.version == Some(1);
        }
    }
    if end > 65536 || length.is_some_and(|n| n > 1_048_576) {
        return Err(());
    }
    let (body, consumed) = if chunked {
        if length.is_some() {
            return Err(());
        }
        let Some((body, n)) = chunks(&input[end..])? else {
            return Ok(Parsed::Incomplete { expect_continue });
        };
        (Cow::Owned(body), end + n)
    } else {
        let length = length.unwrap_or(0);
        if input.len() < end + length {
            return Ok(Parsed::Incomplete { expect_continue });
        }
        (Cow::Borrowed(&input[end..end + length]), end + length)
    };
    Ok(Parsed::Complete(Frame {
        method: req.method.ok_or(())?,
        path: req.path.ok_or(())?,
        authorization,
        body,
        consumed,
        close,
    }))
}
fn chunks(input: &[u8]) -> Result<Option<(Vec<u8>, usize)>, ()> {
    let mut i = 0;
    let mut output = Vec::new();
    loop {
        let Some(n) = memchr::memmem::find(&input[i..], b"\r\n") else {
            return Ok(None);
        };
        let size = input[i..i + n].split(|&c| c == b';').next().ok_or(())?;
        if size.is_empty() || size.len() > 8 || !size.iter().all(|c| c.is_ascii_hexdigit()) {
            return Err(());
        }
        let size = usize::from_str_radix(std::str::from_utf8(size).map_err(|_| ())?, 16)
            .map_err(|_| ())?;
        i += n + 2;
        if size == 0 {
            loop {
                let Some(n) = memchr::memmem::find(&input[i..], b"\r\n") else {
                    return Ok(None);
                };
                i += n + 2;
                if n == 0 {
                    return Ok(Some((output, i)));
                }
                if i > 65536 + output.len() {
                    return Err(());
                }
            }
        }
        if size > 1_048_576 - output.len() {
            return Err(());
        }
        if input.len() < i + size + 2 {
            return Ok(None);
        }
        if &input[i + size..i + size + 2] != b"\r\n" {
            return Err(());
        }
        output.extend_from_slice(&input[i..i + size]);
        i += size + 2;
    }
}
struct App {
    db: db::Db,
    auth: Auth,
    start: Instant,
}
// This binary owns one SQLite connection. The io_uring workers hold the same
// Mutex<App> for the entire query/commit batch, so SQLite and Auth are never
// entered concurrently. Moving ownership between issuers preserves NOMUTEX.
unsafe impl Send for App {}
impl App {
    fn serve(&mut self, frame: &Frame<'_>, out: &mut Vec<u8>) -> u16 {
        let path = frame.path.split('?').next().unwrap_or(frame.path);
        if frame.method == "GET" {
            if path == "/feed" {
                return self.db.read(None, out);
            }
            if path == "/health" {
                return self.db.health(self.start.elapsed().as_secs(), out);
            }
            if let Some(s) = path.strip_prefix("/posts/") {
                if let Some(id) = positive(s) {
                    return self.db.read(Some(id), out);
                }
                if !s.contains('/') {
                    return error(out, 400, "invalid post id");
                }
            }
        }
        if frame.method == "POST"
            && (path == "/posts" || (path.starts_with("/posts/") && path.ends_with("/like")))
        {
            let (user, username) = match self.auth.user(frame.authorization) {
                Ok(v) => v,
                Err(msg) => return post_error(path, 401, msg, out),
            };
            if path == "/posts" {
                let value = match lufp::post_body(&frame.body) {
                    Ok(v) => v,
                    Err(_) => return error(out, 400, "malformed JSON body"),
                };
                let Some(body) = value else {
                    return error(out, 400, "body is required");
                };
                let body = body.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
                if body.is_empty() {
                    return error(out, 400, "body is required");
                }
                if body.chars().count() > 500 {
                    return error(out, 400, "body must be at most 500 characters");
                }
                return self.db.create(user, &username, body, out);
            }
            let Some(id) = path
                .strip_prefix("/posts/")
                .and_then(|s| s.strip_suffix("/like"))
                .and_then(positive)
            else {
                return post_error(path, 400, "invalid post id", out);
            };
            return self.db.like(user, id, out);
        }
        error(out, 404, "not found")
    }
}
fn main() {
    let mut options = net::Options::default();
    let mut epoll = false;
    let mut tuning = uring::Tuning::default();
    for arg in std::env::args().skip(1) {
        if arg == "--epoll" {
            epoll = true;
        } else if let Some(v) = arg.strip_prefix("--uring-batch=") {
            tuning.batch = v.parse().expect("completion batch size");
            assert!((1..=1024).contains(&tuning.batch));
        } else if let Some(v) = arg.strip_prefix("--uring-wait-us=") {
            tuning.wait_us = v.parse().expect("completion wait microseconds");
            assert!(tuning.wait_us <= 10000);
        } else if arg == "--no-group" {
            options.group = false;
        } else if let Some(v) = arg.strip_prefix("--spin-us=") {
            options.spin_us = v.parse().expect("spin microseconds");
        } else {
            panic!("unknown option: {arg}");
        }
    }
    assert!(epoll || options.spin_us == 0, "--spin-us requires --epoll");
    let path = std::env::var("SQLITE_PATH").expect("SQLITE_PATH required");
    let secret = std::env::var("JWT_SECRET").expect("JWT_SECRET required");
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = std::env::var("PORT").unwrap_or_else(|_| "80".into());
    let app = App {
        db: db::Db::open(&path, options.group),
        auth: Auth::new(&secret),
        start: Instant::now(),
    };
    if epoll {
        net::run(&format!("{host}:{port}"), app, options).expect("epoll server");
    } else {
        uring::run(&format!("{host}:{port}"), app, tuning).expect("io_uring server");
    }
}

#[cfg(test)]
include!("../tests/support/allocations.rs");
#[cfg(test)]
include!("../tests/support/snapshots.rs");

#[cfg(test)]
#[path = "../tests/support/serde_oracle.rs"]
mod serde_oracle;
