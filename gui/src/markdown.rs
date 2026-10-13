// Preview rendering (web/src/lib/markdown.ts + Preview.svelte), with the Obsidian extensions most
// common in notes: [[links]], ![[images]], task lists and frontmatter. The note becomes a small
// document model (blocks and inlines) once per change; each frame draws it with egui. The EPUB
// reader builds the same model from XHTML and reuses the drawing.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, LazyLock};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Pos2, Rect, Response, Sense, Stroke, TextFormat, Ui, Vec2,
    text::{CCursor, LayoutJob},
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;

use crate::fonts;
use crate::links::{IMAGE, parse_wikilink};

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// <sup>/<sub> and small text in books.
    pub small: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Wiki { target: String, page: Option<u32>, cfi: Option<String>, missing: bool, embed: bool },
    Href(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRef {
    /// Cache key and location: a URL of the server, a data: URI or a key of the EPUB.
    pub key: String,
    pub alt: String,
    pub width: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String, Style),
    Link(Vec<Inline>, Target),
    Image(ImageRef),
    Break,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// Task checkbox: checked, and the line of the note (1 = first) it toggles.
    pub task: Option<(bool, usize)>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Para(Vec<Inline>),
    List { ordered: bool, start: u64, items: Vec<Item> },
    Quote(Vec<Block>),
    Code(String),
    Rule,
    Table { head: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Props(Vec<(String, Vec<Inline>)>),
}

// ---------------------------------------------------------------- frontmatter and headings

static FRONTMATTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^---\r?\n([\s\S]*?)\r?\n---\r?\n?").unwrap_or_else(|_| unreachable!()));
static PROP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([\w-]+):\s*(.*)$").unwrap_or_else(|_| unreachable!()));

pub struct Front<'a> {
    pub props: Vec<(String, String)>,
    pub body: &'a str,
    /// How many lines the frontmatter takes (the body starts after them).
    pub lines: usize,
}

/// Simple frontmatter (key: value), shown like Obsidian "properties".
pub fn split_frontmatter(text: &str) -> Front<'_> {
    let Some(m) = FRONTMATTER.captures(text) else {
        return Front { props: Vec::new(), body: text, lines: 0 };
    };
    let whole = m.get(0).map_or("", |x| x.as_str());
    let inner = m.get(1).map_or("", |x| x.as_str());
    let mut props = Vec::new();
    for line in inner.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(kv) = PROP.captures(line) {
            let v = kv[2].to_string();
            let v = v.strip_prefix(['"', '\'']).unwrap_or(&v);
            let v = v.strip_suffix(['"', '\'']).unwrap_or(v);
            props.push((kv[1].to_string(), v.to_string()));
        }
    }
    Front { props, body: &text[whole.len()..], lines: whole.matches('\n').count() }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub title: String,
    /// 1 = first line of the note.
    pub line: usize,
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

fn level_num(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].matches('\n').count()
}

/// Note headings from the parser itself (ATX, setext, indented; skips code and frontmatter).
/// The title is the raw inline source, like markdown-it's token content.
pub fn headings(text: &str) -> Vec<Heading> {
    let front = split_frontmatter(text);
    let body = front.body;
    let mut out = Vec::new();
    for (ev, range) in Parser::new_ext(body, options()).into_offset_iter() {
        if let Event::Start(Tag::Heading { level, .. }) = ev {
            let src = &body[range.clone()];
            let trimmed = src.trim_start();
            let title = if trimmed.starts_with('#') {
                let first = trimmed.lines().next().unwrap_or("");
                let t = first.trim_start_matches('#').trim();
                // A closing sequence of #s is not part of the title.
                let t2 = t.trim_end_matches('#');
                if t2.len() != t.len() && (t2.is_empty() || t2.ends_with(' ')) { t2.trim().to_string() } else { t.to_string() }
            } else {
                let lines: Vec<&str> = src.trim_end().lines().collect();
                lines[..lines.len().saturating_sub(1)].iter().map(|l| l.trim()).collect::<Vec<_>>().join("\n")
            };
            out.push(Heading { level: level_num(level), title, line: line_of(body, range.start) + front.lines + 1 });
        }
    }
    out
}

// ---------------------------------------------------------------- building the model

/// What the renderer needs from the vault (Preview's fileUrl/exists props).
pub trait Resolver {
    /// Resolves a vault target ([[target]] or relative path) to an image/file key.
    fn file_url(&self, target: &str) -> String;
    /// Whether the target of a [[link]] exists (missing links are dimmed).
    fn exists(&self, target: &str) -> bool;
}

static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:https?://|www\.)[^\s<>]+|\b[\w.+-]+@[\w-]+(?:\.[\w-]+)+\b").unwrap_or_else(|_| unreachable!())
});

/// Bare URLs and e-mails become links (markdown-it's linkify).
fn linkify(text: &str, style: Style, out: &mut Vec<Inline>) {
    let mut last = 0;
    for m in URL.find_iter(text) {
        let mut s = m.as_str();
        while let Some(c) = s.chars().last() {
            if ".,;:!?'\")]".contains(c) {
                s = &s[..s.len() - c.len_utf8()];
            } else {
                break;
            }
        }
        if s.is_empty() {
            continue;
        }
        if m.start() > last {
            out.push(Inline::Text(text[last..m.start()].to_string(), style));
        }
        let href = if s.contains('@') && !s.contains("://") {
            format!("mailto:{s}")
        } else if s.to_lowercase().starts_with("www.") {
            format!("http://{s}")
        } else {
            s.to_string()
        };
        out.push(Inline::Link(vec![Inline::Text(s.to_string(), style)], Target::Href(href)));
        last = m.start() + s.len();
    }
    if last < text.len() {
        out.push(Inline::Text(text[last..].to_string(), style));
    }
}

/// Text with [[wikilinks]] and ![[embeds]] turned into links and images.
pub fn wikilinks(text: &str, style: Style, r: &dyn Resolver, in_link: bool, out: &mut Vec<Inline>) {
    let mut rest = text;
    loop {
        let open = rest.find("[[");
        let Some(start) = open else { break };
        let embed = start > 0 && rest[..start].ends_with('!');
        let inner_start = start + 2;
        let Some(end_rel) = rest[inner_start..].find("]]") else { break };
        let end = inner_start + end_rel;
        let before = &rest[..if embed { start - 1 } else { start }];
        if !before.is_empty() {
            if in_link { out.push(Inline::Text(before.to_string(), style)) } else { linkify(before, style, out) }
        }
        let parsed = parse_wikilink(&rest[inner_start..end]);
        let l = parsed.link;
        if embed && IMAGE.is_match(&l.target) {
            // ![[photo.png|300]]: the number after | is the width, like in Obsidian.
            let width = parsed.alias.as_deref().filter(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_digit())).and_then(|a| a.parse().ok());
            out.push(Inline::Image(ImageRef { key: r.file_url(&l.target), alt: l.target.clone(), width }));
        } else {
            let label = match (&parsed.alias, l.page, &l.cfi) {
                (Some(a), _, _) => a.clone(),
                (None, Some(p), _) => format!("{} · p.{}", l.target, p),
                (None, None, Some(_)) => format!("{} · saved position", l.target),
                _ => parsed.reference.clone(),
            };
            // [[#section]] (empty target) points to the note itself: never "missing".
            let missing = !l.target.is_empty() && !r.exists(&l.target);
            out.push(Inline::Link(
                vec![Inline::Text(label.trim().to_string(), style)],
                Target::Wiki { target: l.target, page: l.page, cfi: l.cfi, missing, embed },
            ));
        }
        rest = &rest[end + 2..];
    }
    if !rest.is_empty() {
        if in_link { out.push(Inline::Text(rest.to_string(), style)) } else { linkify(rest, style, out) }
    }
}

static IMG_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<img\b[^>]*>"#).unwrap_or_else(|_| unreachable!()));
static ATTR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)([a-z-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap_or_else(|_| unreachable!()));
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap_or_else(|_| unreachable!()));

fn attrs(tag: &str) -> HashMap<String, String> {
    ATTR.captures_iter(tag)
        .map(|c| {
            let v = c.get(2).or(c.get(3)).or(c.get(4)).map_or("", |m| m.as_str());
            (c[1].to_lowercase(), v.to_string())
        })
        .collect()
}

fn decode_uri(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// An image source of the note: remote and data: as they are, anything else is a vault file.
fn image_key(src: &str, r: &dyn Resolver) -> String {
    let lower = src.to_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("data:") || lower.starts_with("blob:") {
        src.to_string()
    } else {
        r.file_url(&decode_uri(src))
    }
}

/// Raw HTML in a note: no HTML engine here, so <br> breaks the line, <img> shows the image and
/// the other tags are dropped (the browser app sanitizes them with DOMPurify instead).
fn html_inline(html: &str, r: &dyn Resolver, out: &mut Vec<Inline>) {
    let lower = html.to_lowercase();
    if lower.starts_with("<br") {
        out.push(Inline::Break);
    } else if IMG_TAG.is_match(html) {
        for m in IMG_TAG.find_iter(html) {
            let a = attrs(m.as_str());
            if let Some(src) = a.get("src") {
                out.push(Inline::Image(ImageRef {
                    key: image_key(src, r),
                    alt: a.get("alt").cloned().unwrap_or_default(),
                    width: a.get("width").and_then(|w| w.trim_end_matches("px").parse().ok()),
                }));
            }
        }
    }
}

struct Builder<'r> {
    r: &'r dyn Resolver,
    body: &'r str,
    line_offset: usize,
    /// Stack of block containers being filled.
    stack: Vec<Container>,
    inlines: Vec<Vec<Inline>>,
    style: Style,
    pending_text: String,
    link_stack: Vec<(Target, usize)>,
    image: Option<(String, String)>,
    code: Option<String>,
    table: Option<(Vec<Vec<Inline>>, Vec<Vec<Vec<Inline>>>, bool)>,
    html_block: Option<String>,
    /// The line of the list item being built (for task checkboxes).
    item_line: usize,
}

enum Container {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    List { ordered: bool, start: u64, items: Vec<Item> },
    Item(Item),
}

impl Container {
    fn blocks(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Container::Root(b) | Container::Quote(b) => Some(b),
            Container::Item(i) => Some(&mut i.blocks),
            Container::List { .. } => None,
        }
    }
}

impl<'r> Builder<'r> {
    fn push_block(&mut self, b: Block) {
        for c in self.stack.iter_mut().rev() {
            if let Some(blocks) = c.blocks() {
                blocks.push(b);
                return;
            }
        }
    }

    fn flush_text(&mut self) {
        if self.pending_text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending_text);
        let in_link = !self.link_stack.is_empty();
        if let Some(cur) = self.inlines.last_mut() {
            if self.style.code {
                cur.push(Inline::Text(text, self.style));
            } else {
                wikilinks(&text, self.style, self.r, in_link, cur);
            }
        }
    }

    fn inline(&mut self, i: Inline) {
        self.flush_text();
        if let Some(cur) = self.inlines.last_mut() {
            cur.push(i);
        }
    }

    fn begin_inlines(&mut self) {
        self.inlines.push(Vec::new());
    }

    fn end_inlines(&mut self) -> Vec<Inline> {
        self.flush_text();
        self.inlines.pop().unwrap_or_default()
    }

    /// Inline content outside a paragraph (tight list items): wrap it in one.
    fn loose_inlines(&mut self) {
        if self.inlines.is_empty() {
            self.begin_inlines();
        }
    }

    fn close_loose(&mut self) {
        if !self.inlines.is_empty() && self.table.is_none() {
            let content = self.end_inlines();
            if !content.is_empty() {
                self.push_block(Block::Para(content));
            }
        }
    }

    fn event(&mut self, ev: Event<'_>, range: Range<usize>) {
        match ev {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some(code) = &mut self.code {
                    code.push_str(&t);
                } else if let Some((_, alt)) = &mut self.image {
                    alt.push_str(&t);
                } else {
                    self.loose_inlines();
                    self.pending_text.push_str(&t);
                }
            }
            Event::Code(t) => {
                self.loose_inlines();
                let style = Style { code: true, ..self.style };
                self.inline(Inline::Text(t.to_string(), style));
            }
            Event::SoftBreak | Event::HardBreak => {
                self.loose_inlines();
                // breaks: true — a newline in the note is a line break, like Obsidian.
                self.inline(Inline::Break);
            }
            Event::Rule => {
                self.close_loose();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                let line = self.item_line;
                for c in self.stack.iter_mut().rev() {
                    if let Container::Item(it) = c {
                        it.task = Some((checked, line));
                        break;
                    }
                }
            }
            Event::InlineHtml(h) => {
                self.loose_inlines();
                self.flush_text();
                let mut out = Vec::new();
                html_inline(&h, self.r, &mut out);
                for i in out {
                    self.inline(i);
                }
            }
            Event::Html(h) => {
                if let Some(b) = &mut self.html_block {
                    b.push_str(&h);
                }
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Paragraph => {
                self.close_loose();
                self.begin_inlines();
            }
            Tag::Heading { .. } => {
                self.close_loose();
                self.begin_inlines();
            }
            Tag::BlockQuote(_) => {
                self.close_loose();
                self.stack.push(Container::Quote(Vec::new()));
            }
            Tag::CodeBlock(_) => {
                self.close_loose();
                self.code = Some(String::new());
            }
            Tag::HtmlBlock => {
                self.close_loose();
                self.html_block = Some(String::new());
            }
            Tag::List(start) => {
                self.close_loose();
                self.stack.push(Container::List { ordered: start.is_some(), start: start.unwrap_or(1), items: Vec::new() });
            }
            Tag::Item => {
                self.close_loose();
                self.item_line = line_of(self.body, range.start) + self.line_offset + 1;
                self.stack.push(Container::Item(Item { task: None, blocks: Vec::new() }));
            }
            Tag::Table(_) => {
                self.close_loose();
                self.table = Some((Vec::new(), Vec::new(), false));
            }
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.2 = true;
                }
            }
            Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.1.push(Vec::new());
                }
            }
            Tag::TableCell => self.begin_inlines(),
            Tag::Emphasis => {
                self.loose_inlines();
                self.flush_text();
                self.style.italic = true;
            }
            Tag::Strong => {
                self.loose_inlines();
                self.flush_text();
                self.style.bold = true;
            }
            Tag::Strikethrough => {
                self.loose_inlines();
                self.flush_text();
                self.style.strike = true;
            }
            Tag::Link { dest_url, .. } => {
                self.loose_inlines();
                self.flush_text();
                let depth = self.inlines.last().map_or(0, Vec::len);
                self.link_stack.push((Target::Href(dest_url.to_string()), depth));
            }
            Tag::Image { dest_url, .. } => {
                self.loose_inlines();
                self.flush_text();
                self.image = Some((dest_url.to_string(), String::new()));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                let content = self.end_inlines();
                if !content.is_empty() {
                    self.push_block(Block::Para(content));
                }
            }
            TagEnd::Heading(level) => {
                let content = self.end_inlines();
                self.push_block(Block::Heading(level_num(level), content));
            }
            TagEnd::BlockQuote(_) => {
                self.close_loose();
                if let Some(Container::Quote(b)) = self.stack.pop() {
                    self.push_block(Block::Quote(b));
                }
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.push_block(Block::Code(code.trim_end_matches('\n').to_string()));
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(h) = self.html_block.take() {
                    let mut inl = Vec::new();
                    if IMG_TAG.is_match(&h) {
                        html_inline(&h, self.r, &mut inl);
                    } else {
                        let text = TAG.replace_all(&h, "");
                        let text = text.trim();
                        if !text.is_empty() {
                            inl.push(Inline::Text(html_unescape(text), Style::default()));
                        }
                    }
                    if !inl.is_empty() {
                        self.push_block(Block::Para(inl));
                    }
                }
            }
            TagEnd::List(_) => {
                self.close_loose();
                if let Some(Container::List { ordered, start, items }) = self.stack.pop() {
                    self.push_block(Block::List { ordered, start, items });
                }
            }
            TagEnd::Item => {
                self.close_loose();
                if let Some(Container::Item(it)) = self.stack.pop() {
                    if let Some(Container::List { items, .. }) = self.stack.last_mut() {
                        items.push(it);
                    }
                }
            }
            TagEnd::TableCell => {
                let cell = self.end_inlines();
                if let Some((head, rows, in_head)) = &mut self.table {
                    if *in_head {
                        head.push(cell);
                    } else if let Some(row) = rows.last_mut() {
                        row.push(cell);
                    }
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.2 = false;
                }
            }
            TagEnd::Table => {
                if let Some((head, rows, _)) = self.table.take() {
                    self.push_block(Block::Table { head, rows });
                }
            }
            TagEnd::Emphasis => {
                self.flush_text();
                self.style.italic = false;
            }
            TagEnd::Strong => {
                self.flush_text();
                self.style.bold = false;
            }
            TagEnd::Strikethrough => {
                self.flush_text();
                self.style.strike = false;
            }
            TagEnd::Link => {
                self.flush_text();
                if let Some((target, depth)) = self.link_stack.pop() {
                    if let Some(cur) = self.inlines.last_mut() {
                        let content: Vec<Inline> = cur.drain(depth.min(cur.len())..).collect();
                        cur.push(Inline::Link(content, target));
                    }
                }
            }
            TagEnd::Image => {
                if let Some((src, alt)) = self.image.take() {
                    let key = image_key(&src, self.r);
                    self.inline(Inline::Image(ImageRef { key, alt, width: None }));
                }
            }
            _ => {}
        }
    }
}

fn html_unescape(s: &str) -> String {
    s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// The note as blocks: the properties box first, then the markdown body.
pub fn parse(text: &str, r: &dyn Resolver) -> Vec<Block> {
    let front = split_frontmatter(text);
    let mut blocks = Vec::new();
    if !front.props.is_empty() {
        let props = front
            .props
            .iter()
            .map(|(k, v)| {
                let mut inl = Vec::new();
                inline_markdown(v, r, &mut inl);
                (k.clone(), inl)
            })
            .collect();
        blocks.push(Block::Props(props));
    }
    let mut b = BuilderWithLine::new(r, front.body, front.lines);
    for (ev, range) in Parser::new_ext(front.body, options()).into_offset_iter() {
        b.0.event(ev, range);
    }
    b.0.close_loose();
    if let Some(Container::Root(body)) = b.0.stack.pop() {
        blocks.extend(body);
    }
    blocks
}

/// markdown-it's renderInline: the inline content of one line (frontmatter values).
pub fn inline_markdown(text: &str, r: &dyn Resolver, out: &mut Vec<Inline>) {
    let blocks = {
        let mut b = BuilderWithLine::new(r, text, 0);
        for (ev, range) in Parser::new_ext(text, options()).into_offset_iter() {
            b.0.event(ev, range);
        }
        b.0.close_loose();
        match b.0.stack.pop() {
            Some(Container::Root(body)) => body,
            _ => Vec::new(),
        }
    };
    for blk in blocks {
        match blk {
            Block::Para(i) | Block::Heading(_, i) => out.extend(i),
            _ => {}
        }
    }
}

struct BuilderWithLine<'r>(Builder<'r>);

impl<'r> BuilderWithLine<'r> {
    fn new(r: &'r dyn Resolver, body: &'r str, line_offset: usize) -> Self {
        BuilderWithLine(Builder {
            r,
            body,
            line_offset,
            stack: vec![Container::Root(Vec::new())],
            inlines: Vec::new(),
            style: Style::default(),
            pending_text: String::new(),
            link_stack: Vec::new(),
            image: None,
            code: None,
            table: None,
            html_block: None,
            item_line: 0,
        })
    }
}

// ---------------------------------------------------------------- images

pub enum ImgState {
    Loading,
    Ready(egui::TextureHandle, Vec2),
    Failed,
}

/// Textures of the images shown by the preview and the EPUB reader, by key. Missing keys are
/// collected in `wanted`; the owner fetches the bytes and calls `insert`.
#[derive(Default)]
pub struct Images {
    map: HashMap<String, ImgState>,
    pub wanted: Vec<String>,
}

/// Largest side kept in memory: the preview column is ~840 px wide.
const MAX_SIDE: u32 = 2048;

pub fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > MAX_SIDE || img.height() > MAX_SIDE { img.thumbnail(MAX_SIDE, MAX_SIDE) } else { img };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

pub fn data_uri(uri: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let (_, data) = uri.split_once(";base64,")?;
    base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()
}

impl Images {
    fn get(&mut self, key: &str) -> Option<&ImgState> {
        if !self.map.contains_key(key) {
            self.map.insert(key.to_string(), ImgState::Loading);
            self.wanted.push(key.to_string());
        }
        self.map.get(key)
    }

    pub fn insert(&mut self, ctx: &egui::Context, key: String, img: Option<egui::ColorImage>) {
        let state = match img {
            Some(img) => {
                let size = Vec2::new(img.size[0] as f32, img.size[1] as f32);
                ImgState::Ready(ctx.load_texture(&key, img, egui::TextureOptions::LINEAR), size)
            }
            None => ImgState::Failed,
        };
        self.map.insert(key, state);
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.wanted.clear();
    }

    /// Drops the textures not in `keep` (switching notes/chapters frees their images).
    pub fn retain(&mut self, keep: &dyn Fn(&str) -> bool) {
        self.map.retain(|k, s| keep(k) || matches!(s, ImgState::Loading));
    }
}

// ---------------------------------------------------------------- drawing

/// Colors and sizes of the article (Preview.svelte's styles, or the book's).
pub struct Look {
    pub size: f32,
    pub text: Color32,
    pub muted: Color32,
    pub heading: Color32,
    pub link: Color32,
    pub border: Color32,
    pub code_bg: Color32,
    pub accent: Color32,
    pub line_height: f32,
    /// Headings in the heading color (notes) or the text color (books).
    pub colored_headings: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    Link(Target),
    Task(usize),
}

pub struct Painter<'a> {
    pub look: &'a Look,
    pub images: &'a mut Images,
    pub click: Option<Click>,
    pub hovered_link: bool,
}

fn format(look: &Look, style: Style, size: f32, color: Color32) -> TextFormat {
    let mut f = if style.code {
        let mut f = TextFormat::simple(fonts::mono(size * 0.8), color);
        f.background = look.code_bg;
        f
    } else {
        TextFormat::simple(fonts::font(fonts::Role::Editor, style.bold, style.italic, if style.small { size * 0.8 } else { size }), color)
    };
    f.line_height = Some(size * look.line_height);
    if style.strike {
        f.strikethrough = Stroke::new(1.0, color);
    }
    f
}

/// Lays inline content out into one job; links keep their character ranges for clicks.
fn layout(look: &Look, inl: &[Inline], size: f32, color: Color32, base: Style, job: &mut LayoutJob, links: &mut Vec<(Range<usize>, Target)>, chars: &mut usize) {
    for i in inl {
        match i {
            Inline::Text(t, s) => {
                let s = Style { bold: s.bold || base.bold, italic: s.italic || base.italic, ..*s };
                job.append(t, 0.0, format(look, s, size, color));
                *chars += t.chars().count();
            }
            Inline::Break => {
                job.append("\n", 0.0, format(look, base, size, color));
                *chars += 1;
            }
            Inline::Link(content, target) => {
                let start = *chars;
                let (c, missing, embed) = match target {
                    Target::Wiki { missing, embed, .. } => {
                        (if *missing { look.link.gamma_multiply(0.6) } else { look.link }, *missing, *embed)
                    }
                    Target::Href(_) => (look.link, false, false),
                };
                let first = job.sections.len();
                let s = Style { italic: base.italic || embed, ..base };
                layout(look, content, size, c, s, job, links, chars);
                // border-bottom: 1px solid link color at 35% (dashed and dimmer when missing).
                let underline = Stroke::new(1.0, c.gamma_multiply(if missing { 0.5 } else { 0.35 }));
                for sec in &mut job.sections[first..] {
                    sec.format.underline = underline;
                }
                links.push((start..*chars, target.clone()));
            }
            // Images are drawn as their own rows (see `runs`).
            Inline::Image(_) => {}
        }
    }
}

/// Splits inline content at images: text runs and images, drawn one after the other.
fn runs(inl: &[Inline]) -> Vec<Result<Vec<Inline>, ImageRef>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for i in inl {
        match i {
            Inline::Image(img) => {
                if !cur.is_empty() {
                    out.push(Ok(std::mem::take(&mut cur)));
                }
                out.push(Err(img.clone()));
            }
            other => cur.push(other.clone()),
        }
    }
    if !cur.is_empty() {
        out.push(Ok(cur));
    }
    out
}

fn trim_breaks(mut v: Vec<Inline>) -> Vec<Inline> {
    while matches!(v.first(), Some(Inline::Break)) {
        v.remove(0);
    }
    while matches!(v.last(), Some(Inline::Break)) {
        v.pop();
    }
    v
}

impl Painter<'_> {
    /// One paragraph-like run of text: selectable, with clickable links.
    pub fn text(&mut self, ui: &mut Ui, inl: &[Inline], size: f32, color: Color32, base: Style, width: f32) -> Response {
        let mut job = LayoutJob::default();
        let mut links = Vec::new();
        let mut chars = 0;
        layout(self.look, inl, size, color, base, &mut job, &mut links, &mut chars);
        job.wrap.max_width = width;
        let label = egui::Label::new(job).sense(Sense::click()).wrap();
        let (pos, galley, resp) = label.layout_in_ui(ui);
        let hover_link = resp.hover_pos().and_then(|p| link_at(&galley, p - pos, &links));
        if hover_link.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            self.hovered_link = true;
        }
        if resp.clicked() {
            if let Some(t) = resp.interact_pointer_pos().and_then(|p| link_at(&galley, p - pos, &links)) {
                self.click = Some(Click::Link(t));
            }
        }
        egui::text_selection::LabelSelectionState::label_text_selection(ui, &resp, pos, galley, color, Stroke::NONE);
        resp
    }

    fn image(&mut self, ui: &mut Ui, img: &ImageRef, width: f32) {
        let look = self.look;
        match self.images.get(&img.key) {
            Some(ImgState::Ready(tex, size)) => {
                let w = img.width.unwrap_or(size.x).min(width).max(1.0);
                let h = size.y * w / size.x.max(1.0);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
                egui::Image::new((tex.id(), Vec2::new(w, h))).corner_radius(4).paint_at(ui, rect);
            }
            Some(ImgState::Loading) => {
                ui.add_space(4.0);
            }
            _ => {
                let alt = if img.alt.is_empty() { "image" } else { &img.alt };
                ui.label(egui::RichText::new(format!("[{alt}]")).font(fonts::editor(look.size * 0.8)).color(look.muted).italics());
            }
        }
    }

    fn inlines(&mut self, ui: &mut Ui, inl: &[Inline], size: f32, color: Color32, base: Style, width: f32) {
        for run in runs(inl) {
            match run {
                Ok(text) => {
                    let text = trim_breaks(text);
                    if !text.is_empty() {
                        self.text(ui, &text, size, color, base, width);
                    }
                }
                Err(img) => self.image(ui, &img, width),
            }
        }
    }

    /// Draws the blocks top to bottom; returns the top y of each one (for the EPUB position).
    pub fn blocks(&mut self, ui: &mut Ui, blocks: &[Block], width: f32, depth: usize) -> Vec<f32> {
        let look = self.look;
        let size = look.size;
        let gap = size * 0.8;
        let mut tops = Vec::with_capacity(blocks.len());
        for (i, b) in blocks.iter().enumerate() {
            tops.push(ui.cursor().top());
            let last = i + 1 == blocks.len();
            match b {
                Block::Para(inl) => {
                    self.inlines(ui, inl, size, look.text, Style::default(), width);
                    if !last || depth == 0 {
                        ui.add_space(gap);
                    }
                }
                Block::Heading(level, inl) => {
                    let scale = match level {
                        1 => 1.8,
                        2 => 1.45,
                        3 => 1.2,
                        _ => 1.0,
                    };
                    let hs = size * scale;
                    ui.add_space(hs * 1.2);
                    let color = if look.colored_headings { look.heading } else { look.text };
                    let tmp = Look { line_height: 1.25, ..clone_look(look) };
                    let mut p = Painter { look: &tmp, images: self.images, click: None, hovered_link: false };
                    p.inlines(ui, inl, hs, color, Style { bold: true, ..Style::default() }, width);
                    if p.click.is_some() {
                        self.click = p.click;
                    }
                    self.hovered_link |= p.hovered_link;
                    ui.add_space(hs * 0.4);
                }
                Block::Quote(inner) => {
                    let start = ui.cursor().top();
                    let x = ui.cursor().left();
                    let pad = size;
                    let inner_w = width - pad - 3.0;
                    ui.add_space(size * 0.1);
                    let r = ui.horizontal(|ui| {
                        ui.add_space(3.0 + pad);
                        ui.vertical(|ui| {
                            ui.set_width(inner_w);
                            let tmp = Look { text: look.muted, ..clone_look(look) };
                            let mut p = Painter { look: &tmp, images: self.images, click: None, hovered_link: false };
                            p.quote_blocks(ui, inner, inner_w, depth + 1);
                            if p.click.is_some() {
                                self.click = p.click;
                            }
                            self.hovered_link |= p.hovered_link;
                        });
                    });
                    let end = r.response.rect.bottom() + size * 0.1;
                    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x, start), Pos2::new(x + 3.0, end)), 0.0, look.border);
                    ui.add_space(size * 0.1 + gap);
                }
                Block::Code(code) => {
                    let frame = egui::Frame::new().fill(look.code_bg).corner_radius(6).inner_margin(egui::Margin::symmetric(
                        size as i8,
                        (size * 0.8) as i8,
                    ));
                    frame.show(ui, |ui| {
                        ui.set_width(width - 2.0 * size);
                        egui::ScrollArea::horizontal().id_salt(("code", ui.cursor().top() as i64, i)).show(ui, |ui| {
                            let mut job = LayoutJob::simple(code.clone(), fonts::mono(size * 0.8), look.text, f32::INFINITY);
                            job.sections.iter_mut().for_each(|s| s.format.line_height = Some(size * 0.8 * 1.5));
                            ui.add(egui::Label::new(job).extend());
                        });
                    });
                    ui.add_space(gap);
                }
                Block::Rule => {
                    ui.add_space(size * 1.5);
                    let y = ui.cursor().top();
                    ui.painter().hline(ui.cursor().left()..=ui.cursor().left() + width, y, Stroke::new(1.0, look.border));
                    ui.add_space(size * 1.5);
                }
                Block::List { ordered, start, items } => {
                    self.list(ui, *ordered, *start, items, width, depth);
                    if depth == 0 || !last {
                        ui.add_space(gap);
                    }
                }
                Block::Table { head, rows } => {
                    self.table(ui, head, rows, width);
                    ui.add_space(gap);
                }
                Block::Props(props) => {
                    self.props(ui, props, width);
                    ui.add_space(size * 1.4);
                }
            }
        }
        tops
    }

    fn quote_blocks(&mut self, ui: &mut Ui, blocks: &[Block], width: f32, depth: usize) {
        // Quotes are italic: draw their paragraphs with an italic base style.
        for (i, b) in blocks.iter().enumerate() {
            match b {
                Block::Para(inl) => {
                    let look = self.look;
                    self.inlines(ui, inl, look.size, look.text, Style { italic: true, ..Style::default() }, width);
                    if i + 1 < blocks.len() {
                        ui.add_space(look.size * 0.8);
                    }
                }
                other => {
                    self.blocks(ui, std::slice::from_ref(other), width, depth);
                }
            }
        }
    }

    fn list(&mut self, ui: &mut Ui, ordered: bool, start: u64, items: &[Item], width: f32, depth: usize) {
        let look = self.look;
        let size = look.size;
        let indent = size * 1.4;
        for (n, item) in items.iter().enumerate() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let marker_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(indent, size * look.line_height));
                if let Some((checked, line)) = item.task {
                    // li.task: no bullet, the checkbox where the bullet would be.
                    let box_size = size * 0.7;
                    let r = Rect::from_center_size(
                        Pos2::new(marker_rect.left() + box_size / 2.0 + 1.0, marker_rect.center().y),
                        Vec2::splat(box_size),
                    );
                    let resp = ui.interact(r.expand(3.0), ui.id().with(("task", line)), Sense::click());
                    let stroke = Stroke::new(1.2, if resp.hovered() { look.accent } else { look.muted });
                    if checked {
                        ui.painter().rect_filled(r, 3, look.accent);
                        let a = r.left_center() + Vec2::new(box_size * 0.2, 0.0);
                        let b = r.center_bottom() + Vec2::new(-box_size * 0.08, -box_size * 0.25);
                        let c = r.right_top() + Vec2::new(-box_size * 0.2, box_size * 0.25);
                        let s = Stroke::new(2.0, Color32::WHITE);
                        ui.painter().line_segment([a, b], s);
                        ui.painter().line_segment([b, c], s);
                    } else {
                        ui.painter().rect_stroke(r, 3, stroke, egui::StrokeKind::Inside);
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        self.click = Some(Click::Task(line));
                    }
                } else {
                    let marker = if ordered {
                        format!("{}.", start + n as u64)
                    } else {
                        match depth % 3 {
                            0 => "•".into(),
                            1 => "◦".into(),
                            _ => "▪".into(),
                        }
                    };
                    ui.painter().text(
                        Pos2::new(marker_rect.right() - size * 0.35, marker_rect.center().y),
                        Align2::RIGHT_CENTER,
                        marker,
                        fonts::editor(size),
                        look.text,
                    );
                }
                ui.allocate_space(Vec2::new(indent, 1.0));
                ui.vertical(|ui| {
                    ui.set_width(width - indent);
                    if item.blocks.is_empty() {
                        ui.allocate_space(Vec2::new(1.0, size * look.line_height));
                    }
                    self.blocks(ui, &item.blocks, width - indent, depth + 1);
                });
            });
        }
    }

    fn table(&mut self, ui: &mut Ui, head: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], width: f32) {
        let look = self.look;
        let size = look.size * 0.9;
        let cols = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
        if cols == 0 {
            return;
        }
        let pad = Vec2::new(size * 0.6, size * 0.3);
        // Natural width of every column, then shrunk to fit.
        let measure = |ui: &Ui, inl: &[Inline], bold: bool| -> f32 {
            let mut job = LayoutJob::default();
            let (mut links, mut chars) = (Vec::new(), 0);
            layout(look, inl, size, look.text, Style { bold, ..Style::default() }, &mut job, &mut links, &mut chars);
            ui.fonts_mut(|f| f.layout_job(job)).size().x
        };
        let mut widths = vec![0.0f32; cols];
        for (c, cell) in head.iter().enumerate() {
            widths[c] = widths[c].max(measure(ui, cell, true));
        }
        for row in rows {
            for (c, cell) in row.iter().enumerate() {
                widths[c] = widths[c].max(measure(ui, cell, false));
            }
        }
        let total: f32 = widths.iter().map(|w| w + 2.0 * pad.x).sum();
        if total > width {
            let scale = (width - cols as f32 * 2.0 * pad.x).max(cols as f32 * 20.0) / widths.iter().sum::<f32>().max(1.0);
            widths.iter_mut().for_each(|w| *w = (*w * scale).max(20.0));
        }
        let mut draw_row = |ui: &mut Ui, cells: &[Vec<Inline>], bold: bool, this: &mut Self| {
            let top = ui.cursor().top();
            let left = ui.cursor().left();
            let mut x = left;
            let mut bottom = top;
            let mut rects = Vec::new();
            for (c, w) in widths.iter().enumerate() {
                let rect = Rect::from_min_size(Pos2::new(x, top), Vec2::new(w + 2.0 * pad.x, 0.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min + pad, Vec2::new(*w, 10_000.0))));
                if let Some(cell) = cells.get(c) {
                    let tmp = Look { size, line_height: 1.6, ..clone_look(look) };
                    let mut p = Painter { look: &tmp, images: this.images, click: None, hovered_link: false };
                    p.inlines(&mut child, cell, size, look.text, Style { bold, ..Style::default() }, *w);
                    if p.click.is_some() {
                        this.click = p.click;
                    }
                }
                bottom = bottom.max(child.min_rect().bottom() + pad.y);
                rects.push(rect);
                x += w + 2.0 * pad.x;
            }
            for r in rects {
                let r = Rect::from_min_max(r.min, Pos2::new(r.max.x, bottom));
                ui.painter().rect_stroke(r, 0, Stroke::new(1.0, look.border), egui::StrokeKind::Inside);
            }
            ui.allocate_rect(Rect::from_min_max(Pos2::new(left, top), Pos2::new(x, bottom)), Sense::hover());
        };
        ui.spacing_mut().item_spacing.y = 0.0;
        if !head.is_empty() {
            draw_row(ui, head, true, self);
        }
        for row in rows {
            draw_row(ui, row, false, self);
        }
    }

    fn props(&mut self, ui: &mut Ui, props: &[(String, Vec<Inline>)], width: f32) {
        let look = self.look;
        let size = look.size * 0.8;
        let frame = egui::Frame::new()
            .stroke(Stroke::new(1.0, look.border))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric((size * 0.9) as i8, (size * 0.6) as i8));
        frame.show(ui, |ui| {
            ui.set_width(width - 2.0 * size * 0.9);
            let key_w = props
                .iter()
                .map(|(k, _)| ui.fonts_mut(|f| f.layout_no_wrap(k.clone(), fonts::mono(size * 0.85), look.muted)).size().x)
                .fold(0.0, f32::max);
            for (k, v) in props {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(14.0, 2.0);
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(key_w, size * 1.6), Sense::hover());
                    ui.painter().text(
                        Pos2::new(rect.left(), rect.top() + size * 0.2),
                        Align2::LEFT_TOP,
                        k,
                        fonts::mono(size * 0.85),
                        look.muted,
                    );
                    let w = ui.available_width();
                    let tmp = Look { size, ..clone_look(look) };
                    let mut p = Painter { look: &tmp, images: self.images, click: None, hovered_link: false };
                    p.inlines(ui, v, size, look.muted, Style::default(), w);
                    if p.click.is_some() {
                        self.click = p.click;
                    }
                    self.hovered_link |= p.hovered_link;
                });
            }
        });
    }
}

fn clone_look(l: &Look) -> Look {
    Look {
        size: l.size,
        text: l.text,
        muted: l.muted,
        heading: l.heading,
        link: l.link,
        border: l.border,
        code_bg: l.code_bg,
        accent: l.accent,
        line_height: l.line_height,
        colored_headings: l.colored_headings,
    }
}

/// The link under a point of a laid-out text, if the point is really over its glyphs.
fn link_at(galley: &egui::Galley, pos: Vec2, links: &[(Range<usize>, Target)]) -> Option<Target> {
    if links.is_empty() || !galley.rect.contains(pos.to_pos2()) {
        return None;
    }
    // cursor_from_pos gives the nearest gap between characters: the one under the point is the
    // character after the gap when the point is right of it, the one before otherwise.
    let c: CCursor = galley.cursor_from_pos(pos);
    let r = galley.pos_from_cursor(c);
    let idx = if pos.x >= r.left() { c.index.0 } else { c.index.0.saturating_sub(1) };
    let row = galley.rows.iter().find(|row| row.rect().y_range().contains(pos.y))?;
    if pos.x > row.rect().right() {
        return None;
    }
    links.iter().find(|(range, _)| range.contains(&idx)).map(|(_, t)| t.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct R;
    impl Resolver for R {
        fn file_url(&self, t: &str) -> String {
            format!("/f/{t}")
        }
        fn exists(&self, t: &str) -> bool {
            t == "There"
        }
    }

    #[test]
    fn frontmatter_props_and_lines() {
        let f = split_frontmatter("---\nsource: \"[[b.pdf]]\"\ntags: x\n---\n\n# T\n");
        assert_eq!(f.props, vec![("source".into(), "[[b.pdf]]".into()), ("tags".into(), "x".into())]);
        assert_eq!(f.lines, 4);
        assert_eq!(f.body, "\n# T\n");
    }

    #[test]
    fn headings_skip_code_and_count_frontmatter() {
        let h = headings("---\na: 1\n---\n# One\n```\n# not\n```\nTwo\n---\n## Three ##\n");
        assert_eq!(
            h,
            vec![
                Heading { level: 1, title: "One".into(), line: 4 },
                Heading { level: 2, title: "Two".into(), line: 8 },
                Heading { level: 2, title: "Three".into(), line: 10 },
            ]
        );
    }

    #[test]
    fn wikilinks_tasks_and_embeds() {
        let blocks = parse("- [ ] a [[There|x]]\n- [x] b [[Gone]]\n\n![[p.png|300]]", &R);
        let Block::List { items, .. } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(items[0].task, Some((false, 1)));
        assert_eq!(items[1].task, Some((true, 2)));
        let Block::Para(inl) = &items[1].blocks[0] else { panic!() };
        assert!(inl.iter().any(|i| matches!(i, Inline::Link(_, Target::Wiki { missing: true, .. }))));
        let Block::Para(inl) = &blocks[1] else { panic!() };
        assert_eq!(inl[0], Inline::Image(ImageRef { key: "/f/p.png".into(), alt: "p.png".into(), width: Some(300.0) }));
    }

    #[test]
    fn raw_html_images_and_blocks() {
        let blocks = parse("a<br>b <img src=\"x.png\" width=\"20\">\n\n<div>hi <b>there</b></div>\n", &R);
        let Block::Para(inl) = &blocks[0] else { panic!("{blocks:?}") };
        assert!(inl.contains(&Inline::Break));
        assert!(inl.iter().any(|i| matches!(i, Inline::Image(img) if img.key == "/f/x.png" && img.width == Some(20.0))));
        assert_eq!(blocks[1], Block::Para(vec![Inline::Text("hi there".into(), Style::default())]));
    }

    #[test]
    fn page_links_get_a_label_and_urls_are_linked() {
        let blocks = parse("see [[b.pdf#page=3]] and https://x.org.", &R);
        let Block::Para(inl) = &blocks[0] else { panic!() };
        assert!(inl.iter().any(|i| matches!(i, Inline::Link(c, Target::Wiki { page: Some(3), .. }) if c == &vec![Inline::Text("b.pdf · p.3".into(), Style::default())])));
        assert!(inl.iter().any(|i| matches!(i, Inline::Link(_, Target::Href(h)) if h == "https://x.org")));
    }
}
