// HTTP client of the Quirenote server: the same routes and JSON the Svelte app uses
// (web/src/lib/api.ts), so this GUI is a drop-in replacement for the browser.

use std::io::BufRead;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    #[serde(default)]
    pub children: Option<Vec<Entry>>,
}

impl Entry {
    pub fn kids(&self) -> &[Entry] {
        self.children.as_deref().unwrap_or(&[])
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Note {
    pub path: String,
    pub content: String,
    /// UnixNano as a string: opaque to the client, only sent back to the server.
    pub mtime: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VaultEvent {
    pub path: String,
    pub op: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Part {
    pub text: String,
    #[serde(default)]
    pub hit: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SearchResult {
    pub path: String,
    pub line: usize,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RelinkPlan {
    pub files: Vec<String>,
    pub ambiguous: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RelinkDone {
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct SetupStatus {
    pub configured: bool,
    #[serde(default)]
    pub allowed: bool,
    #[serde(default)]
    pub suggested: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SetupDone {
    pub dir: String,
    pub obsidian: bool,
}

#[derive(Debug, Clone)]
pub enum ApiError {
    /// 409 on create/move: something already exists there.
    Exists(String),
    /// 409 on save: the note changed on disk; carries the version on disk.
    Conflict(Note),
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Exists(m) | ApiError::Other(m) => f.write_str(m),
            ApiError::Conflict(_) => f.write_str("file was modified on disk"),
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// encodeURIComponent: everything but A-Z a-z 0-9 - _ . ! ~ * ' ( )
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

pub fn component(s: &str) -> String {
    utf8_percent_encode(s, COMPONENT).to_string()
}

/// A vault path in a URL: each segment encoded, the slashes kept.
pub fn enc(path: &str) -> String {
    path.split('/').map(component).collect::<Vec<_>>().join("/")
}

#[derive(Clone)]
pub struct Api {
    base: String,
    agent: ureq::Agent,
}

struct Raw {
    status: u16,
    body: Vec<u8>,
}

fn error_text(raw: &Raw) -> String {
    #[derive(Deserialize)]
    struct E {
        error: Option<String>,
    }
    serde_json::from_slice::<E>(&raw.body)
        .ok()
        .and_then(|e| e.error)
        .unwrap_or_else(|| format!("HTTP {}", raw.status))
}

impl Api {
    pub fn new(base: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Api {
            base: base.trim_end_matches('/').to_string(),
            agent,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn file_url(&self, path: &str) -> String {
        format!("{}/api/files/{}", self.base, enc(path))
    }

    fn finish(resp: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Raw> {
        let mut resp = resp.map_err(|e| ApiError::Other(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(1 << 30)
            .read_to_vec()
            .map_err(|e| ApiError::Other(e.to_string()))?;
        Ok(Raw { status, body })
    }

    fn get(&self, url: &str) -> Result<Raw> {
        Self::finish(self.agent.get(url).call())
    }

    fn send(&self, method: &str, url: &str, body: &serde_json::Value) -> Result<Raw> {
        // The server requires JSON for everything that changes the vault (CSRF protection).
        let text = body.to_string();
        let req = match method {
            "PUT" => self.agent.put(url),
            _ => self.agent.post(url),
        };
        Self::finish(req.header("Content-Type", "application/json").send(text.as_bytes()))
    }

    fn json<T: for<'de> Deserialize<'de>>(raw: Raw) -> Result<T> {
        if !(200..300).contains(&raw.status) {
            return Err(ApiError::Other(error_text(&raw)));
        }
        serde_json::from_slice(&raw.body).map_err(|e| ApiError::Other(e.to_string()))
    }

    /// 409 means "already exists" for create/move/trash/images.
    fn json_exists<T: for<'de> Deserialize<'de>>(raw: Raw) -> Result<T> {
        if raw.status == 409 {
            return Err(ApiError::Exists(error_text(&raw)));
        }
        Self::json(raw)
    }

    pub fn tree(&self) -> Result<Vec<Entry>> {
        Self::json(self.get(&format!("{}/api/tree", self.base))?)
    }

    pub fn note(&self, path: &str) -> Result<Note> {
        Self::json(self.get(&format!("{}/api/notes/{}", self.base, enc(path)))?)
    }

    /// mtime = the version the editor read; "0" forces the write.
    pub fn save(&self, path: &str, content: &str, mtime: &str) -> Result<Note> {
        let raw = self.send(
            "PUT",
            &format!("{}/api/notes/{}", self.base, enc(path)),
            &serde_json::json!({ "content": content, "mtime": mtime }),
        )?;
        if raw.status == 409 {
            return match serde_json::from_slice::<Note>(&raw.body) {
                Ok(n) => Err(ApiError::Conflict(n)),
                Err(_) => Err(ApiError::Other(error_text(&raw))),
            };
        }
        Self::json(raw)
    }

    /// Creates a new note; fails with Exists if it already exists (the server never overwrites).
    pub fn create(&self, path: &str, content: &str) -> Result<Note> {
        Self::json_exists(self.send(
            "POST",
            &format!("{}/api/notes/{}", self.base, enc(path)),
            &serde_json::json!({ "content": content }),
        )?)
    }

    pub fn create_dir(&self, path: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct P {
            path: String,
        }
        let p: P = Self::json(self.send(
            "POST",
            &format!("{}/api/dirs/{}", self.base, enc(path)),
            &serde_json::json!({}),
        )?)?;
        Ok(p.path)
    }

    pub fn search(&self, q: &str) -> Result<Vec<SearchResult>> {
        Self::json(self.get(&format!("{}/api/search?q={}", self.base, component(q)))?)
    }

    pub fn backlinks(&self, note: &str) -> Result<Vec<SearchResult>> {
        Self::json(self.get(&format!("{}/api/backlinks?note={}", self.base, component(note)))?)
    }

    fn post_path(&self, url: &str, body: serde_json::Value) -> Result<String> {
        #[derive(Deserialize)]
        struct P {
            path: String,
        }
        let p: P = Self::json_exists(self.send("POST", url, &body)?)?;
        Ok(p.path)
    }

    /// Renames/moves a file or folder; Exists if the destination already exists.
    pub fn move_item(&self, from: &str, to: &str) -> Result<String> {
        self.post_path(
            &format!("{}/api/move", self.base),
            serde_json::json!({ "from": from, "to": to }),
        )
    }

    /// Moves to the vault trash (.trash); nothing is deleted.
    pub fn trash(&self, path: &str) -> Result<String> {
        self.post_path(&format!("{}/api/trash", self.base), serde_json::json!({ "path": path }))
    }

    /// Stores an image in the vault attachment folder; returns the created path.
    pub fn upload_image(&self, note: &str, data: &[u8]) -> Result<String> {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.post_path(
            &format!("{}/api/images", self.base),
            serde_json::json!({ "note": note, "data": b64 }),
        )
    }

    pub fn preview_relink(&self, from: &str, to: &str) -> Result<RelinkPlan> {
        Self::json(self.get(&format!(
            "{}/api/relink?from={}&to={}",
            self.base,
            component(from),
            component(to)
        ))?)
    }

    pub fn relink(&self, from: &str, to: &str) -> Result<RelinkDone> {
        Self::json_exists(self.send(
            "POST",
            &format!("{}/api/relink", self.base),
            &serde_json::json!({ "from": from, "to": to }),
        )?)
    }

    pub fn setup(&self) -> Result<SetupStatus> {
        Self::json(self.get(&format!("{}/api/setup", self.base))?)
    }

    pub fn run_setup(&self, mode: &str, path: &str) -> Result<SetupDone> {
        Self::json(self.send(
            "POST",
            &format!("{}/api/setup", self.base),
            &serde_json::json!({ "mode": mode, "path": path }),
        )?)
    }

    /// Bytes of a vault file (PDF, EPUB, image).
    pub fn file(&self, path: &str) -> Result<Vec<u8>> {
        let raw = self.get(&self.file_url(path))?;
        if raw.status != 200 {
            return Err(ApiError::Other(error_text(&raw)));
        }
        Ok(raw.body)
    }

    /// Bytes of any URL (images referenced by the preview).
    pub fn url_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let raw = self.get(url)?;
        if raw.status != 200 {
            return Err(ApiError::Other(error_text(&raw)));
        }
        Ok(raw.body)
    }

    /// Server-sent events of the vault (`/api/events`), until the connection drops.
    /// Calls `on_event` for every event; returns when the stream ends.
    pub fn events(&self, mut on_event: impl FnMut(VaultEvent)) -> Result<()> {
        // The stream is long-lived: no global timeout on this agent.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut resp = agent
            .get(&format!("{}/api/events", self.base))
            .call()
            .map_err(|e| ApiError::Other(e.to_string()))?;
        if resp.status() != 200 {
            return Err(ApiError::Other(format!("HTTP {}", resp.status())));
        }
        let reader = std::io::BufReader::new(resp.body_mut().as_reader());
        for line in reader.lines() {
            let line = line.map_err(|e| ApiError::Other(e.to_string()))?;
            if let Some(data) = line.strip_prefix("data:") {
                if let Ok(ev) = serde_json::from_str::<VaultEvent>(data.trim()) {
                    on_event(ev);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(component("a b/ç?&#"), "a%20b%2F%C3%A7%3F%26%23");
        assert_eq!(component("it's (ok)!*~"), "it's%20(ok)!*~");
        assert_eq!(enc("Pasta A/nota #1.md"), "Pasta%20A/nota%20%231.md");
    }
}
