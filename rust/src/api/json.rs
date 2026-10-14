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
/// follows it. A JSON `null` leaves the zero value, like Go. `None` for an empty or invalid body.
pub fn decode<T: DeserializeOwned + Default>(bytes: &[u8]) -> Option<T> {
    let v = serde_json::Deserializer::from_slice(bytes)
        .into_iter::<Option<T>>()
        .next()?
        .ok()?;
    Some(v.unwrap_or_default())
}

/// Walks the keys of a JSON object matching them case-insensitively, like `encoding/json`
/// does for struct fields (`{"Content": ...}` fills `content`). Unknown keys are ignored.
fn visit_ci_map<'de, A, F>(mut map: A, keys: &[&'static str], mut on_key: F) -> Result<(), A::Error>
where
    A: serde::de::MapAccess<'de>,
    F: FnMut(&'static str, &mut A) -> Result<(), A::Error>,
{
    while let Some(k) = map.next_key::<std::borrow::Cow<'de, str>>()? {
        match keys.iter().find(|x| x.eq_ignore_ascii_case(&k)) {
            Some(name) => on_key(name, &mut map)?,
            None => {
                map.next_value::<serde::de::IgnoredAny>()?;
            }
        }
    }
    Ok(())
}

/// A request struct whose fields are all optional strings, decoded like Go would.
macro_rules! ci_struct {
    ($name:ident { $($field:ident),+ }) => {
        #[derive(Default)]
        pub struct $name {
            $(pub $field: Option<String>),+
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = $name;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str("a JSON object")
                    }
                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        map: A,
                    ) -> Result<$name, A::Error> {
                        let mut out = $name::default();
                        visit_ci_map(map, &[$(stringify!($field)),+], |name, m| {
                            match name {
                                $(stringify!($field) => out.$field = m.next_value()?,)+
                                _ => {}
                            }
                            Ok(())
                        })?;
                        Ok(out)
                    }
                }
                d.deserialize_map(V)
            }
        }
    };
}

ci_struct!(CreateRequest { content });
ci_struct!(MoveRequest { from, to });
ci_struct!(TrashRequest { path });
ci_struct!(ImageRequest { note, data });
ci_struct!(SetupRequest { mode, path });

/// `{"content": ..., "mtime": "123"}`; `mtime` is `json:",string"` in Go: a number inside a
/// JSON string (`null` and a missing key are 0, a bare number is an error).
#[derive(Default)]
pub struct SaveRequest {
    pub content: Option<String>,
    pub mtime: i64,
}

impl<'de> Deserialize<'de> for SaveRequest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = SaveRequest;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<SaveRequest, A::Error> {
                let mut out = SaveRequest::default();
                visit_ci_map(map, &["content", "mtime"], |name, m| {
                    if name == "content" {
                        out.content = m.next_value()?;
                    } else if let Some(s) = m.next_value::<Option<String>>()? {
                        out.mtime = s.parse::<i64>().map_err(serde::de::Error::custom)?;
                    }
                    Ok(())
                })?;
                Ok(out)
            }
        }
        d.deserialize_map(V)
    }
}
