// The EPUB reader (EpubReader.svelte, foliate-js): one chapter at a time, scrolled; at the
// end (or start) of a chapter a bit more wheel in the same direction moves to the next (or
// previous) one. Chapters list, previous/next, position as a percentage, last position per
// book, and [[book.epub#cfi=…]] links in the CFI format foliate writes (resolved to the
// paragraph they point into).

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::{Duration, Instant};

use eframe::egui::{self, Frame, Id, Key, Margin, Modifiers, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::fonts;
use crate::icons;
use crate::kit;
use crate::markdown::{self, Block, ImageRef, Images, Inline, Look, Painter, Style, Target};
use crate::state::Theme;

pub const NOT_FOUND: &str = "link position not found in this book";

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    /// Which book this position belongs to.
    pub id: String,
    pub cfi: String,
    /// 0–1 of the whole book.
    pub fraction: f32,
    pub chapter: String,
}

#[derive(Clone, Debug)]
pub struct TocItem {
    pub label: String,
    /// Zip path of the chapter, with the #fragment.
    pub href: String,
    pub depth: usize,
}

struct SpineItem {
    path: String,
    size: u64,
}

pub struct Book {
    zip: zip::ZipArchive<Cursor<Vec<u8>>>,
    spine: Vec<SpineItem>,
    pub toc: Vec<TocItem>,
    /// Where the text starts (landmarks "bodymatter" / guide "text"), for a first opening.
    text_start: Option<String>,
}

// ---------------------------------------------------------------- paths and entities

fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..=i])
}

/// A relative href of a file inside the zip, resolved against `base` (another zip path).
pub fn resolve(base: &str, href: &str) -> String {
    let href = percent_encoding::percent_decode_str(href).decode_utf8_lossy().into_owned();
    let (path, frag) = match href.split_once('#') {
        Some((p, f)) => (p.to_string(), Some(f.to_string())),
        None => (href.clone(), None),
    };
    let joined = if path.is_empty() { base.to_string() } else if path.starts_with('/') { path[1..].to_string() } else { format!("{}{}", dir_of(base), path) };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut out = parts.join("/");
    if let Some(f) = frag {
        out.push('#');
        out.push_str(&f);
    }
    out
}

fn entity(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let n = if let Some(hex) = num.strip_prefix(['x', 'X']) { u32::from_str_radix(hex, 16).ok()? } else { num.parse().ok()? };
        return char::from_u32(n).map(String::from);
    }
    Some(
        match name {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            "nbsp" => "\u{a0}",
            "shy" => "\u{ad}",
            "mdash" => "—",
            "ndash" => "–",
            "hellip" => "…",
            "lsquo" => "‘",
            "rsquo" => "’",
            "ldquo" => "“",
            "rdquo" => "”",
            "laquo" => "«",
            "raquo" => "»",
            "copy" => "©",
            "middot" => "·",
            "bull" => "•",
            "thinsp" => "\u{2009}",
            "ensp" => "\u{2002}",
            "emsp" => "\u{2003}",
            _ => return None,
        }
        .to_string(),
    )
}

fn local_name(e: &BytesStart<'_>) -> String {
    let n = e.local_name();
    String::from_utf8_lossy(n.as_ref()).to_lowercase()
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let key = String::from_utf8_lossy(a.key.as_ref()).to_lowercase();
        let local = key.rsplit(':').next().unwrap_or(&key).to_string();
        (key == name || local == name).then(|| a.unescape_value().map(|v| v.into_owned()).unwrap_or_default())
    })
}

fn xml_reader(text: &str) -> Reader<&[u8]> {
    let mut r = Reader::from_str(text);
    let c = r.config_mut();
    c.check_end_names = false;
    c.allow_unmatched_ends = true;
    c.expand_empty_elements = false;
    r
}

// ---------------------------------------------------------------- package

fn read_file(zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, path: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(path).ok()?;
    let mut v = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut v).ok()?;
    Some(v)
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl Book {
    pub fn open(data: Vec<u8>) -> Result<Book, String> {
        let mut zip = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| e.to_string())?;
        let container = read_file(&mut zip, "META-INF/container.xml").ok_or("not an EPUB: no META-INF/container.xml")?;
        let container = text_of(&container);
        let mut opf_path = None;
        let mut r = xml_reader(&container);
        loop {
            match r.read_event() {
                Ok(Event::Start(e) | Event::Empty(e)) if local_name(&e) == "rootfile" => {
                    opf_path = attr(&e, "full-path");
                    break;
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        let opf_path = opf_path.ok_or("no package document")?;
        let opf = text_of(&read_file(&mut zip, &opf_path).ok_or("package document missing")?);
        let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
        let mut spine_ids = Vec::new();
        let mut ncx_id = None;
        let mut guide_text = None;
        let mut r = xml_reader(&opf);
        loop {
            match r.read_event() {
                Ok(Event::Start(e) | Event::Empty(e)) => match local_name(&e).as_str() {
                    "item" => {
                        if let (Some(id), Some(href)) = (attr(&e, "id"), attr(&e, "href")) {
                            let mt = attr(&e, "media-type").unwrap_or_default();
                            let props = attr(&e, "properties").unwrap_or_default();
                            manifest.insert(id, (resolve(&opf_path, &href), mt, props));
                        }
                    }
                    "spine" => ncx_id = attr(&e, "toc"),
                    "itemref" => {
                        if let Some(id) = attr(&e, "idref") {
                            spine_ids.push(id);
                        }
                    }
                    "reference" => {
                        if attr(&e, "type").as_deref() == Some("text") {
                            guide_text = attr(&e, "href").map(|h| resolve(&opf_path, &h));
                        }
                    }
                    _ => {}
                },
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        let mut spine = Vec::new();
        for id in &spine_ids {
            if let Some((path, _, _)) = manifest.get(id) {
                let size = zip.by_name(path).map(|f| f.size()).unwrap_or(1).max(1);
                spine.push(SpineItem { path: path.clone(), size });
            }
        }
        if spine.is_empty() {
            return Err("the book has no chapters".into());
        }
        let mut toc = Vec::new();
        let mut text_start = guide_text;
        if let Some((nav_path, _, _)) = manifest.values().find(|(_, _, props)| props.split_whitespace().any(|p| p == "nav")) {
            if let Some(bytes) = read_file(&mut zip, nav_path) {
                let (items, start) = parse_nav(&text_of(&bytes), nav_path);
                toc = items;
                if start.is_some() {
                    text_start = start;
                }
            }
        }
        if toc.is_empty() {
            let ncx = ncx_id.and_then(|id| manifest.get(&id).cloned()).or_else(|| {
                manifest.values().find(|(_, mt, _)| mt == "application/x-dtbncx+xml").cloned()
            });
            if let Some((ncx_path, _, _)) = ncx {
                if let Some(bytes) = read_file(&mut zip, &ncx_path) {
                    toc = parse_ncx(&text_of(&bytes), &ncx_path);
                }
            }
        }
        Ok(Book { zip, spine, toc, text_start })
    }

    pub fn chapters(&self) -> usize {
        self.spine.len()
    }

    fn spine_index(&self, path: &str) -> Option<usize> {
        let p = path.split('#').next().unwrap_or(path);
        self.spine.iter().position(|s| s.path == p)
    }

    fn file(&mut self, path: &str) -> Option<Vec<u8>> {
        read_file(&mut self.zip, path)
    }

    /// Fraction of the book before chapter `i` and the size of chapter `i` (by bytes, like foliate).
    fn weights(&self, i: usize) -> (f32, f32) {
        let total: u64 = self.spine.iter().map(|s| s.size).sum();
        let before: u64 = self.spine[..i].iter().map(|s| s.size).sum();
        (before as f32 / total.max(1) as f32, self.spine[i].size as f32 / total.max(1) as f32)
    }
}

/// The table of contents of an EPUB 3 navigation document (and the "bodymatter" landmark).
fn parse_nav(text: &str, base: &str) -> (Vec<TocItem>, Option<String>) {
    let mut r = xml_reader(text);
    let mut out = Vec::new();
    let mut start = None;
    let mut nav_kind = String::new();
    let mut depth = 0usize;
    let mut in_nav = false;
    let mut link: Option<(String, String, String)> = None; // href, label, epub:type
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "nav" => {
                    in_nav = true;
                    nav_kind = attr(&e, "type").unwrap_or_default();
                    depth = 0;
                }
                "ol" if in_nav => depth += 1,
                "a" if in_nav => {
                    link = Some((attr(&e, "href").unwrap_or_default(), String::new(), attr(&e, "type").unwrap_or_default()));
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if let Some(l) = &mut link {
                    l.1.push_str(&t.decode().map(|c| c.into_owned()).unwrap_or_default());
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if let Some(l) = &mut link {
                    if let Some(s) = entity(&String::from_utf8_lossy(&e)) {
                        l.1.push_str(&s);
                    }
                }
            }
            Ok(Event::End(e)) => match String::from_utf8_lossy(e.local_name().as_ref()).to_lowercase().as_str() {
                "nav" => in_nav = false,
                "ol" if in_nav => depth = depth.saturating_sub(1),
                "a" => {
                    if let Some((href, label, kind)) = link.take() {
                        let href = resolve(base, &href);
                        if nav_kind.contains("toc") {
                            let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
                            out.push(TocItem { label, href, depth: depth.saturating_sub(1) });
                        } else if nav_kind.contains("landmarks") && kind.contains("bodymatter") {
                            start = Some(href);
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    (out, start)
}

/// The table of contents of an EPUB 2 NCX file.
fn parse_ncx(text: &str, base: &str) -> Vec<TocItem> {
    let mut r = xml_reader(text);
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut in_text = false;
    let mut label = String::new();
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => match local_name(&e).as_str() {
                "navpoint" => {
                    depth += 1;
                    label.clear();
                }
                "text" => in_text = true,
                _ => {}
            },
            Ok(Event::Empty(e)) if local_name(&e) == "content" => {
                if let Some(src) = attr(&e, "src") {
                    out.push(TocItem { label: label.trim().to_string(), href: resolve(base, &src), depth: depth.saturating_sub(1) });
                }
            }
            Ok(Event::Text(t)) if in_text => label.push_str(&t.decode().map(|c| c.into_owned()).unwrap_or_default()),
            Ok(Event::End(e)) => match String::from_utf8_lossy(e.local_name().as_ref()).to_lowercase().as_str() {
                "navpoint" => depth = depth.saturating_sub(1),
                "text" => in_text = false,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------- chapters

/// Where each block of a chapter came from: the CFI steps of its element and the ids in it.
#[derive(Clone, Debug, Default)]
pub struct BlockInfo {
    pub path: Vec<u32>,
}

pub struct Chapter {
    pub blocks: Vec<Block>,
    pub infos: Vec<BlockInfo>,
    pub ids: HashMap<String, usize>,
}

const BLOCK_TAGS: &[&str] = &[
    "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "blockquote", "pre", "section", "article", "header", "footer",
    "aside", "nav", "figure", "figcaption", "dt", "dd", "table", "tr", "td", "th", "ul", "ol", "dl", "hr", "body", "main",
    "address", "center",
];

struct Conv {
    chapter: String,
    out: Chapter,
    inl: Vec<Inline>,
    style: Vec<Style>,
    links: Vec<(String, usize)>,
    /// CFI steps of the open elements, and how many element children each has so far.
    path: Vec<u32>,
    children: Vec<u32>,
    names: Vec<String>,
    block_path: Option<Vec<u32>>,
    pending_ids: Vec<String>,
    heading: Option<u8>,
    quote: usize,
    pre: usize,
    skip: usize,
    lists: Vec<(bool, u32)>,
}

impl Conv {
    fn cur_style(&self) -> Style {
        self.style.last().copied().unwrap_or_default()
    }

    fn text(&mut self, s: &str) {
        if self.skip > 0 {
            return;
        }
        let style = self.cur_style();
        if self.pre > 0 {
            self.inl.push(Inline::Text(s.to_string(), style));
            return;
        }
        // HTML whitespace: runs collapse to one space; none at the start of a block.
        let mut t = String::with_capacity(s.len());
        let mut last_space = match self.inl.last() {
            None | Some(Inline::Break) => true,
            Some(Inline::Text(x, _)) => x.ends_with(' '),
            _ => false,
        };
        for c in s.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                if !last_space {
                    t.push(' ');
                    last_space = true;
                }
            } else {
                t.push(c);
                last_space = false;
            }
        }
        if !t.is_empty() {
            if self.block_path.is_none() {
                self.block_path = Some(self.path.clone());
            }
            self.inl.push(Inline::Text(t, style));
        }
    }

    fn flush(&mut self) {
        // Trailing spaces and breaks do not make a block of their own.
        while let Some(Inline::Text(t, _)) = self.inl.last_mut() {
            let trimmed = t.trim_end_matches(' ').len();
            if trimmed == 0 {
                self.inl.pop();
            } else {
                t.truncate(trimmed);
                break;
            }
        }
        let has_content = self.inl.iter().any(|i| match i {
            Inline::Text(t, _) => !t.trim().is_empty(),
            Inline::Image(_) | Inline::Link(..) => true,
            Inline::Break => false,
        });
        if !has_content {
            self.inl.clear();
            return;
        }
        let content = std::mem::take(&mut self.inl);
        let block = match self.heading {
            Some(l) => Block::Heading(l, content),
            None if self.pre > 0 => Block::Code(content.iter().map(|i| if let Inline::Text(t, _) = i { t.as_str() } else { "\n" }).collect()),
            None => Block::Para(content),
        };
        let block = if self.quote > 0 { Block::Quote(vec![block]) } else { block };
        let idx = self.out.blocks.len();
        self.out.blocks.push(block);
        self.out.infos.push(BlockInfo { path: self.block_path.take().unwrap_or_else(|| self.path.clone()) });
        for id in self.pending_ids.drain(..) {
            self.out.ids.entry(id).or_insert(idx);
        }
    }

    fn start(&mut self, e: &BytesStart<'_>, empty: bool) {
        let name = local_name(e);
        // CFI: even steps count element children.
        let step = {
            let c = self.children.last_mut();
            match c {
                Some(c) => {
                    *c += 1;
                    *c * 2
                }
                None => 2,
            }
        };
        if matches!(name.as_str(), "head" | "script" | "style" | "title") {
            if !empty {
                self.skip += 1;
                self.push(name, step);
            }
            return;
        }
        if self.skip > 0 {
            if !empty {
                self.push(name, step);
            }
            return;
        }
        let block = BLOCK_TAGS.contains(&name.as_str());
        if block {
            self.flush();
        }
        if let Some(id) = attr(e, "id") {
            self.pending_ids.push(id);
        }
        match name.as_str() {
            "br" => self.inl.push(Inline::Break),
            "img" | "image" => {
                let src = attr(e, "src").or_else(|| attr(e, "href"));
                if let Some(src) = src {
                    if self.block_path.is_none() {
                        let mut p = self.path.clone();
                        p.push(step);
                        self.block_path = Some(p);
                    }
                    let key = format!("epub:{}", resolve(&self.chapter, &src));
                    self.inl.push(Inline::Image(ImageRef { key, alt: attr(e, "alt").unwrap_or_default(), width: None }));
                }
            }
            "hr" => {
                self.out.blocks.push(Block::Rule);
                self.out.infos.push(BlockInfo { path: { let mut p = self.path.clone(); p.push(step); p } });
            }
            _ => {}
        }
        if empty {
            return;
        }
        self.push(name.clone(), step);
        let mut st = self.cur_style();
        match name.as_str() {
            "em" | "i" | "cite" | "var" | "dfn" => st.italic = true,
            "strong" | "b" | "th" => st.bold = true,
            "code" | "tt" | "kbd" | "samp" => st.code = true,
            "sup" | "sub" | "small" => st.small = true,
            "del" | "s" | "strike" => st.strike = true,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.heading = name[1..].parse().ok(),
            "blockquote" => self.quote += 1,
            "pre" => self.pre += 1,
            "ul" => self.lists.push((false, 0)),
            "ol" => self.lists.push((true, attr(e, "start").and_then(|s| s.parse::<u32>().ok()).unwrap_or(1).saturating_sub(1))),
            "li" => {
                let depth = self.lists.len().max(1);
                let marker = match self.lists.last_mut() {
                    Some((true, n)) => {
                        *n += 1;
                        format!("{n}. ")
                    }
                    _ => "• ".to_string(),
                };
                self.block_path = Some(self.path.clone());
                self.inl.push(Inline::Text(format!("{}{marker}", "\u{2003}".repeat(depth - 1)), Style::default()));
            }
            "a" => {
                if let Some(href) = attr(e, "href") {
                    self.links.push((href, self.inl.len()));
                }
            }
            _ => {}
        }
        self.style.push(st);
    }

    fn push(&mut self, name: String, step: u32) {
        self.names.push(name);
        self.path.push(step);
        self.children.push(0);
    }

    fn end(&mut self) {
        let Some(name) = self.names.pop() else { return };
        self.path.pop();
        self.children.pop();
        if matches!(name.as_str(), "head" | "script" | "style" | "title") {
            self.skip = self.skip.saturating_sub(1);
            return;
        }
        if self.skip > 0 {
            return;
        }
        self.style.pop();
        match name.as_str() {
            "a" => {
                if let Some((href, start)) = self.links.pop() {
                    let start = start.min(self.inl.len());
                    let content: Vec<Inline> = self.inl.drain(start..).collect();
                    let lower = href.to_lowercase();
                    let target = if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:") {
                        Target::Href(href)
                    } else {
                        Target::Href(format!("epub-internal:{}", resolve(&self.chapter, &href)))
                    };
                    if !content.is_empty() {
                        self.inl.push(Inline::Link(content, target));
                    }
                }
            }
            "blockquote" => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            "pre" => {
                self.flush();
                self.pre = self.pre.saturating_sub(1);
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.pop();
            }
            n if n.len() == 2 && n.starts_with('h') && n[1..].parse::<u8>().is_ok() => {
                self.flush();
                self.heading = None;
            }
            n if BLOCK_TAGS.contains(&n) => self.flush(),
            _ => {}
        }
    }
}

/// A chapter's XHTML as blocks, with the CFI path of each.
pub fn parse_chapter(xhtml: &str, chapter: &str) -> Chapter {
    let mut c = Conv {
        chapter: chapter.to_string(),
        out: Chapter { blocks: Vec::new(), infos: Vec::new(), ids: HashMap::new() },
        inl: Vec::new(),
        style: Vec::new(),
        links: Vec::new(),
        path: Vec::new(),
        children: Vec::new(),
        names: Vec::new(),
        block_path: None,
        pending_ids: Vec::new(),
        heading: None,
        quote: 0,
        pre: 0,
        skip: 0,
        lists: Vec::new(),
    };
    let mut r = xml_reader(xhtml);
    let mut root_seen = false;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                if !root_seen {
                    // <html>: the CFI paths start below it (body is /4).
                    root_seen = true;
                    c.names.push("html".into());
                    c.children.push(0);
                    continue;
                }
                c.start(&e, false);
            }
            Ok(Event::Empty(e)) => c.start(&e, true),
            Ok(Event::End(_)) => c.end(),
            Ok(Event::Text(t)) => {
                let s = t.decode().map(|x| x.into_owned()).unwrap_or_default();
                c.text(&s);
            }
            Ok(Event::CData(t)) => {
                let s = String::from_utf8_lossy(&t).into_owned();
                c.text(&s);
            }
            Ok(Event::GeneralRef(e)) => {
                let name = String::from_utf8_lossy(&e).into_owned();
                if let Some(s) = entity(&name) {
                    // A no-break space must not collapse with the spaces around it.
                    if c.skip == 0 {
                        let st = c.cur_style();
                        if c.block_path.is_none() {
                            c.block_path = Some(c.path.clone());
                        }
                        c.inl.push(Inline::Text(s, st));
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    c.flush();
    c.out
}

// ---------------------------------------------------------------- CFI

pub fn make_cfi(spine: usize, path: &[u32]) -> String {
    let inner: String = path.iter().map(|s| format!("/{s}")).collect();
    format!("epubcfi(/6/{}!{inner})", (spine + 1) * 2)
}

fn steps(s: &str) -> Vec<u32> {
    // /4/2[id]/10:3 → [4, 2, 10]; assertions and character offsets are ignored.
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('/') {
        rest = &rest[i + 1..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse() {
            out.push(n);
        }
    }
    out
}

/// The chapter and the element path of a CFI, from this app or from foliate (range CFIs too).
pub fn parse_cfi(cfi: &str) -> Option<(usize, Vec<u32>)> {
    let inner = cfi.trim().strip_prefix("epubcfi(")?.strip_suffix(')')?;
    let (pkg, content) = inner.split_once('!').unwrap_or((inner, ""));
    let spine_step = *steps(pkg).last()?;
    if spine_step < 2 || spine_step % 2 != 0 {
        return None;
    }
    let mut parts = content.split(',');
    let mut path = steps(parts.next().unwrap_or(""));
    if let Some(start) = parts.next() {
        path.extend(steps(start));
    }
    // Odd steps are text nodes: the element containing them is what matters here.
    path.retain(|s| s % 2 == 0);
    Some((spine_step as usize / 2 - 1, path))
}

/// The block an element path points into: the one sharing the longest prefix with it.
pub fn block_for(infos: &[BlockInfo], path: &[u32]) -> usize {
    let mut best = (0usize, 0usize);
    for (i, info) in infos.iter().enumerate() {
        let common = info.path.iter().zip(path).take_while(|(a, b)| a == b).count();
        if common > best.1 {
            best = (i, common);
        }
    }
    best.0
}

// ---------------------------------------------------------------- reader

enum Scroll {
    Top,
    Bottom,
    Block(usize),
}

pub struct EpubReader {
    pub id: String,
    pub title: String,
    pub book: Option<Book>,
    pub error: String,
    chapter: usize,
    current: Option<Chapter>,
    tops: Vec<f32>,
    scroll_to: Option<Scroll>,
    push: f32,
    turning_until: Option<Instant>,
    pub location: Option<Location>,
    images: Images,
    pending_target: Option<(String, u64)>,
    last_target: u64,
    saved: Option<String>,
    viewport_h: f32,
    offset: f32,
    content_h: f32,
    scroll_by: f32,
}

/// Wheel "push" past the end/start of a chapter before turning, so trackpad inertia does not
/// skip chapters.
const EDGE_PUSH: f32 = 120.0;

impl EpubReader {
    pub fn new(id: String, title: String, saved: Option<String>) -> Self {
        EpubReader {
            id,
            title,
            book: None,
            error: String::new(),
            chapter: 0,
            current: None,
            tops: Vec::new(),
            scroll_to: None,
            push: 0.0,
            turning_until: None,
            location: None,
            images: Images::default(),
            pending_target: None,
            last_target: 0,
            saved,
            viewport_h: 600.0,
            offset: 0.0,
            content_h: 0.0,
            scroll_by: 0.0,
        }
    }

    /// The book is parsed: open at the link target, else the saved position, else the text start.
    pub fn opened(&mut self, book: Book) {
        self.book = Some(book);
        let target = self.pending_target.take();
        if let Some((cfi, n)) = &target {
            self.last_target = *n;
            if self.go_cfi(cfi) {
                return;
            }
            self.error = NOT_FOUND.into();
        }
        if let Some(saved) = self.saved.clone() {
            if self.go_cfi(&saved) {
                return;
            }
        }
        let start = self.book.as_ref().and_then(|b| b.text_start.clone());
        match start {
            Some(href) => {
                if !self.go_href(&href) {
                    self.load_chapter(0, Scroll::Top);
                }
            }
            None => self.load_chapter(0, Scroll::Top),
        }
    }

    /// A request to go to a position ([[book.epub#cfi=…]]); n changes on every request.
    pub fn target(&mut self, cfi: &str, n: u64) {
        if n == self.last_target {
            return;
        }
        if self.book.is_none() {
            self.pending_target = Some((cfi.to_string(), n));
            return;
        }
        self.last_target = n;
        self.error.clear();
        if !self.go_cfi(cfi) {
            self.error = NOT_FOUND.into();
        }
    }

    fn valid(&self, cfi: &str) -> Option<(usize, Vec<u32>)> {
        let (spine, path) = parse_cfi(cfi)?;
        (spine < self.book.as_ref()?.chapters()).then_some((spine, path))
    }

    fn go_cfi(&mut self, cfi: &str) -> bool {
        let Some((spine, path)) = self.valid(cfi) else { return false };
        self.load_chapter(spine, Scroll::Top);
        if let Some(ch) = &self.current {
            let b = block_for(&ch.infos, &path);
            self.scroll_to = Some(Scroll::Block(b));
        }
        true
    }

    fn go_href(&mut self, href: &str) -> bool {
        let Some(i) = self.book.as_ref().and_then(|b| b.spine_index(href)) else { return false };
        self.load_chapter(i, Scroll::Top);
        if let Some((_, frag)) = href.split_once('#') {
            if let Some(&b) = self.current.as_ref().and_then(|c| c.ids.get(frag)) {
                self.scroll_to = Some(Scroll::Block(b));
            }
        }
        true
    }

    fn load_chapter(&mut self, i: usize, at: Scroll) {
        let Some(book) = &mut self.book else { return };
        let i = i.min(book.chapters() - 1);
        let path = book.spine[i].path.clone();
        let xhtml = book.file(&path).map(|b| text_of(&b)).unwrap_or_default();
        self.current = Some(parse_chapter(&xhtml, &path));
        self.chapter = i;
        self.tops.clear();
        self.scroll_to = Some(at);
        // Images of the previous chapter are freed.
        self.images.clear();
    }

    /// User-initiated navigation: the previous error message no longer applies.
    pub fn next(&mut self) {
        self.error.clear();
        if self.offset + self.viewport_h < self.content_h - 2.0 {
            self.scroll_by += self.viewport_h * 0.9;
        } else if self.book.as_ref().is_some_and(|b| self.chapter + 1 < b.chapters()) {
            self.load_chapter(self.chapter + 1, Scroll::Top);
        }
    }

    pub fn prev(&mut self) {
        self.error.clear();
        if self.offset > 0.5 {
            self.scroll_by -= self.viewport_h * 0.9;
        } else if self.chapter > 0 {
            self.load_chapter(self.chapter - 1, Scroll::Bottom);
        }
    }

    /// Same interface as the PDF reader (Alt+↑/↓ while the editor has focus).
    pub fn scroll(&mut self, dir: i32) {
        if dir > 0 { self.next() } else { self.prev() }
    }

    pub fn turn_page(&mut self, dir: i32) {
        self.scroll(dir);
    }

    fn chapter_label(&self, block: usize) -> String {
        let Some(book) = &self.book else { return String::new() };
        let here = &book.spine[self.chapter].path;
        let mut label = None;
        for item in &book.toc {
            let (p, frag) = item.href.split_once('#').map_or((item.href.as_str(), None), |(p, f)| (p, Some(f)));
            let Some(idx) = book.spine_index(p) else { continue };
            if idx > self.chapter {
                break;
            }
            if idx < self.chapter {
                label = Some(item.label.clone());
                continue;
            }
            let at = frag.and_then(|f| self.current.as_ref()?.ids.get(f).copied()).unwrap_or(0);
            if at <= block || p != here {
                label = Some(item.label.clone());
            }
        }
        label.unwrap_or_default()
    }

    pub fn wanted_images(&mut self) -> Vec<(String, Vec<u8>)> {
        let wanted = std::mem::take(&mut self.images.wanted);
        let Some(book) = &mut self.book else { return Vec::new() };
        wanted
            .into_iter()
            .map(|k| {
                let path = k.strip_prefix("epub:").unwrap_or(&k).to_string();
                let bytes = book.file(&path).unwrap_or_default();
                (k, bytes)
            })
            .collect()
    }

    pub fn image_loaded(&mut self, ctx: &egui::Context, key: String, img: Option<egui::ColorImage>) {
        self.images.insert(ctx, key, img);
    }

    /// Draws the reader; returns an external link to open in the browser, if one was clicked.
    #[allow(clippy::too_many_lines)]
    pub fn ui(&mut self, ui: &mut Ui, theme: Theme, p: &crate::theme::Palette) -> Option<String> {
        let mut external = None;
        // Toolbar: title, chapters, previous, %, next.
        Frame::new().inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
                let toc_w = if self.book.as_ref().is_some_and(|b| !b.toc.is_empty()) { (ui.available_width() * 0.4).min(260.0) } else { 0.0 };
                let w = (ui.available_width() - toc_w - 2.0 * 32.0 - 40.0 - 24.0).max(40.0);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::hover());
                crate::tree::paint_truncated(ui, r, crate::links::file_name(&self.title), fonts::ui(13.0), p.text);
                resp.on_hover_text(&self.title);
                if toc_w > 0.0 {
                    let current = self.location.as_ref().map(|l| l.chapter.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "Chapters".into());
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt(("epub-toc", &self.id))
                        .width(toc_w)
                        .selected_text(RichText::new(current).font(fonts::ui(12.0)))
                        .show_ui(ui, |ui| {
                            if let Some(book) = &self.book {
                                for (i, item) in book.toc.iter().enumerate() {
                                    let label = format!("{}{}", "  ".repeat(item.depth), item.label);
                                    if ui.selectable_label(false, RichText::new(label).font(fonts::ui(12.0))).clicked() {
                                        chosen = Some(i);
                                    }
                                }
                            }
                        });
                    if let Some(i) = chosen {
                        self.error.clear();
                        if let Some(href) = self.book.as_ref().map(|b| b.toc[i].href.clone()) {
                            self.go_href(&href);
                        }
                    }
                }
                if kit::button(ui, Some(icons::CHEVRON_LEFT), "", "Previous page (←)").clicked() {
                    self.prev();
                }
                let pct = self.location.as_ref().map_or("…".to_string(), |l| format!("{}%", (l.fraction * 100.0).round() as i32));
                ui.label(RichText::new(pct).font(fonts::ui(13.0)).color(p.muted));
                if kit::button(ui, Some(icons::CHEVRON_RIGHT), "", "Next page (→)").clicked() {
                    self.next();
                }
            });
        });
        let y = ui.cursor().top();
        ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.border));
        if !self.error.is_empty() {
            Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.label(RichText::new(format!("EPUB error: {}", self.error)).color(p.error).font(fonts::ui(13.0)));
            });
        }
        // Theme colors inside the book.
        let (bg, fg, link) = match theme {
            Theme::Light => (crate::theme::PAPER.page_bg, p.text, crate::theme::PAPER.link),
            Theme::Dark => (crate::theme::DARK.page_bg, p.text, crate::theme::DARK.link),
        };
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, bg);
        let focus_id = Id::new(("epub-view", &self.id));
        let bg_resp = ui.interact(area, focus_id, Sense::click());
        if bg_resp.clicked() {
            bg_resp.request_focus();
        }
        // Arrow keys inside the book also turn the page.
        if ui.memory(|m| m.has_focus(focus_id)) {
            let (n, pv) = ui.input_mut(|i| {
                let n = i.consume_key(Modifiers::NONE, Key::ArrowRight) || i.consume_key(Modifiers::NONE, Key::PageDown) || i.consume_key(Modifiers::NONE, Key::Space);
                let pv = i.consume_key(Modifiers::NONE, Key::ArrowLeft) || i.consume_key(Modifiers::NONE, Key::PageUp);
                (n, pv)
            });
            if n {
                self.next();
            }
            if pv {
                self.prev();
            }
        }
        let Some(chapter) = self.current.take() else {
            if self.book.is_none() && self.error.is_empty() {
                ui.allocate_rect(area, Sense::hover());
            }
            return None;
        };
        let mut sa = ScrollArea::vertical().id_salt(("epub-scroll", &self.id, self.chapter)).auto_shrink([false, false]);
        match self.scroll_to.take() {
            Some(Scroll::Top) => sa = sa.vertical_scroll_offset(0.0),
            Some(Scroll::Bottom) => sa = sa.vertical_scroll_offset(f32::MAX / 4.0),
            Some(Scroll::Block(b)) => match self.tops.get(b) {
                Some(&t) => sa = sa.vertical_scroll_offset(t),
                // Not laid out yet: try again after this frame.
                None => self.scroll_to = Some(Scroll::Block(b)),
            },
            None => {}
        }
        let scroll_by = std::mem::take(&mut self.scroll_by);
        let look = Look {
            size: 18.0,
            text: fg,
            muted: p.muted,
            heading: fg,
            link,
            border: p.border,
            code_bg: p.code_bg,
            accent: p.accent,
            line_height: 1.6,
            colored_headings: false,
        };
        let mut click = None;
        let out = sa.show(ui, |ui| {
            let avail = ui.available_width();
            let col = (avail - 72.0).min(760.0).max(100.0);
            let left = ((avail - col) / 2.0).max(36.0_f32.min(avail / 2.0));
            ui.add_space(36.0);
            let mut tops = Vec::new();
            ui.horizontal_top(|ui| {
                ui.add_space(left);
                ui.vertical(|ui| {
                    ui.set_width(col);
                    let origin = ui.cursor().top();
                    let mut painter = Painter { look: &look, images: &mut self.images, click: None, hovered_link: false };
                    tops = painter.blocks(ui, &chapter.blocks, col, 0).into_iter().map(|t| t - origin).collect();
                    click = painter.click;
                });
            });
            ui.add_space(36.0);
            if scroll_by != 0.0 {
                ui.scroll_with_delta(Vec2::new(0.0, -scroll_by));
            }
            tops
        });
        self.tops = out.inner;
        self.viewport_h = out.inner_rect.height();
        self.offset = out.state.offset.y;
        self.content_h = out.content_size.y;
        self.current = Some(chapter);

        // Wheel at the edge of a chapter moves to the next/previous one.
        let hovered = ui.rect_contains_pointer(area);
        let dy: f32 = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Point => delta.y,
                        egui::MouseWheelUnit::Line => delta.y * 40.0,
                        egui::MouseWheelUnit::Page => delta.y * self.viewport_h,
                    }),
                    _ => None,
                })
                .sum()
        });
        let turning = self.turning_until.is_some_and(|t| Instant::now() < t);
        if hovered && dy != 0.0 && !turning && !ui.input(|i| i.modifiers.ctrl) {
            let at_end = dy < 0.0 && self.offset + self.viewport_h >= self.content_h - 2.0;
            let at_start = dy > 0.0 && self.offset <= 0.0;
            if !at_end && !at_start {
                self.push = 0.0;
            } else {
                self.push += dy.abs();
                if self.push >= EDGE_PUSH {
                    self.push = 0.0;
                    self.turning_until = Some(Instant::now() + Duration::from_millis(300));
                    let has_next = self.book.as_ref().is_some_and(|b| self.chapter + 1 < b.chapters());
                    if at_end && has_next {
                        self.load_chapter(self.chapter + 1, Scroll::Top);
                    } else if at_start && self.chapter > 0 {
                        self.load_chapter(self.chapter - 1, Scroll::Bottom);
                    }
                }
            }
        }

        match click {
            Some(markdown::Click::Link(Target::Href(h))) => {
                if let Some(internal) = h.strip_prefix("epub-internal:") {
                    self.error.clear();
                    self.go_href(internal);
                } else {
                    external = Some(h);
                }
            }
            _ => {}
        }

        // The position: the first block at the top of the view.
        if let Some(ch) = &self.current {
            let idx = self.tops.iter().rposition(|&t| t <= self.offset + 1.0).unwrap_or(0);
            let path = ch.infos.get(idx).map(|i| i.path.clone()).unwrap_or_default();
            let cfi = make_cfi(self.chapter, &path);
            let (before, size) = self.book.as_ref().map_or((0.0, 1.0), |b| b.weights(self.chapter));
            let within = if self.content_h > self.viewport_h { (self.offset / (self.content_h - self.viewport_h)).clamp(0.0, 1.0) } else { 0.0 };
            let loc = Location { id: self.id.clone(), cfi, fraction: before + size * within, chapter: self.chapter_label(idx) };
            if self.location.as_ref() != Some(&loc) {
                self.location = Some(loc);
            }
        }
        let _ = Inline::Break;
        external
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_hrefs_inside_the_zip() {
        assert_eq!(resolve("OEBPS/content.opf", "text/ch1.xhtml"), "OEBPS/text/ch1.xhtml");
        assert_eq!(resolve("OEBPS/text/ch1.xhtml", "../images/a%20b.png"), "OEBPS/images/a b.png");
        assert_eq!(resolve("OEBPS/text/ch1.xhtml", "#n1"), "OEBPS/text/ch1.xhtml#n1");
    }

    #[test]
    fn parses_cfis_from_foliate_and_from_here() {
        assert_eq!(parse_cfi("epubcfi(/6/14[ch7]!/4/2/10/1:23)"), Some((6, vec![4, 2, 10])));
        assert_eq!(parse_cfi("epubcfi(/6/4!/4/10,/1:0,/3:12)"), Some((1, vec![4, 10])));
        assert_eq!(parse_cfi(&make_cfi(2, &[4, 6])), Some((2, vec![4, 6])));
        assert_eq!(parse_cfi("nonsense"), None);
    }

    #[test]
    fn chapter_blocks_with_paths_ids_and_links() {
        let x = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head>
<body><h1 id="top">Title</h1><p>One &amp; <em>two</em>&nbsp;three</p><div><p id="n2">Second <a href="ch2.xhtml#x">link</a></p></div><p><img src="../img/a.png" alt="A"/></p></body></html>"#;
        let ch = parse_chapter(x, "OEBPS/text/ch1.xhtml");
        assert_eq!(ch.blocks.len(), 4);
        assert!(matches!(&ch.blocks[0], Block::Heading(1, _)));
        assert_eq!(ch.infos[0].path, vec![4, 2]);
        assert_eq!(ch.infos[2].path, vec![4, 6, 2]);
        assert_eq!(ch.ids.get("n2"), Some(&2));
        assert_eq!(ch.ids.get("top"), Some(&0));
        let Block::Para(inl) = &ch.blocks[2] else { panic!() };
        assert!(inl.iter().any(|i| matches!(i, Inline::Link(_, Target::Href(h)) if h == "epub-internal:OEBPS/text/ch2.xhtml#x")));
        let Block::Para(inl) = &ch.blocks[3] else { panic!() };
        assert!(matches!(&inl[0], Inline::Image(img) if img.key == "epub:OEBPS/img/a.png"));
        assert_eq!(block_for(&ch.infos, &[4, 6, 2]), 2);
        assert_eq!(block_for(&ch.infos, &[4, 4]), 1);
    }
}
