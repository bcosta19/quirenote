//! The HTTP API (port of internal/api): same routes, status codes, JSON and headers as the Go
//! server. Where Go's `net/http` behaviour is part of the observable contract (mux redirects,
//! 405 bodies, `ServeContent` ranges) it is reproduced here on purpose.

mod content;
mod json;
mod mime_builtin;
mod spa;

use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use base64::Engine;
use futures_util::stream;
use tokio_util::sync::CancellationToken;

use crate::relink;
use crate::search;
use crate::setup;
use crate::vault::{self, MAX_ATTACHMENT, Vault};
use crate::watch::Broker;

use json::{
    CreateRequest, ImageRequest, MoveRequest, SaveRequest, SetupRequest, TrashRequest, decode,
    json_response, text_response,
};

/// Opens and saves the vault chosen on first run (it must call `AppState::set_vault`).
pub type ApplyFn = dyn Fn(setup::Prepared) -> Result<(), String> + Send + Sync;

struct SetupCfg {
    home: String,
    apply: Box<ApplyFn>,
}

pub struct AppState {
    /// `None` until a vault is configured (first run).
    vault: RwLock<Option<Arc<Vault>>>,
    broker: Broker,
    has_static: bool,
    setup_cfg: RwLock<Option<Arc<SetupCfg>>>,
    setup_mu: tokio::sync::Mutex<()>,
    shutdown: CancellationToken,
    debug: bool,
}

impl AppState {
    /// `vault` may be `None`: the API then only answers the first-run setup.
    pub fn new(
        vault: Option<Arc<Vault>>,
        broker: Broker,
        shutdown: CancellationToken,
        debug: bool,
    ) -> Arc<AppState> {
        Arc::new(AppState {
            vault: RwLock::new(vault),
            broker,
            has_static: spa::available(),
            setup_cfg: RwLock::new(None),
            setup_mu: tokio::sync::Mutex::new(()),
            shutdown,
            debug,
        })
    }

    /// Allows choosing the vault from the browser while none is configured.
    pub fn enable_setup(&self, home: String, apply: Box<ApplyFn>) {
        if let Ok(mut g) = self.setup_cfg.write() {
            *g = Some(Arc::new(SetupCfg { home, apply }));
        }
    }

    /// Starts serving `v`.
    pub fn set_vault(&self, v: Arc<Vault>) {
        if let Ok(mut g) = self.vault.write() {
            *g = Some(v);
        }
    }

    pub fn has_static(&self) -> bool {
        self.has_static
    }

    fn vault(&self) -> Option<Arc<Vault>> {
        self.vault.read().ok().and_then(|g| g.clone())
    }

    fn setup_cfg(&self) -> Option<Arc<SetupCfg>> {
        self.setup_cfg.read().ok().and_then(|g| g.clone())
    }
}

type S = State<Arc<AppState>>;

/// The `{path...}` of the Go routes: whatever follows `/api/<kind>/`, percent-decoded. Bytes
/// that are not UTF-8 make the path invalid (`None`), as `fs.ValidPath` does in Go.
pub struct Tail(Option<String>);

impl Tail {
    fn empty() -> Tail {
        Tail(Some(String::new()))
    }

    /// The decoded path, or the `invalid path` answer.
    fn get(self) -> Option<String> {
        self.0
    }
}

impl<St: Send + Sync> FromRequestParts<St> for Tail {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &St) -> Result<Self, Self::Rejection> {
        let mut it = parts.uri.path().splitn(4, '/');
        let raw = it.nth(3).unwrap_or("");
        let bytes: Vec<u8> = percent_encoding::percent_decode_str(raw).collect();
        Ok(Tail(String::from_utf8(bytes).ok()))
    }
}

/// The vault for a handler. `require_vault` guarantees one exists on every `/api/` route
/// except setup and events, so the `None` arm is only a defensive answer.
macro_rules! vault_or_409 {
    ($s:expr) => {
        match $s.vault() {
            Some(v) => v,
            None => return no_vault(),
        }
    };
}

fn no_vault() -> Response {
    json_response(
        StatusCode::CONFLICT,
        &serde_json::json!({"error": "no vault configured yet", "setup": true}),
    )
}

/// Wraps a route so that a method it does not handle is answered like Go's mux does: a GET or
/// HEAD falls through to the SPA (`GET /` matches every path), anything else gets
/// `405 Method Not Allowed` with the `Allow` list of the patterns that match the path.
fn route(
    methods: &[&str],
    r: axum::routing::MethodRouter<Arc<AppState>>,
    has_static: bool,
) -> axum::routing::MethodRouter<Arc<AppState>> {
    let mut allow: Vec<&str> = methods.to_vec();
    if has_static {
        allow.push("GET");
    }
    if allow.contains(&"GET") {
        allow.push("HEAD");
    }
    allow.sort_unstable();
    allow.dedup();
    let allow = allow.join(", ");
    r.fallback(move |req: Request| {
        let allow = allow.clone();
        async move {
            let m = req.method();
            if has_static && (m == Method::GET || m == Method::HEAD) {
                return spa::fallback(req).await;
            }
            method_not_allowed(&allow)
        }
    })
}

fn method_not_allowed(allow: &str) -> Response {
    let mut res = text_response(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed\n");
    if let Ok(v) = HeaderValue::from_str(allow) {
        res.headers_mut().insert(header::ALLOW, v);
    }
    res
}

pub fn router(state: Arc<AppState>) -> Router {
    let st = state.has_static;
    let notes = |r| route(&["GET", "POST", "PUT"], r, st);
    let mut inner = Router::new()
        .route("/api/tree", route(&["GET"], get(tree), st))
        .route(
            "/api/notes/",
            notes(
                get(read_note_empty)
                    .put(save_note_empty)
                    .post(create_note_empty),
            ),
        )
        .route(
            "/api/notes/{*path}",
            notes(get(read_note).put(save_note).post(create_note)),
        )
        .route("/api/files/", route(&["GET"], get(file_empty), st))
        .route("/api/files/{*path}", route(&["GET"], get(file), st))
        .route("/api/dirs/", route(&["POST"], post(mkdir_empty), st))
        .route("/api/dirs/{*path}", route(&["POST"], post(mkdir), st))
        .route("/api/move", route(&["POST"], post(move_file), st))
        .route("/api/trash", route(&["POST"], post(trash), st))
        .route(
            "/api/relink",
            route(&["GET", "POST"], get(relink_preview).post(relink_run), st),
        )
        .route("/api/images", route(&["POST"], post(save_image), st))
        .route("/api/events", route(&["GET"], get(events), st))
        .route("/api/search", route(&["GET"], get(search_notes), st))
        .route("/api/backlinks", route(&["GET"], get(backlinks), st))
        .route(
            "/api/setup",
            route(&["GET", "POST"], get(setup_status).post(setup_vault), st),
        );
    inner = if st {
        inner.fallback(static_fallback)
    } else {
        inner.fallback(not_found)
    };
    let inner = inner.with_state(Arc::clone(&state));

    // Same order as Go: logRequests(requireJSON(requireVault(mux))). The layers wrap the whole
    // inner router (as the fallback of the outer one), so they run before routing.
    Router::new()
        .fallback_service(inner)
        .layer(middleware::map_response(normalize_allow))
        .layer(middleware::from_fn(mux_redirects))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_vault,
        ))
        .layer(middleware::from_fn(require_json))
        .layer(middleware::from_fn_with_state(state, log_requests))
        .layer(middleware::from_fn(require_host))
}

/// The SPA fallback, except for the subtree roots (`/api/notes`, ...) where the mux lists the
/// methods of the subtree in its 405.
async fn static_fallback(req: Request) -> Response {
    let m = req.method();
    if m != Method::GET && m != Method::HEAD {
        match req.uri().path() {
            "/api/notes" => return method_not_allowed("GET, HEAD, POST, PUT"),
            "/api/dirs" => return method_not_allowed("GET, HEAD, POST"),
            _ => {}
        }
    }
    spa::fallback(req).await
}

/// Axum adds its own `Allow` to anything routed through a method router; keep only the Go-style
/// one on 405 and none elsewhere.
async fn normalize_allow(mut res: Response) -> Response {
    let first = res.headers().get(header::ALLOW).cloned();
    res.headers_mut().remove(header::ALLOW);
    if res.status() == StatusCode::METHOD_NOT_ALLOWED
        && let Some(v) = first
    {
        res.headers_mut().insert(header::ALLOW, v);
    }
    res
}

/// `net/http` refuses an HTTP/1.1 request without a Host header.
async fn require_host(req: Request, next: Next) -> Response {
    if req.version() == axum::http::Version::HTTP_11
        && !req.headers().contains_key(header::HOST)
        && req.uri().authority().is_none()
    {
        let mut res = text_response(
            StatusCode::BAD_REQUEST,
            "400 Bad Request: missing required Host header",
        );
        res.headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
        return res;
    }
    next.run(req).await
}

async fn not_found() -> Response {
    text_response(StatusCode::NOT_FOUND, "404 page not found\n")
}

// ---- middleware --------------------------------------------------------------------

async fn log_requests(State(s): S, req: Request, next: Next) -> Response {
    if !s.debug {
        return next.run(req).await;
    }
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let start = std::time::Instant::now();
    let res = next.run(req).await;
    if path != "/api/events" {
        eprintln!(
            "{} DEBUG http method={method} path={path} dur={:?}",
            crate::log_time(),
            start.elapsed()
        );
    }
    res
}

/// Rejects requests that change the vault without a JSON Content-Type. Forms and "no-cors"
/// fetches from other pages cannot send this header without a preflight, so this blocks CSRF.
async fn require_json(req: Request, next: Next) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let ct = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let mt = ct.split(';').next().unwrap_or("").trim();
        if mt != "application/json" {
            return json_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                &serde_json::json!({"error": "use Content-Type: application/json"}),
            );
        }
    }
    next.run(req).await
}

/// Answers the vault endpoints with 409 `{"setup": true}` until a vault is configured, so the
/// frontend shows the first-run screen.
async fn require_vault(State(s): S, req: Request, next: Next) -> Response {
    let p = req.uri().path();
    if p.starts_with("/api/") && p != "/api/setup" && p != "/api/events" && s.vault().is_none() {
        return no_vault();
    }
    next.run(req).await
}

/// `path.Clean` for URL paths, as `net/http`'s `cleanPath`: keeps a trailing slash.
fn clean_url_path(p: &str) -> String {
    if p.is_empty() {
        return "/".into();
    }
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    let mut np = crate::golike::clean(&p);
    if p.ends_with('/') && np != "/" {
        if p.len() == np.len() + 1 && p.starts_with(&np) {
            np = p;
        } else {
            np.push('/');
        }
    }
    np
}

/// What `http.ServeMux` does before dispatching: redirect (307) to the cleaned path, and
/// from `/api/notes` to `/api/notes/` for the methods registered on the subtree.
async fn mux_redirects(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let cleaned = clean_url_path(path);
    let target = if cleaned != path {
        Some(cleaned)
    } else {
        let m = req.method();
        let get_like = m == Method::GET || m == Method::HEAD;
        match path {
            "/api/notes" if get_like || m == Method::PUT || m == Method::POST => {
                Some("/api/notes/".to_string())
            }
            "/api/files" if get_like => Some("/api/files/".to_string()),
            "/api/dirs" if m == Method::POST => Some("/api/dirs/".to_string()),
            _ => None,
        }
    };
    let Some(mut loc) = target else {
        return next.run(req).await;
    };
    if let Some(q) = req.uri().query() {
        loc.push('?');
        loc.push_str(q);
    }
    redirect(req.method(), &loc)
}

fn redirect(method: &Method, loc: &str) -> Response {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::TEMPORARY_REDIRECT;
    if let Ok(v) = HeaderValue::from_str(loc) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    if method == Method::GET || method == Method::HEAD {
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        if method == Method::GET {
            let esc = loc.replace('&', "&amp;").replace('"', "&#34;");
            *res.body_mut() = Body::from(format!("<a href=\"{esc}\">Temporary Redirect</a>.\n\n"));
        }
    }
    res
}

// ---- errors ------------------------------------------------------------------------

fn error_json(status: StatusCode, msg: &str) -> Response {
    json_response(status, &serde_json::json!({ "error": msg }))
}

fn write_err(err: &vault::Error) -> Response {
    let status = match err {
        e if e.is_not_exist() => StatusCode::NOT_FOUND,
        vault::Error::InvalidPath | vault::Error::NotNote | vault::Error::BadName => {
            StatusCode::BAD_REQUEST
        }
        // os.Root returns its own error for paths that escape the vault.
        e if e.is_escape() => StatusCode::BAD_REQUEST,
        e => {
            eprintln!("{} ERROR api err=\"{e}\"", crate::log_time());
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    error_json(status, &err.to_string())
}

fn write_search_err(err: &search::Error) -> Response {
    match err {
        search::Error::NoRipgrep => error_json(StatusCode::SERVICE_UNAVAILABLE, &err.to_string()),
        search::Error::Other(m) => {
            eprintln!("{} ERROR api err=\"{m}\"", crate::log_time());
            error_json(StatusCode::INTERNAL_SERVER_ERROR, m)
        }
    }
}

fn write_relink_err(err: &relink::Error) -> Response {
    match err {
        relink::Error::TypeChanged
        | relink::Error::NotRenamed
        | relink::Error::Vault(vault::Error::BadName) => {
            error_json(StatusCode::BAD_REQUEST, &err.to_string())
        }
        relink::Error::Search(e) => write_search_err(e),
        relink::Error::Vault(e) => write_err(e),
    }
}

fn invalid_json() -> Response {
    error_json(StatusCode::BAD_REQUEST, "invalid JSON")
}

enum BodyError {
    TooLarge,
    Read,
}

/// Reads at most `limit` bytes of the body (Go's `http.MaxBytesReader`).
async fn read_body(req: Request, limit: usize) -> Result<Bytes, BodyError> {
    use http_body_util::BodyExt;
    let mut body = req.into_body();
    let mut buf = bytes::BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| BodyError::Read)?;
        if let Some(data) = frame.data_ref() {
            if buf.len() + data.len() > limit {
                return Err(BodyError::TooLarge);
            }
            buf.extend_from_slice(data);
        }
    }
    Ok(buf.freeze())
}

/// Reads and decodes a JSON request body, answering 400 `invalid JSON` like the Go handlers.
async fn json_body<T: serde::de::DeserializeOwned + Default>(
    req: Request,
    limit: usize,
) -> Result<T, Response> {
    let bytes = read_body(req, limit).await.map_err(|_| invalid_json())?;
    decode(&bytes).ok_or_else(invalid_json)
}

// ---- query string ------------------------------------------------------------------

/// `r.URL.Query().Get(key)`: first value; pairs with bad escapes or `;` are ignored.
fn query_get(req_query: Option<&str>, key: &str) -> String {
    for pair in req_query.unwrap_or("").split('&') {
        if pair.is_empty() || pair.contains(';') {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (Some(k), Some(v)) = (query_unescape(k), query_unescape(v)) else {
            continue;
        };
        if k == key {
            return v;
        }
    }
    String::new()
}

fn query_unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = b.get(i + 1..i + 3)?;
                let v = u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?;
                out.push(v);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

// ---- first-run setup ---------------------------------------------------------------

async fn setup_status(State(s): S) -> Response {
    if s.vault().is_some() {
        return json_response(StatusCode::OK, &serde_json::json!({"configured": true}));
    }
    json_response(
        StatusCode::OK,
        &serde_json::json!({
            "configured": false,
            "allowed": s.setup_cfg().is_some(),
            "suggested": setup::DEFAULT_PATH,
        }),
    )
}

/// Picks the vault on first run. Once a vault is configured it is refused: changing it is
/// done from the terminal (`quirenote init`), so a browser cannot point the server at other
/// folders of the machine.
async fn setup_vault(State(s): S, req: Request) -> Response {
    let _guard = s.setup_mu.lock().await;
    let cfg = s.setup_cfg();
    let (true, Some(cfg)) = (s.vault().is_none(), cfg) else {
        return error_json(
            StatusCode::FORBIDDEN,
            "the vault is already configured; change it with `quirenote init <folder>`",
        );
    };
    let body: SetupRequest = match json_body(req, 64 << 10).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let (mode, path) = (body.mode.unwrap_or_default(), body.path.unwrap_or_default());
    let home = cfg.home.clone();
    let prepared = tokio::task::spawn_blocking(move || setup::prepare(&mode, &path, &home)).await;
    let res = match prepared {
        Ok(Ok(r)) => r,
        Ok(Err(m)) => return error_json(StatusCode::BAD_REQUEST, &m),
        Err(e) => return error_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let reply = json_response(StatusCode::OK, &res);
    match (cfg.apply)(res) {
        Ok(()) => reply,
        Err(m) => {
            eprintln!("{} ERROR api err=\"{m}\"", crate::log_time());
            error_json(StatusCode::INTERNAL_SERVER_ERROR, &m)
        }
    }
}

// ---- notes -------------------------------------------------------------------------

async fn tree(State(s): S) -> Response {
    let v = vault_or_409!(s);
    match v.tree().await {
        Ok(entries) => json_response(StatusCode::OK, &entries),
        Err(e) => write_err(&e),
    }
}

async fn read_note_empty(state: S) -> Response {
    read_note(state, Tail::empty()).await
}

async fn read_note(State(s): S, tail: Tail) -> Response {
    let v = vault_or_409!(s);
    let Some(path) = tail.get() else {
        return write_err(&vault::Error::InvalidPath);
    };
    match v.read(&path).await {
        Ok(note) => json_response(StatusCode::OK, &note),
        Err(e) => write_err(&e),
    }
}

async fn save_note_empty(state: S, req: Request) -> Response {
    save_note(state, Tail::empty(), req).await
}

async fn save_note(State(s): S, tail: Tail, req: Request) -> Response {
    let v = vault_or_409!(s);
    let body: SaveRequest = match json_body(req, 10 << 20).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(path) = tail.get() else {
        return write_err(&vault::Error::InvalidPath);
    };
    match v
        .save(&path, &body.content.unwrap_or_default(), body.mtime)
        .await
    {
        Ok(note) => json_response(StatusCode::OK, &note),
        // Return the version on disk so the frontend can decide (reload or overwrite).
        Err(vault::Error::Conflict(current)) => json_response(StatusCode::CONFLICT, &*current),
        Err(e) => write_err(&e),
    }
}

async fn create_note_empty(state: S, req: Request) -> Response {
    create_note(state, Tail::empty(), req).await
}

/// Creates a new note; 409 if it already exists (never overwrites).
async fn create_note(State(s): S, tail: Tail, req: Request) -> Response {
    let v = vault_or_409!(s);
    let body: CreateRequest = match json_body(req, 10 << 20).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(path) = tail.get() else {
        return write_err(&vault::Error::InvalidPath);
    };
    match v.create(&path, &body.content.unwrap_or_default()).await {
        Ok(note) => json_response(StatusCode::CREATED, &note),
        Err(e @ vault::Error::Exists) => error_json(StatusCode::CONFLICT, &e.to_string()),
        Err(e) => write_err(&e),
    }
}

/// Renames/moves a file or folder; 409 if the destination already exists.
async fn move_file(State(s): S, req: Request) -> Response {
    let v = vault_or_409!(s);
    let body: MoveRequest = match json_body(req, 64 << 10).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let (from, to) = (body.from.unwrap_or_default(), body.to.unwrap_or_default());
    match v.move_to(&from, &to).await {
        Ok(p) => json_response(StatusCode::OK, &serde_json::json!({ "path": p })),
        Err(e @ vault::Error::Exists) => error_json(StatusCode::CONFLICT, &e.to_string()),
        Err(e) => write_err(&e),
    }
}

/// Tells which notes would have their `[[links]]` updated (nothing is written).
async fn relink_preview(State(s): S, req: Request) -> Response {
    let v = vault_or_409!(s);
    let q = req.uri().query();
    let (from, to) = (query_get(q, "from"), query_get(q, "to"));
    match relink::preview(&v, &from, &to).await {
        Ok(plan) => json_response(StatusCode::OK, &plan),
        Err(e) => write_relink_err(&e),
    }
}

/// Updates the `[[links]]` in other notes after renaming `from` to `to`.
async fn relink_run(State(s): S, req: Request) -> Response {
    let v = vault_or_409!(s);
    let body: MoveRequest = match json_body(req, 64 << 10).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let (from, to) = (body.from.unwrap_or_default(), body.to.unwrap_or_default());
    match relink::run(&v, &from, &to).await {
        Ok(res) => json_response(StatusCode::OK, &res),
        Err(e) => write_relink_err(&e),
    }
}

/// Moves to the vault trash (`.trash`), deleting nothing.
async fn trash(State(s): S, req: Request) -> Response {
    let v = vault_or_409!(s);
    let body: TrashRequest = match json_body(req, 64 << 10).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    match v.trash(&body.path.unwrap_or_default()).await {
        Ok(p) => json_response(StatusCode::OK, &serde_json::json!({ "path": p })),
        Err(e) => write_err(&e),
    }
}

/// Stores an image pasted/dropped into the editor and returns its path.
async fn save_image(State(s): S, req: Request) -> Response {
    let v = vault_or_409!(s);
    let limit = MAX_ATTACHMENT * 4 / 3 + 4096;
    let bytes = match read_body(req, limit).await {
        Ok(b) => b,
        Err(BodyError::TooLarge) => {
            return error_json(StatusCode::PAYLOAD_TOO_LARGE, "image too large (max 20 MB)");
        }
        Err(BodyError::Read) => return invalid_json(),
    };
    let Some(body) = decode::<ImageRequest>(&bytes) else {
        return invalid_json();
    };
    let Ok(data) = base64::engine::general_purpose::STANDARD.decode(body.data.unwrap_or_default())
    else {
        return error_json(StatusCode::BAD_REQUEST, "invalid base64 image");
    };
    match v.save_image(&body.note.unwrap_or_default(), data).await {
        Ok(p) => json_response(StatusCode::CREATED, &serde_json::json!({ "path": p })),
        Err(e @ vault::Error::NotImage) => {
            error_json(StatusCode::UNSUPPORTED_MEDIA_TYPE, &e.to_string())
        }
        Err(e) => write_err(&e),
    }
}

async fn mkdir_empty(state: S) -> Response {
    mkdir(state, Tail::empty()).await
}

async fn mkdir(State(s): S, tail: Tail) -> Response {
    let v = vault_or_409!(s);
    let Some(path) = tail.get() else {
        return write_err(&vault::Error::InvalidPath);
    };
    match v.mkdir(&path).await {
        Ok(p) => json_response(StatusCode::CREATED, &serde_json::json!({ "path": p })),
        Err(e) => write_err(&e),
    }
}

// ---- files -------------------------------------------------------------------------

async fn file_empty(state: S, req: Request) -> Response {
    file(state, Tail::empty(), req).await
}

async fn file(State(s): S, tail: Tail, req: Request) -> Response {
    let v = vault_or_409!(s);
    let Some(path) = tail.get() else {
        return write_err(&vault::Error::InvalidPath);
    };
    let opened = match content::open(&v, &path).await {
        Ok(o) => o,
        Err(e) => return write_err(&e),
    };
    let Some(opened) = opened else {
        return not_found().await;
    };
    // Opened directly by URL, an SVG/HTML from the vault cannot run scripts on the app origin.
    // (PDF.js fetches and <img> are not affected by these headers.)
    let mut res = content::serve_file(req.headers(), req.method(), opened).await;
    res.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox"),
    );
    res.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

// ---- search ------------------------------------------------------------------------

async fn search_notes(State(s): S, req: Request) -> Response {
    let v = vault_or_409!(s);
    let q = query_get(req.uri().query(), "q");
    if q.len() > 200 {
        return error_json(StatusCode::BAD_REQUEST, "query too long");
    }
    match search::search(v.dir(), &q).await {
        Ok(res) => json_response(StatusCode::OK, &res),
        Err(e) => write_search_err(&e),
    }
}

/// Lists the snippets of other notes with a `[[link]]` to `?note=`.
async fn backlinks(State(s): S, req: Request) -> Response {
    let v = vault_or_409!(s);
    let note = query_get(req.uri().query(), "note");
    match search::backlinks(v.dir(), &note).await {
        Ok(res) => json_response(StatusCode::OK, &res),
        Err(e) => write_search_err(&e),
    }
}

// ---- events (SSE) ------------------------------------------------------------------

/// Keeps an SSE connection open and sends vault changes.
async fn events(State(s): S) -> Response {
    use tokio::sync::broadcast::error::RecvError;

    struct Sse {
        rx: tokio::sync::broadcast::Receiver<crate::watch::Event>,
        ping: tokio::time::Interval,
        cancel: CancellationToken,
        connected: bool,
    }

    let mut ping = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(30),
        Duration::from_secs(30),
    );
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let st = Sse {
        rx: s.broker.subscribe(),
        ping,
        cancel: s.shutdown.clone(),
        connected: false,
    };
    let body = stream::unfold(st, |mut st| async move {
        if !st.connected {
            st.connected = true;
            return Some((
                Ok::<_, std::convert::Infallible>(Bytes::from_static(b": connected\n\n")),
                st,
            ));
        }
        loop {
            tokio::select! {
                () = st.cancel.cancelled() => return None,
                _ = st.ping.tick() => {
                    return Some((Ok(Bytes::from_static(b": ping\n\n")), st));
                }
                r = st.rx.recv() => match r {
                    Ok(e) => {
                        let data = json::to_vec_compact(&e);
                        let mut out = Vec::with_capacity(data.len() + 8);
                        out.extend_from_slice(b"data: ");
                        out.extend_from_slice(&data);
                        out.extend_from_slice(b"\n\n");
                        return Some((Ok(Bytes::from(out)), st));
                    }
                    // A slow client loses events instead of blocking the watcher.
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return None,
                },
            }
        }
    });
    let mut res = Response::new(Body::from_stream(body));
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}
