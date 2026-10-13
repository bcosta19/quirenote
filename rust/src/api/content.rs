//! Serving file bodies the way Go's `http.ServeContent` does: content type from the extension
//! (or sniffed), `Last-Modified`, `Accept-Ranges`, a single `Range`, 304 and 416.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use super::json::text_response;
use crate::golike;
use crate::vault::{self, Vault};

/// Go's `mime.TypeByExtension` table: the built-in entries plus, like Go on Unix, the local
/// shared-MIME database (`globs2`) or, failing that, the `mime.types` files. The first entry
/// seen for an extension wins; `text/*` types get `; charset=utf-8`.
struct MimeTable {
    exact: HashMap<String, String>,
    lower: HashMap<String, String>,
}

impl MimeTable {
    fn set(&mut self, ext: &str, mime: &str) {
        let mut m = mime.to_string();
        if m.starts_with("text/") && !m.contains("charset") {
            m.push_str("; charset=utf-8");
        }
        self.exact.insert(ext.to_string(), m.clone());
        self.lower.insert(ext.to_lowercase(), m);
    }

    fn load_globs(&mut self, path: &str) -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        for line in text.lines() {
            let f: Vec<&str> = line.split(':').collect();
            if f.len() < 3 || f[0].is_empty() || f[2].len() < 3 {
                continue;
            }
            if f[0].starts_with('#') || !f[2].starts_with("*.") {
                continue;
            }
            let ext = &f[2][1..];
            if ext.contains(['?', '*', '[']) || self.exact.contains_key(ext) {
                continue;
            }
            self.set(ext, f[1]);
        }
        true
    }

    fn load_types(&mut self, path: &str) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() <= 1 || f[0].starts_with('#') {
                continue;
            }
            for ext in &f[1..] {
                if ext.starts_with('#') {
                    break;
                }
                self.set(&format!(".{ext}"), f[0]);
            }
        }
    }
}

fn mime_table() -> &'static MimeTable {
    static T: OnceLock<MimeTable> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = MimeTable {
            exact: HashMap::new(),
            lower: HashMap::new(),
        };
        for (k, v) in super::mime_builtin::BUILTIN {
            t.exact.insert((*k).to_string(), (*v).to_string());
            t.lower.insert((*k).to_string(), (*v).to_string());
        }
        let globs = ["/usr/local/share/mime/globs2", "/usr/share/mime/globs2"];
        if !globs.iter().any(|g| t.load_globs(g)) {
            for f in [
                "/etc/mime.types",
                "/etc/apache2/mime.types",
                "/etc/apache/mime.types",
                "/etc/httpd/conf/mime.types",
            ] {
                t.load_types(f);
            }
        }
        t
    })
}

/// `mime.TypeByExtension`: exact match first, then case-insensitive.
fn type_by_extension(name: &str) -> Option<String> {
    let ext = golike::ext(name);
    let t = mime_table();
    t.exact
        .get(ext)
        .or_else(|| t.lower.get(&ext.to_lowercase()))
        .cloned()
}

/// A small `http.DetectContentType`: the signatures that matter for vault attachments, then
/// text vs binary.
fn sniff(data: &[u8]) -> &'static str {
    let d = &data[..data.len().min(512)];
    let sigs: &[(&[u8], &str)] = &[
        (b"%PDF-", "application/pdf"),
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xFF\xD8\xFF", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"BM", "image/bmp"),
        (b"PK\x03\x04", "application/zip"),
    ];
    for (sig, t) in sigs {
        if d.starts_with(sig) {
            return t;
        }
    }
    if d.len() >= 12 && d.starts_with(b"RIFF") && &d[8..12] == b"WEBP" {
        return "image/webp";
    }
    let binary = d.iter().any(|&b| {
        b <= 0x08 || b == 0x0B || (0x0E..=0x1A).contains(&b) || (0x1C..=0x1F).contains(&b)
    });
    if binary {
        "application/octet-stream"
    } else {
        "text/plain; charset=utf-8"
    }
}

pub fn content_type_for(name: &str, data: &[u8]) -> String {
    type_by_extension(name).unwrap_or_else(|| sniff(data).to_string())
}

/// A vault file ready to be served.
pub struct Opened {
    file: std::fs::File,
    size: u64,
    mtime: SystemTime,
    ctype: String,
}

/// Opens a vault file for reading. `None` when the path is a directory.
pub async fn open(v: &Arc<Vault>, p: &str) -> Result<Option<Opened>, vault::Error> {
    let v = Arc::clone(v);
    let p = p.to_string();
    tokio::task::spawn_blocking(move || -> Result<Option<Opened>, vault::Error> {
        let mut file = v.open_file_sync(&p)?.into_std();
        let md = file
            .metadata()
            .map_err(|e| vault::Error::io("statat", &p, e))?;
        if md.is_dir() {
            return Ok(None);
        }
        let name = golike::base(&p);
        let ctype = match type_by_extension(name) {
            Some(t) => t,
            None => {
                let mut head = vec![0u8; 512];
                let n = file.read(&mut head).unwrap_or(0);
                file.seek(SeekFrom::Start(0))
                    .map_err(|e| vault::Error::io("seek", &p, e))?;
                sniff(&head[..n]).to_string()
            }
        };
        Ok(Some(Opened {
            size: md.len(),
            mtime: md.modified().unwrap_or(UNIX_EPOCH),
            file,
            ctype,
        }))
    })
    .await
    .map_err(|e| vault::Error::Internal(e.to_string()))?
}

/// `http.ServeContent` for a vault file.
pub async fn serve_file(headers: &HeaderMap, method: &Method, o: Opened) -> Response {
    let Opened {
        file,
        size,
        mtime,
        ctype,
    } = o;
    match plan(headers, method, size, Some(mtime)) {
        Plan::Early(r) => r,
        Plan::Body {
            mut res,
            start,
            len,
        } => {
            set_ctype(&mut res, &ctype);
            if method == Method::HEAD {
                return res;
            }
            let mut f = tokio::fs::File::from_std(file);
            if start > 0 && f.seek(SeekFrom::Start(start)).await.is_err() {
                return text_response(StatusCode::INTERNAL_SERVER_ERROR, "seek failed\n");
            }
            *res.body_mut() =
                Body::from_stream(ReaderStream::with_capacity(f.take(len), 64 * 1024));
            res
        }
    }
}

/// `http.ServeContent` for in-memory content (the embedded frontend).
pub fn serve_bytes(
    headers: &HeaderMap,
    method: &Method,
    data: Cow<'static, [u8]>,
    ctype: &str,
    mtime: Option<SystemTime>,
) -> Response {
    let size = data.len() as u64;
    match plan(headers, method, size, mtime) {
        Plan::Early(r) => r,
        Plan::Body {
            mut res,
            start,
            len,
        } => {
            set_ctype(&mut res, ctype);
            if method == Method::HEAD {
                return res;
            }
            let (s, l) = (start as usize, len as usize);
            *res.body_mut() = match data {
                Cow::Borrowed(b) => Body::from(Bytes::from_static(&b[s..s + l])),
                Cow::Owned(v) => Body::from(Bytes::copy_from_slice(&v[s..s + l])),
            };
            res
        }
    }
}

fn set_ctype(res: &mut Response, ctype: &str) {
    if let Ok(v) = HeaderValue::from_str(ctype) {
        res.headers_mut().insert(header::CONTENT_TYPE, v);
    }
}

enum Plan {
    /// 304 / 416: nothing else to do.
    Early(Response),
    /// Headers are filled in; send `len` bytes from `start`.
    Body { res: Response, start: u64, len: u64 },
}

enum RangeSpec {
    Full,
    Range(u64, u64), // start, length
    Invalid,
    NoOverlap,
}

/// `parseRange` for a single range. Several ranges are answered with the whole content (the
/// RFC lets a server ignore `Range`); Go would answer `multipart/byteranges`.
fn parse_range(h: &str, size: u64) -> RangeSpec {
    let Some(spec) = h.strip_prefix("bytes=") else {
        return RangeSpec::Invalid;
    };
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut no_overlap = false;
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((a, b)) = part.split_once('-') else {
            return RangeSpec::Invalid;
        };
        let (a, b) = (a.trim(), b.trim());
        if a.is_empty() {
            // Suffix: the last N bytes.
            let Ok(n) = b.parse::<i64>() else {
                return RangeSpec::Invalid;
            };
            if b.is_empty() || n < 0 {
                return RangeSpec::Invalid;
            }
            let n = (n as u64).min(size);
            ranges.push((size - n, n));
        } else {
            let Ok(start) = a.parse::<u64>() else {
                return RangeSpec::Invalid;
            };
            if start >= size {
                no_overlap = true;
                continue;
            }
            let end = if b.is_empty() {
                size - 1
            } else {
                let Ok(end) = b.parse::<u64>() else {
                    return RangeSpec::Invalid;
                };
                if start > end {
                    return RangeSpec::Invalid;
                }
                end.min(size - 1)
            };
            ranges.push((start, end - start + 1));
        }
    }
    if no_overlap && ranges.is_empty() {
        return RangeSpec::NoOverlap;
    }
    match ranges.as_slice() {
        [(s, l)] => RangeSpec::Range(*s, *l),
        _ => RangeSpec::Full,
    }
}

fn plan(headers: &HeaderMap, method: &Method, size: u64, mtime: Option<SystemTime>) -> Plan {
    let mut res = Response::new(Body::empty());
    let h = res.headers_mut();
    let mtime = mtime.filter(|t| *t > UNIX_EPOCH);
    if let Some(t) = mtime
        && let Ok(v) = HeaderValue::from_str(&httpdate::fmt_http_date(t))
    {
        h.insert(header::LAST_MODIFIED, v);
    }

    // If-Modified-Since (GET/HEAD only).
    if let (Some(t), Some(ims)) = (
        mtime,
        headers
            .get(header::IF_MODIFIED_SINCE)
            .and_then(|v| v.to_str().ok()),
    ) && (method == Method::GET || method == Method::HEAD)
        && let Ok(since) = httpdate::parse_http_date(ims)
    {
        let truncated = UNIX_EPOCH
            + Duration::from_secs(
                t.duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            );
        if truncated <= since {
            *res.status_mut() = StatusCode::NOT_MODIFIED;
            return Plan::Early(res);
        }
    }

    res.headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let mut range_hdr = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|r| !r.is_empty());
    // If-Range: only honour the range if the validator still matches.
    if range_hdr.is_some()
        && let Some(ir) = headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok())
    {
        let still = match (mtime, httpdate::parse_http_date(ir)) {
            (Some(t), Ok(d)) => {
                t.duration_since(UNIX_EPOCH).map(|x| x.as_secs()).ok()
                    == d.duration_since(UNIX_EPOCH).map(|x| x.as_secs()).ok()
            }
            _ => false,
        };
        if !still {
            range_hdr = None;
        }
    }
    let spec = range_hdr.map_or(RangeSpec::Full, |r| parse_range(r, size));
    match spec {
        RangeSpec::Invalid | RangeSpec::NoOverlap => {
            let msg = if matches!(spec, RangeSpec::Invalid) {
                "invalid range\n"
            } else {
                "invalid range: failed to overlap\n"
            };
            let mut r = text_response(StatusCode::RANGE_NOT_SATISFIABLE, msg);
            // Go only sets Content-Range when no range overlaps the content.
            if matches!(spec, RangeSpec::NoOverlap)
                && let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}"))
            {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            Plan::Early(r)
        }
        RangeSpec::Range(start, len) => {
            *res.status_mut() = StatusCode::PARTIAL_CONTENT;
            let h = res.headers_mut();
            if let Ok(v) = HeaderValue::from_str(&format!(
                "bytes {start}-{}/{size}",
                start + len.saturating_sub(1)
            )) {
                h.insert(header::CONTENT_RANGE, v);
            }
            h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
            Plan::Body { res, start, len }
        }
        RangeSpec::Full => {
            res.headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
            Plan::Body {
                res,
                start: 0,
                len: size,
            }
        }
    }
}
