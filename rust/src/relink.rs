//! Rewrites `[[links]]` in other notes when a file is renamed (port of internal/relink).

use std::sync::Arc;

use regex::Regex;
use serde::Serialize;

use crate::golike;
use crate::search;
use crate::vault::{self, Entry, Vault};

#[derive(Debug)]
pub enum Error {
    TypeChanged,
    NotRenamed,
    Vault(vault::Error),
    Search(search::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TypeChanged => f.write_str(
                "the extension changed between a note (.md) and another type; update the links by hand",
            ),
            Error::NotRenamed => f.write_str(
                "the file was not renamed (the source still exists or the destination does not)",
            ),
            Error::Vault(e) => e.fmt(f),
            Error::Search(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<vault::Error> for Error {
    fn from(e: vault::Error) -> Self {
        Error::Vault(e)
    }
}

impl From<search::Error> for Error {
    fn from(e: search::Error) -> Self {
        Error::Search(e)
    }
}

/// What would change. `ambiguous`: another file with the old name still exists, so
/// name-only links (`[[Name]]`) stay as they are (they may point to it).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Plan {
    pub files: Vec<String>,
    pub ambiguous: bool,
}

/// The updated notes and the skipped ones (changed during the operation).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RunResult {
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
    pub ambiguous: bool,
}

fn is_note(p: &str) -> bool {
    golike::ext(p).eq_ignore_ascii_case(".md")
}

/// The ways to reference the file in a `[[link]]`: name only and full path, both without
/// `.md` for notes (Obsidian omits it) and with the extension for other files.
fn forms(p: &str) -> (String, String) {
    let full = if is_note(p) {
        &p[..p.len() - golike::ext(p).len()]
    } else {
        p
    };
    (golike::base(full).to_string(), full.to_string())
}

/// Replaces links to `from` with links to `to` in `content`. It keeps `!`, `.md`, `#section`,
/// `|alias` and `\|`. Name-only links: with `name_links == false` they stay untouched;
/// otherwise they stay name-only if the new name is unique in the vault (`name_unique`), or
/// switch to the full path, like Obsidian does.
pub fn rewrite(
    content: &str,
    from: &str,
    to: &str,
    name_unique: bool,
    name_links: bool,
) -> (String, usize) {
    let (old_name, old_full) = forms(from);
    let (new_name, new_full) = forms(to);
    let mut alts = golike::quote_meta(&old_name);
    if old_full != old_name {
        alts.push('|');
        alts.push_str(&golike::quote_meta(&old_full));
    }
    let md = if is_note(from) { r"(\.md)?" } else { "()" }; // other files carry the extension
    let src = format!(r"(?i)(\[\[)({alts}){md}(\\?[#|][^\]\n]*)?(\]\])");
    let Ok(re) = Regex::new(&src) else {
        return (content.to_string(), 0);
    };
    let old_name_l = old_name.to_lowercase();
    let mut n = 0usize;
    let out = re.replace_all(content, |c: &regex::Captures<'_>| {
        let g = |i: usize| c.get(i).map_or("", |m| m.as_str());
        let by_name = g(2).to_lowercase() == old_name_l;
        if by_name && !name_links {
            return g(0).to_string();
        }
        let target = if by_name && name_unique {
            &new_name
        } else {
            &new_full
        };
        n += 1;
        format!("{}{}{}{}{}", g(1), target, g(3), g(4), g(5))
    });
    (out.into_owned(), n)
}

struct Change {
    path: String,
    content: String,
    mtime: i64,
}

fn count_name(entries: &[Entry], name: &str) -> usize {
    let name = name.to_lowercase();
    fn go(entries: &[Entry], name: &str) -> usize {
        entries
            .iter()
            .map(|e| {
                if e.dir {
                    go(&e.children, name)
                } else {
                    usize::from(e.name.to_lowercase() == name)
                }
            })
            .sum()
    }
    go(entries, &name)
}

/// Validates the request and computes the new content of each affected note, without writing.
async fn prepare(v: &Arc<Vault>, from: &str, to: &str) -> Result<(Vec<Change>, bool), Error> {
    // Only after the rename: rejects stale or forged requests.
    if v.exists(from).await || !v.exists(to).await {
        return Err(Error::NotRenamed);
    }
    if !vault::valid_name(to) {
        return Err(vault::Error::BadName.into());
    }
    if is_note(from) != is_note(to) {
        return Err(Error::TypeChanged);
    }
    // A subprocess matching file contents can omit an atomically replaced note. Discover
    // and read all notes in one writer-queue job, then retain optimistic saves below.
    let (tree, notes) = v.notes_snapshot().await?;
    // Another file with the old name? (A case-only rename, Lexer -> lexer, counts itself.)
    let same = usize::from(golike::base(from).to_lowercase() == golike::base(to).to_lowercase());
    let ambiguous = count_name(&tree, golike::base(from)) > same;
    let unique = count_name(&tree, golike::base(to)) <= 1;

    let (from_o, to_o) = (from.to_string(), to.to_string());
    let changes = notes
        .into_iter()
        .filter_map(|note| {
            let (content, _) = rewrite(&note.content, &from_o, &to_o, unique, !ambiguous);
            // Nothing actually changed (e.g. only ambiguous name-only links).
            (content != note.content).then_some(Change {
                path: note.path,
                content,
                mtime: note.mtime,
            })
        })
        .collect();
    Ok((changes, ambiguous))
}

/// Tells which notes would change, writing nothing.
pub async fn preview(v: &Arc<Vault>, from: &str, to: &str) -> Result<Plan, Error> {
    let (changes, ambiguous) = prepare(v, from, to).await?;
    Ok(Plan {
        files: changes.into_iter().map(|c| c.path).collect(),
        ambiguous,
    })
}

/// Updates the vault notes that link to `from`, after `from` became `to`.
pub async fn run(v: &Arc<Vault>, from: &str, to: &str) -> Result<RunResult, Error> {
    let (changes, ambiguous) = prepare(v, from, to).await?;
    let mut res = RunResult {
        updated: Vec::new(),
        skipped: Vec::new(),
        ambiguous,
    };
    for c in changes {
        // Write only if the note did not change since it was read (Neovim, Obsidian, Syncthing).
        match v.save(&c.path, &c.content, c.mtime).await {
            Ok(_) => res.updated.push(c.path),
            Err(vault::Error::Conflict(_)) => res.skipped.push(c.path),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(res)
}

#[cfg(test)]
mod tests;
