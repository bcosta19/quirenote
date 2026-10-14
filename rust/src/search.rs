//! Text searches over the vault notes using ripgrep (port of internal/search).
//!
//! Like the Go version, it shells out to `rg`: the regex semantics, the `--json` snippets
//! and the exact result set are then identical by construction, and the comparison measures
//! the servers, not two different search engines.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::golike;

#[derive(Debug)]
pub enum Error {
    NoRipgrep,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoRipgrep => f.write_str("ripgrep (rg) is not installed"),
            Error::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// A piece of the matched snippet; `hit` marks the matching text.
#[derive(Debug, Serialize, PartialEq, Eq, Clone)]
pub struct Part {
    pub text: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hit: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    /// Relative to the vault.
    pub path: String,
    pub line: i64,
    pub parts: Vec<Part>,
}

const PER_FILE: usize = 5;
/// Result limit; one extra is returned so the frontend knows the list was truncated.
pub const MAX_TOTAL: usize = 200;
/// Size of the snippet shown around the match.
const MAX_CHARS: usize = 160;
const SEARCH_FOR: Duration = Duration::from_secs(5);

/// Accented variants: searching "funcao" also finds "função".
const ACCENTS: &[(char, &str)] = &[
    ('a', "aáàâãä"),
    ('e', "eéèêë"),
    ('i', "iíìîï"),
    ('o', "oóòôõö"),
    ('u', "uúùûü"),
    ('c', "cç"),
    ('n', "nñ"),
];

/// Builds the ripgrep regex: literal text, accent-insensitive.
pub fn pattern(q: &str) -> String {
    let mut b = String::new();
    for r in q.to_lowercase().chars() {
        let base = fold(r);
        if let Some((_, v)) = ACCENTS.iter().find(|(c, _)| *c == base) {
            b.push('[');
            b.push_str(v);
            b.push_str(&v.to_uppercase());
            b.push(']');
        } else {
            b.push_str(&golike::quote_meta(&r.to_string()));
        }
    }
    b
}

fn fold(r: char) -> char {
    ACCENTS
        .iter()
        .find(|(_, v)| v.contains(r))
        .map_or(r, |(base, _)| *base)
}

/// UTF-8 text ("text") or, if the content is not valid UTF-8, the bytes in base64 ("bytes").
#[derive(Debug, Deserialize, Default)]
struct RgData {
    #[serde(default)]
    text: String,
    #[serde(default)]
    bytes: String,
}

impl RgData {
    fn into_bytes(self) -> Vec<u8> {
        if self.bytes.is_empty() {
            self.text.into_bytes()
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(self.bytes)
                .unwrap_or_default()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct RgSub {
    start: i64,
    end: i64,
}

#[derive(Debug, Deserialize, Default)]
struct RgInner {
    #[serde(default)]
    path: RgData,
    #[serde(default)]
    lines: RgData,
    #[serde(default)]
    line_number: i64,
    #[serde(default)]
    submatches: Vec<RgSub>,
}

#[derive(Debug, Deserialize)]
struct RgMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: RgInner,
}

fn look_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Looks for `q` in the `.md` files of `dir` (skipping hidden ones like `.obsidian`/`.trash`).
pub async fn search(dir: &Path, q: &str) -> Result<Vec<SearchResult>, Error> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    run(dir, &pattern(q)).await
}

/// Matches `[[links]]` to the note (path relative to the vault, with `.md`): by name or by
/// path, with or without `.md`, followed by `#section`, `|alias` (or `\|` inside tables)
/// or `]]`. It is approximate: it counts `[[Name]]` for every note with that name.
pub fn backlink_pattern(note: &str) -> String {
    // Notes are referenced without ".md" (optional); other files with their extension.
    let (full, md) = if golike::ext(note).eq_ignore_ascii_case(".md") {
        (&note[..note.len() - 3], r"(?:\.md)?")
    } else {
        (note, "")
    };
    let name = golike::base(full);
    let mut alts = golike::quote_meta(name);
    if full != name {
        alts.push('|');
        alts.push_str(&golike::quote_meta(full));
    }
    format!(r"\[\[(?:{alts}){md}(?:\\?[#|][^\]]*)?\]\]")
}

/// Lists the snippets of other notes that link to `note`.
pub async fn backlinks(dir: &Path, note: &str) -> Result<Vec<SearchResult>, Error> {
    let note = note.strip_prefix('/').unwrap_or(note);
    if note.is_empty() || note.starts_with("../") {
        return Ok(Vec::new());
    }
    let mut res = run(dir, &backlink_pattern(note)).await?;
    res.retain(|r| r.path != note); // links from the note to itself do not count
    Ok(res)
}

/// Lists ALL `.md` notes matching the pattern (without the search result limits). Unlike
/// `search`, it fails on timeout: its caller (link updating) must not act on a partial list.
pub async fn files(dir: &Path, pattern: &str) -> Result<Vec<String>, Error> {
    let rg = look_path("rg").ok_or(Error::NoRipgrep)?;
    let mut child = Command::new(rg)
        .args([
            "--files-with-matches",
            "--null",
            "--ignore-case",
            "--no-ignore",
            "--iglob",
            "*.md",
            "-e",
            pattern,
            "--",
            ".",
        ])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Other(e.to_string()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Other("no stdout".into()))?;
    let mut out = Vec::new();
    let run = async {
        stdout.read_to_end(&mut out).await?;
        child.wait().await
    };
    let status = match tokio::time::timeout(3 * SEARCH_FOR, run).await {
        Err(_) => {
            return Err(Error::Other(
                "the search took too long; nothing was changed".into(),
            ));
        }
        Ok(Err(e)) => return Err(Error::Other(e.to_string())),
        Ok(Ok(s)) => s,
    };
    match status.code() {
        Some(1) => return Ok(Vec::new()), // exit code 1 = no file matched
        Some(0) => {}
        _ => return Err(Error::Other(format!("rg: {status}"))),
    }
    Ok(out
        .split(|b| *b == 0)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let s = String::from_utf8_lossy(l);
            s.strip_prefix("./").unwrap_or(&s).to_string()
        })
        .collect())
}

/// Runs ripgrep with the pattern over the vault's `.md` notes and builds the snippets.
async fn run(dir: &Path, pattern: &str) -> Result<Vec<SearchResult>, Error> {
    let rg = look_path("rg").ok_or(Error::NoRipgrep)?;
    // No shell: the term is an argument after "-e" and is never interpreted.
    // --no-ignore: a .gitignore in the vault must not hide notes (hidden files stay excluded).
    let mut child = Command::new(rg)
        .args([
            "--json",
            "--ignore-case",
            "--no-ignore",
            "--max-count",
            "5",
            "--iglob",
            "*.md",
            "--max-filesize",
            "2M",
            "-e",
            pattern,
            "--",
            ".",
        ])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Other(e.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Other("no stdout".into()))?;

    let mut results: Vec<SearchResult> = Vec::new();
    let mut count: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut rd = BufReader::with_capacity(64 * 1024, stdout);
    let read_all = async {
        let mut raw = Vec::new();
        while results.len() <= MAX_TOTAL {
            raw.clear();
            // read_until, not lines(): very long JSON lines (a single-line note) must not
            // stop reading the remaining results.
            match rd.read_until(b'\n', &mut raw).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(m) = serde_json::from_slice::<RgMessage>(&raw) else {
                continue;
            };
            if m.kind != "match" {
                continue;
            }
            let line_number = m.data.line_number;
            let path_bytes = m.data.path.into_bytes();
            let path_s = String::from_utf8_lossy(&path_bytes);
            let p = path_s.strip_prefix("./").unwrap_or(&path_s).to_string();
            let c = count.entry(p.clone()).or_insert(0);
            if *c >= PER_FILE {
                continue;
            }
            *c += 1;
            let line = m.data.lines.into_bytes();
            results.push(SearchResult {
                path: p,
                line: line_number,
                parts: snippet(&line, &m.data.submatches),
            });
        }
    };
    let timed_out = tokio::time::timeout(SEARCH_FOR, read_all).await.is_err();
    // Enough results or end of output: stop rg (exit code 1 = "nothing found").
    let _ = child.start_kill();
    let _ = child.wait().await;
    if timed_out && results.is_empty() {
        return Err(Error::Other("the search took too long".into()));
    }
    Ok(results)
}

fn utf8_start(b: u8) -> bool {
    b & 0xC0 != 0x80
}

/// Cuts the line around the first match and marks the matches. rg offsets are in bytes;
/// invalid UTF-8 bytes become U+FFFD.
fn snippet(raw_line: &[u8], matches: &[RgSub]) -> Vec<Part> {
    let mut line = raw_line;
    while let [rest @ .., b'\r' | b'\n'] = line {
        line = rest;
    }
    let subs: Vec<(usize, usize)> = matches
        .iter()
        .filter(|s| s.start >= 0 && s.start <= s.end && s.end as usize <= line.len())
        .map(|s| (s.start as usize, s.end as usize))
        .collect();
    let fallback = || {
        vec![Part {
            text: clip(&golike::to_valid_utf8(line)),
            hit: false,
        }]
    };
    let Some(first) = subs.first() else {
        return fallback();
    };
    // Byte window around the first match, adjusted so it does not split UTF-8.
    let mut start = first.0.saturating_sub(MAX_CHARS / 2);
    let mut end = line.len().min(start + MAX_CHARS * 2);
    while start > 0 && !utf8_start(line[start]) {
        start -= 1;
    }
    while end < line.len() && !utf8_start(line[end]) {
        end += 1;
    }
    let mut parts: Vec<Part> = Vec::new();
    let mut pos = start;
    for &(s, e) in &subs {
        if s < pos || e > end {
            continue;
        }
        if s > pos {
            parts.push(Part {
                text: golike::to_valid_utf8(&line[pos..s]),
                hit: false,
            });
        }
        parts.push(Part {
            text: golike::to_valid_utf8(&line[s..e]),
            hit: true,
        });
        pos = e;
    }
    if pos < end {
        parts.push(Part {
            text: golike::to_valid_utf8(&line[pos..end]),
            hit: false,
        });
    }
    let Some(first_part) = parts.first_mut() else {
        return fallback();
    };
    if start > 0 {
        first_part.text = format!("…{}", first_part.text.trim_start_matches([' ', '\t']));
    }
    if end < line.len()
        && let Some(last) = parts.last_mut()
    {
        last.text.push('…');
    }
    parts
}

fn clip(s: &str) -> String {
    if s.chars().count() > MAX_CHARS * 2 {
        let mut t: String = s.chars().take(MAX_CHARS * 2).collect();
        t.push('…');
        t
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests;
