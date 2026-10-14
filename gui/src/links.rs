// Links in Obsidian format: [[target#page=12|alias]] (port of web/src/lib/links.ts and
// linkText from linkComplete.ts).

use std::sync::LazyLock;

use regex::Regex;

use crate::api::Entry;

pub static WIKILINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[([^\]|#]+)(#[^\]|]*)?(\|[^\]]*)?\]\]").unwrap_or_else(|_| unreachable!())
});

pub static IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(png|jpe?g|gif|webp|svg|avif|bmp)$").unwrap_or_else(|_| unreachable!())
});

#[derive(Clone, Debug, PartialEq, Default)]
pub struct WikiLink {
    pub target: String,
    pub page: Option<u32>,
    /// Position in an EPUB: [[book.epub#cfi=epubcfi(...)]]
    pub cfi: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Parsed {
    pub link: WikiLink,
    pub reference: String,
    pub alias: Option<String>,
}

/// The inside of [[...]]: escaped \| and \# (in tables) count as the separators.
pub fn parse_wikilink(raw: &str) -> Parsed {
    let unescaped = raw.replace("\\|", "|").replace("\\#", "#");
    let mut parts = unescaped.split('|');
    let reference = parts.next().unwrap_or("").to_string();
    let alias = parts.next().map(str::to_string);
    let page = reference.find("#page=").and_then(|i| {
        let digits: String = reference[i + 6..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    });
    let cfi = reference
        .find("#cfi=")
        .map(|i| reference[i + 5..].to_string())
        .filter(|c| !c.is_empty());
    let target = reference.split('#').next().unwrap_or("").trim().to_string();
    Parsed {
        link: WikiLink { target, page, cfi },
        reference,
        alias,
    }
}

/// Finds the wikilink that contains character column `col` of the line.
pub fn link_at(line: &str, col: usize) -> Option<WikiLink> {
    for m in WIKILINK.find_iter(line) {
        let start = line[..m.start()].chars().count();
        let len = m.as_str().chars().count();
        if col >= start && col <= start + len {
            let s = m.as_str();
            return Some(parse_wikilink(&s[2..s.len() - 2]).link);
        }
    }
    None
}

pub fn flatten(entries: &[Entry], out: &mut Vec<String>) {
    for e in entries {
        if e.dir {
            flatten(e.kids(), out);
        } else {
            out.push(e.path.clone());
        }
    }
}

pub fn all_files(entries: &[Entry]) -> Vec<String> {
    let mut out = Vec::new();
    flatten(entries, &mut out);
    out
}

/// Resolves the target like Obsidian: full path or file name only, with or without .md.
pub fn resolve(files: &[String], target: &str) -> Option<String> {
    let candidates = [target.to_string(), format!("{target}.md")];
    for c in &candidates {
        if let Some(f) = files.iter().find(|f| *f == c) {
            return Some(f.clone());
        }
    }
    for c in &candidates {
        let suffix = format!("/{c}");
        if let Some(f) = files.iter().find(|f| f.ends_with(&suffix) || *f == c) {
            return Some(f.clone());
        }
    }
    None
}

/// Link to the current page, in a format Obsidian understands too.
pub fn page_link(path: &str, page: u32) -> String {
    format!("[[{path}#page={page}]]")
}

/// Link to a position in an EPUB, without the optional id assertions ("[c3]") that would
/// break the [[link]].
pub fn cfi_link(path: &str, cfi: &str) -> String {
    static ASSERT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\[[^\]]*\]").unwrap_or_else(|_| unreachable!()));
    format!("[[{path}#cfi={}]]", ASSERT.replace_all(cfi, ""))
}

pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn dir_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

pub fn strip_md(name: &str) -> &str {
    if name.len() >= 3 && name[name.len() - 3..].eq_ignore_ascii_case(".md") {
        &name[..name.len() - 3]
    } else {
        name
    }
}

pub fn is_md(path: &str) -> bool {
    path.to_lowercase().ends_with(".md")
}

pub fn is_pdf(path: &str) -> bool {
    path.to_lowercase().ends_with(".pdf")
}

pub fn is_epub(path: &str) -> bool {
    path.to_lowercase().ends_with(".epub")
}

/// Link text the way Obsidian writes it: the name only when it is unique in the vault,
/// otherwise the path. Notes without ".md"; other files with their extension.
pub fn link_text(path: &str, all: &[String]) -> String {
    let name = file_name(path);
    let lower = name.to_lowercase();
    let unique = all
        .iter()
        .filter(|p| file_name(p).to_lowercase() == lower)
        .count()
        <= 1;
    let text = if unique { name } else { path };
    strip_md(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_alias_and_escapes() {
        let p = parse_wikilink("book.pdf#page=12|chapter");
        assert_eq!(p.link.target, "book.pdf");
        assert_eq!(p.link.page, Some(12));
        assert_eq!(p.alias.as_deref(), Some("chapter"));
        let e = parse_wikilink("Note\\|shown");
        assert_eq!(e.link.target, "Note");
        assert_eq!(e.alias.as_deref(), Some("shown"));
        let c = parse_wikilink("b.epub#cfi=epubcfi(/6/4!/4/2)");
        assert_eq!(c.link.cfi.as_deref(), Some("epubcfi(/6/4!/4/2)"));
    }

    #[test]
    fn finds_link_under_column() {
        let line = "see [[A|b]] and [[C#page=3]]";
        assert_eq!(link_at(line, 6).map(|l| l.target), Some("A".into()));
        assert_eq!(link_at(line, 20).and_then(|l| l.page), Some(3));
        assert_eq!(link_at(line, 1), None);
    }

    #[test]
    fn resolves_like_obsidian() {
        let files = vec![
            "a/Note.md".to_string(),
            "Note2.md".to_string(),
            "b/x.pdf".to_string(),
        ];
        assert_eq!(resolve(&files, "Note").as_deref(), Some("a/Note.md"));
        assert_eq!(resolve(&files, "x.pdf").as_deref(), Some("b/x.pdf"));
        assert_eq!(resolve(&files, "missing"), None);
        assert_eq!(link_text("a/Note.md", &files), "Note");
        let dup = vec!["a/N.md".to_string(), "b/n.md".to_string()];
        assert_eq!(link_text("a/N.md", &dup), "a/N");
        assert_eq!(
            cfi_link("b.epub", "epubcfi(/6/4[c3]!/4)"),
            "[[b.epub#cfi=epubcfi(/6/4!/4)]]"
        );
    }
}
