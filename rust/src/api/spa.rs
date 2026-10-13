//! The embedded frontend (`web/dist`) and the SPA fallback (port of `spa` + `web.Dist`).

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use rust_embed::RustEmbed;

use super::content;
use super::json::text_response;

/// The frontend build, embedded in the binary at compile time (`cd web && npm run build`).
#[derive(RustEmbed)]
#[folder = "../web/dist/"]
struct Dist;

/// Whether a frontend build is embedded (`index.html` exists).
pub fn available() -> bool {
    Dist::get("index.html").is_some()
}

fn is_dir(p: &str) -> bool {
    let prefix = format!("{p}/");
    Dist::iter().any(|f| f.starts_with(&prefix))
}

/// Serves the frontend build and returns `index.html` for unknown routes.
pub async fn fallback(req: Request) -> Response {
    let method = req.method().clone();
    if method != Method::GET && method != Method::HEAD {
        let mut res = text_response(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed\n");
        res.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return res;
    }
    let raw = req.uri().path();
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned();
    let trimmed = crate::golike::clean(&decoded);
    let mut p = trimmed.trim_start_matches('/').to_string();
    if p.is_empty() || p == "." {
        p = "index.html".into();
    }
    // FileServer redirects ".../index.html" to "./".
    if decoded.ends_with("/index.html") {
        return local_redirect("./", req.uri().query());
    }
    let name = match Dist::get(&p) {
        Some(_) => p.clone(),
        None => {
            if is_dir(&p) && !decoded.ends_with('/') {
                let base = crate::golike::base(&decoded);
                return local_redirect(&format!("{base}/"), req.uri().query());
            }
            "index.html".to_string()
        }
    };
    let Some(file) = Dist::get(&name) else {
        return text_response(StatusCode::NOT_FOUND, "404 page not found\n");
    };
    let ctype = content::content_type_for(&name, &file.data);
    content::serve_bytes(req.headers(), &method, file.data, &ctype, None)
}

fn local_redirect(target: &str, query: Option<&str>) -> Response {
    let mut loc = target.to_string();
    if let Some(q) = query {
        loc.push('?');
        loc.push_str(q);
    }
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::MOVED_PERMANENTLY;
    if let Ok(v) = HeaderValue::from_str(&loc) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res
}
