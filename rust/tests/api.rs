//! Ports of internal/api/api_test.go plus checks of the Go `net/http` behaviours the contract
//! depends on. The router is driven in-process with `tower::ServiceExt::oneshot`.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use quirenote::api::{self, AppState};
use quirenote::vault::Vault;
use quirenote::watch::Broker;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn state(v: Option<Arc<Vault>>) -> Arc<AppState> {
    AppState::new(v, Broker::new(), CancellationToken::new(), false)
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    ct: Option<&str>,
    body: &str,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(ct) = ct {
        req = req.header(header::CONTENT_TYPE, ct);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        headers,
    )
}

#[tokio::test]
async fn mutations_require_json() {
    let dir = tempfile::tempdir().unwrap();
    let v = Arc::new(Vault::open(dir.path()).unwrap());
    let app = api::router(state(Some(v)));

    // What a form/no-cors fetch from another page would do: rejected.
    for ct in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
    ] {
        let (code, _, _) = call(
            &app,
            Method::POST,
            "/api/notes/csrf.md",
            ct,
            r#"{"content":"x"}"#,
        )
        .await;
        assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE, "POST with {ct:?}");
        let (code, _, _) = call(&app, Method::POST, "/api/dirs/csrf", ct, "{}").await;
        assert_eq!(
            code,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "POST /api/dirs with {ct:?}"
        );
    }
    let (code, _, _) = call(
        &app,
        Method::POST,
        "/api/notes/ok.md",
        Some("application/json; charset=utf-8"),
        r#"{"content":"x"}"#,
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let (code, _, _) = call(
        &app,
        Method::POST,
        "/api/notes/ok.md",
        Some("application/json"),
        r#"{"content":"y"}"#,
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT);
    let (code, _, _) = call(&app, Method::GET, "/api/tree", None, "").await;
    assert_eq!(code, StatusCode::OK);
    // Go compares the media type case-sensitively.
    let (code, _, _) = call(
        &app,
        Method::POST,
        "/api/notes/u.md",
        Some("Application/JSON"),
        "{}",
    )
    .await;
    assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn first_run_setup() {
    let home = tempfile::tempdir().unwrap();
    let home_s = std::fs::canonicalize(home.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let st = state(None);
    let st2 = Arc::clone(&st);
    st.enable_setup(
        home_s.clone(),
        Box::new(move |res| {
            let v = Vault::open(&res.dir).map_err(|e| e.to_string())?;
            st2.set_vault(Arc::new(v));
            Ok(())
        }),
    );
    let app = api::router(st);
    let j = Some("application/json");

    // Without a vault, the vault endpoints point to the setup.
    let (code, body, _) = call(&app, Method::GET, "/api/tree", j, "").await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(body.contains(r#""setup":true"#), "{body}");
    let (code, body, _) = call(&app, Method::GET, "/api/setup", j, "").await;
    assert_eq!(code, StatusCode::OK);
    assert!(body.contains(r#""configured":false"#), "{body}");
    let (code, _, _) = call(
        &app,
        Method::POST,
        "/api/setup",
        j,
        r#"{"mode":"existing","path":"/"}"#,
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, body, _) = call(
        &app,
        Method::POST,
        "/api/setup",
        j,
        r#"{"mode":"new","path":"~/Notes"}"#,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (code, body, _) = call(&app, Method::GET, "/api/tree", j, "").await;
    assert_eq!(code, StatusCode::OK);
    assert!(body.contains("Welcome.md"), "{body}");
    // Once configured, the browser cannot point the server elsewhere.
    let (code, _, _) = call(
        &app,
        Method::POST,
        "/api/setup",
        j,
        r#"{"mode":"existing","path":"~"}"#,
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn net_http_behaviours() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "# A").unwrap();
    let v = Arc::new(Vault::open(dir.path()).unwrap());
    let app = api::router(state(Some(v)));
    let j = Some("application/json");

    // The mux redirects unclean paths and a subtree root without its slash.
    let (code, _, h) = call(&app, Method::GET, "/api/notes/x/../a.md", None, "").await;
    assert_eq!(code, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(h[header::LOCATION], "/api/notes/a.md");
    let (code, _, h) = call(&app, Method::GET, "/api/notes", None, "").await;
    assert_eq!(
        (code, h[header::LOCATION].to_str().unwrap()),
        (StatusCode::TEMPORARY_REDIRECT, "/api/notes/")
    );
    // An empty path is an invalid path, an encoded ".." too.
    let (code, body, _) = call(&app, Method::GET, "/api/notes/", None, "").await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(body, "{\"error\":\"invalid path\"}\n");
    let (code, _, _) = call(&app, Method::GET, "/api/notes/%2e%2e/x", None, "").await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    // Known path, wrong method: 405 + Allow, like the mux.
    let (code, body, h) = call(&app, Method::DELETE, "/api/tree", j, "").await;
    assert_eq!(code, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body, "Method Not Allowed\n");
    assert_eq!(h[header::ALLOW], "GET, HEAD");
    // Trailing garbage after the JSON value is ignored by Go's decoder.
    let (code, _, _) = call(
        &app,
        Method::PUT,
        "/api/notes/a.md",
        j,
        r#"{"content":"x"} trailing"#,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    // mtime must be a string.
    let (code, body, _) = call(
        &app,
        Method::PUT,
        "/api/notes/a.md",
        j,
        r#"{"content":"x","mtime":5}"#,
    )
    .await;
    assert_eq!(
        (code, body.as_str()),
        (StatusCode::BAD_REQUEST, "{\"error\":\"invalid JSON\"}\n")
    );
    // HTML-escaped JSON, like json.Encoder.
    let (_, body, _) = call(
        &app,
        Method::PUT,
        "/api/notes/a.md",
        j,
        r#"{"content":"<&>"}"#,
    )
    .await;
    assert!(body.contains(r#""content":"\u003c\u0026\u003e""#), "{body}");
    // HEAD works on GET routes.
    let (code, body, _) = call(&app, Method::HEAD, "/api/tree", None, "").await;
    assert_eq!((code, body.as_str()), (StatusCode::OK, ""));
    // Files carry the sandbox headers; a directory is a plain 404.
    let (code, _, h) = call(&app, Method::GET, "/api/files/a.md", None, "").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(h[header::CONTENT_SECURITY_POLICY], "sandbox");
    assert_eq!(h["x-content-type-options"], "nosniff");
    std::fs::create_dir(dir.path().join("d")).unwrap();
    let (code, body, _) = call(&app, Method::GET, "/api/files/d", None, "").await;
    assert_eq!(
        (code, body.as_str()),
        (StatusCode::NOT_FOUND, "404 page not found\n")
    );
}

#[tokio::test]
async fn ranges_and_images() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "0123456789").unwrap();
    let v = Arc::new(Vault::open(dir.path()).unwrap());
    let app = api::router(state(Some(v)));

    let range = |r: &str| {
        Request::builder()
            .uri("/api/files/a.md")
            .header(header::RANGE, r.to_string())
            .body(Body::empty())
            .unwrap()
    };
    let res = app.clone().oneshot(range("bytes=2-4")).await.unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
    assert_eq!(
        &res.into_body().collect().await.unwrap().to_bytes()[..],
        b"234"
    );
    let res = app.clone().oneshot(range("bytes=-3")).await.unwrap();
    assert_eq!(
        &res.into_body().collect().await.unwrap().to_bytes()[..],
        b"789"
    );
    let res = app.clone().oneshot(range("bytes=50-")).await.unwrap();
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes */10");

    let j = Some("application/json");
    let (code, body, _) = call(
        &app,
        Method::POST,
        "/api/images",
        j,
        r#"{"note":"a.md","data":"%%%"}"#,
    )
    .await;
    assert_eq!(
        (code, body.as_str()),
        (
            StatusCode::BAD_REQUEST,
            "{\"error\":\"invalid base64 image\"}\n"
        )
    );
    let (code, _, _) = call(
        &app,
        Method::POST,
        "/api/images",
        j,
        r#"{"note":"a.md","data":"PHN2Zz4="}"#,
    )
    .await;
    assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let png = base64_png();
    let (code, body, _) = call(
        &app,
        Method::POST,
        "/api/images",
        j,
        &format!(r#"{{"note":"a.md","data":"{png}"}}"#),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    assert!(body.contains("Pasted image "), "{body}");
}

fn base64_png() -> String {
    use base64::Engine;
    let png: &[u8] = &[
        0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
        0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
    ];
    base64::engine::general_purpose::STANDARD.encode(png)
}
