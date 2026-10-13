//! JSON helpers that reproduce Go's `encoding/json` behaviour where the contract can see it.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

/// `json.Encoder` escapes `<`, `>`, `&`, U+2028 and U+2029 inside strings (HTML safety).
struct GoFormatter;

impl serde_json::ser::Formatter for GoFormatter {
    fn write_string_fragment<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        fragment: &str,
    ) -> std::io::Result<()> {
        let mut last = 0;
        for (i, c) in fragment.char_indices() {
            let rep = match c {
                '<' => "\\u003c",
                '>' => "\\u003e",
                '&' => "\\u0026",
                '\u{2028}' => "\\u2028",
                '\u{2029}' => "\\u2029",
                _ => continue,
            };
            w.write_all(&fragment.as_bytes()[last..i])?;
            w.write_all(rep.as_bytes())?;
            last = i + c.len_utf8();
        }
        w.write_all(&fragment.as_bytes()[last..])
    }
}

/// `json.Marshal`.
pub fn to_vec_compact<T: Serialize>(v: &T) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, GoFormatter);
    if v.serialize(&mut ser).is_err() {
        return b"null".to_vec();
    }
    buf
}

/// `json.NewEncoder(w).Encode(v)`: the value followed by a newline.
pub fn to_vec<T: Serialize>(v: &T) -> Vec<u8> {
    let mut buf = to_vec_compact(v);
    buf.push(b'\n');
    buf
}

pub fn json_response<T: Serialize>(status: StatusCode, v: &T) -> Response {
    let mut res = Response::new(Body::from(to_vec(v)));
    *res.status_mut() = status;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    res
}

/// `http.Error`-style plain text answer.
pub fn text_response(status: StatusCode, body: &str) -> Response {
    let mut res = Response::new(Body::from(body.to_string()));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// `json.NewDecoder(r).Decode(&v)`: decodes the first JSON value and ignores whatever
/// follows it. `None` for an empty or invalid body.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    serde_json::Deserializer::from_slice(bytes)
        .into_iter::<T>()
        .next()?
        .ok()
}

/// A number carried in a JSON string (Go's `json:",string"`); `null` and a missing key are 0.
pub fn de_i64_string<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    match Option::<String>::deserialize(d)? {
        None => Ok(0),
        Some(s) => s.parse::<i64>().map_err(serde::de::Error::custom),
    }
}
