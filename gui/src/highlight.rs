// Markdown highlighting of the editor, like CodeMirror's markdown mode with the app's themes
// (web/src/lib/theme.ts): larger bold headings, faint markup characters (# * - `), italic quotes,
// code in mono at 0.8em, YAML frontmatter, links. Line based, with the state of fenced code
// blocks and the frontmatter carried from line to line.

use std::ops::Range;

use eframe::egui::{Color32, Stroke, TextFormat, text::LayoutJob};

use crate::fonts::{self, Role};
use crate::theme::EditorColors;

#[derive(Clone, Copy, PartialEq)]
struct Span {
    color: Color32,
    scale: f32,
    bold: bool,
    italic: bool,
    strike: bool,
    mono: bool,
    underline: bool,
}

pub struct Highlighted {
    pub job: LayoutJob,
    /// Character ranges of the lines of fenced code blocks (drawn on their own background).
    pub code_lines: Vec<Range<usize>>,
}

struct Ctx<'a> {
    c: &'a EditorColors,
    size: f32,
    job: LayoutJob,
}

impl Ctx<'_> {
    fn base(&self) -> Span {
        Span { color: self.c.text, scale: 1.0, bold: false, italic: false, strike: false, mono: false, underline: false }
    }

    fn push(&mut self, text: &str, s: Span) {
        if text.is_empty() {
            return;
        }
        let size = if s.mono { self.size * 0.8 * s.scale } else { self.size * s.scale };
        let font = if s.mono { fonts::mono(size) } else { fonts::font(Role::Editor, s.bold, s.italic, size) };
        let mut f = TextFormat::simple(font, s.color);
        f.line_height = Some(size * 1.6);
        if s.strike {
            f.strikethrough = Stroke::new(1.0, s.color);
        }
        if s.underline {
            f.underline = Stroke::new(1.0, s.color);
        }
        self.job.append(text, 0.0, f);
    }
}

fn heading_scale(level: usize) -> f32 {
    match level {
        1 => 1.45,
        2 => 1.25,
        3 => 1.1,
        _ => 1.0,
    }
}

fn is_fence(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = t.chars().take_while(|&x| x == c).count();
    (n >= 3).then_some((c, n))
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn setext_level(next: Option<&str>) -> Option<usize> {
    let n = next?.trim_end();
    let t = n.trim_start_matches(' ');
    if n.len() - t.len() > 3 || t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c == '=') {
        Some(1)
    } else if t.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
}

/// Highlights the whole note. `text` must be exactly the editor buffer.
pub fn highlight(text: &str, c: &EditorColors, size: f32) -> Highlighted {
    let mut cx = Ctx { c, size, job: LayoutJob::default() };
    let mut code_lines = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut fence: Option<(char, usize)> = None;
    // YAML frontmatter only at the very start: --- ... ---
    let mut front = lines.first().is_some_and(|l| l.trim_end() == "---") && lines.len() > 1 && lines[1..].iter().any(|l| l.trim_end() == "---");
    let mut char_pos = 0usize;
    let mut para_open = false;
    for (i, line) in lines.iter().enumerate() {
        let last = i + 1 == lines.len();
        let len = line.chars().count();
        let base = cx.base();
        let mut nl_span = base;
        if front {
            if i > 0 && line.trim_end() == "---" {
                front = false;
            }
            yaml_line(&mut cx, line, i == 0 || !front);
            nl_span = Span { color: c.meta, ..base };
        } else if let Some((fc, n)) = fence {
            let mono = Span { color: c.monospace, mono: true, ..base };
            if let Some((c2, n2)) = is_fence(line) {
                if c2 == fc && n2 >= n && line.trim().chars().all(|x| x == fc) {
                    fence = None;
                    cx.push(line, Span { color: c.marks, ..mono });
                } else {
                    cx.push(line, mono);
                }
            } else {
                cx.push(line, mono);
            }
            code_lines.push(char_pos..char_pos + len);
            nl_span = mono;
            para_open = false;
        } else if let Some(f) = is_fence(line) {
            fence = Some(f);
            let mono = Span { color: c.monospace, mono: true, ..base };
            let t = line.trim_start();
            let marks_len = line.len() - t.len() + f.1 * f.0.len_utf8();
            cx.push(&line[..marks_len], Span { color: c.marks, ..mono });
            cx.push(&line[marks_len..], Span { color: c.meta, ..mono });
            code_lines.push(char_pos..char_pos + len);
            nl_span = mono;
            para_open = false;
        } else if let Some(level) = atx_level(line) {
            let hs = Span { color: c.heading, scale: heading_scale(level), bold: true, ..base };
            let t = line.trim_start();
            let indent = line.len() - t.len();
            let hashes = indent + level;
            cx.push(&line[..hashes], Span { color: c.marks, ..hs });
            inline(&mut cx, &line[hashes..], hs);
            nl_span = hs;
            para_open = false;
        } else if !line.trim().is_empty() && !para_open && setext_level(lines.get(i + 1).copied()).is_some() && !is_rule(line) && !line.trim_start().starts_with('>') {
            let level = setext_level(lines.get(i + 1).copied()).unwrap_or(1);
            let hs = Span { color: c.heading, scale: heading_scale(level), bold: true, ..base };
            inline(&mut cx, line, hs);
            nl_span = hs;
            para_open = true;
        } else if para_open && setext_level(Some(line)).is_some() && i > 0 && !lines[i - 1].trim().is_empty() {
            cx.push(line, Span { color: c.marks, ..base });
            para_open = false;
        } else if is_rule(line) {
            cx.push(line, Span { color: c.marks, ..base });
            para_open = false;
        } else {
            block_line(&mut cx, line, base);
            para_open = !line.trim().is_empty();
        }
        if !last {
            cx.push("\n", nl_span);
        }
        char_pos += len + 1;
    }
    Highlighted { job: cx.job, code_lines }
}

fn atx_level(line: &str) -> Option<usize> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let n = t.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&n) {
        return None;
    }
    let rest = &t[n..];
    (rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')).then_some(n)
}

fn yaml_line(cx: &mut Ctx<'_>, line: &str, delimiter: bool) {
    let c = cx.c;
    let base = cx.base();
    if delimiter && line.trim_end() == "---" {
        cx.push(line, Span { color: c.meta, ..base });
        return;
    }
    if line.trim_start().starts_with('#') {
        cx.push(line, Span { color: c.meta, ..base });
        return;
    }
    let key_end = line.find(':').filter(|&i| {
        let key = line[..i].trim().trim_start_matches("- ");
        !key.is_empty() && !key.contains(' ')
    });
    match key_end {
        Some(i) => {
            cx.push(&line[..i], base);
            cx.push(":", Span { color: c.marks, ..base });
            let value = &line[i + 1..];
            let v = value.trim();
            let span = if v.starts_with('"') || v.starts_with('\'') {
                Span { color: c.string, ..base }
            } else if v.parse::<f64>().is_ok() {
                Span { color: c.number, ..base }
            } else if matches!(v, "true" | "false" | "null" | "yes" | "no") {
                Span { color: c.keyword, ..base }
            } else {
                base
            };
            cx.push(value, span);
        }
        _ => cx.push(line, base),
    }
}

/// Block prefixes (quotes, list markers, task boxes), then the inline content.
fn block_line(cx: &mut Ctx<'_>, line: &str, base: Span) {
    let c = cx.c;
    let mut rest = line;
    let mut style = base;
    // > quotes (nested)
    loop {
        let t = rest.trim_start_matches([' ', '\t']);
        if let Some(after) = t.strip_prefix('>') {
            let ws = rest.len() - t.len();
            cx.push(&rest[..ws], style);
            cx.push(">", Span { color: c.marks, ..style });
            rest = after;
            style = Span { color: c.quote, italic: c.quote_italic, ..base };
        } else {
            break;
        }
    }
    // list marker: - * + or 1. 1)
    let t = rest.trim_start_matches([' ', '\t']);
    let ws = rest.len() - t.len();
    let marker_len = if t.starts_with(['-', '*', '+']) && (t.len() == 1 || t[1..].starts_with([' ', '\t'])) {
        1
    } else {
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && digits <= 9 && t[digits..].starts_with(['.', ')']) && (t.len() == digits + 1 || t[digits + 1..].starts_with([' ', '\t'])) {
            digits + 1
        } else {
            0
        }
    };
    if marker_len > 0 {
        cx.push(&rest[..ws], style);
        cx.push(&t[..marker_len], Span { color: c.marks, ..style });
        rest = &t[marker_len..];
        // task box [ ] / [x]
        let t2 = rest.trim_start_matches([' ', '\t']);
        let ws2 = rest.len() - t2.len();
        if ws2 > 0 && (t2.starts_with("[ ]") || t2.starts_with("[x]") || t2.starts_with("[X]")) {
            cx.push(&rest[..ws2], style);
            cx.push(&t2[..3], Span { color: c.marks, ..style });
            rest = &t2[3..];
        }
    }
    inline(cx, rest, style);
}

fn find_from(s: &str, from: usize, pat: &str) -> Option<usize> {
    s.get(from..)?.find(pat).map(|i| i + from)
}

/// Inline markup: code spans, [[wikilinks]], [links](url), URLs, **strong**, *emphasis*, ~~strike~~.
fn inline(cx: &mut Ctx<'_>, s: &str, style: Span) {
    let c = cx.c;
    let marks = Span { color: c.marks, bold: style.bold, scale: style.scale, ..cx.base() };
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut plain_start = 0;
    let flush = |cx: &mut Ctx<'_>, from: usize, to: usize| cx.push(&s[from..to], style);
    'outer: while i < bytes.len() {
        let b = bytes[i];
        // backslash escape
        if b == b'\\' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_punctuation() {
            flush(cx, plain_start, i);
            cx.push("\\", marks);
            cx.push(&s[i + 1..i + 2], style);
            i += 2;
            plain_start = i;
            continue;
        }
        // code span
        if b == b'`' {
            let n = bytes[i..].iter().take_while(|&&x| x == b'`').count();
            let delim = &s[i..i + n];
            let mut search = i + n;
            let mut close = None;
            while let Some(j) = find_from(s, search, delim) {
                let run = bytes[j..].iter().take_while(|&&x| x == b'`').count();
                if run == n {
                    close = Some(j);
                    break;
                }
                search = j + run;
            }
            if let Some(j) = close {
                flush(cx, plain_start, i);
                let mono = Span { color: c.monospace, mono: true, ..style };
                cx.push(delim, Span { color: c.marks, ..mono });
                cx.push(&s[i + n..j], mono);
                cx.push(delim, Span { color: c.marks, ..mono });
                i = j + n;
                plain_start = i;
                continue;
            }
            i += n;
            continue;
        }
        // [[wikilink]] / ![[embed]]
        if s[i..].starts_with("[[") || s[i..].starts_with("![[") {
            let open = if b == b'!' { 3 } else { 2 };
            if let Some(j) = find_from(s, i + open, "]]") {
                // CommonMark sees a link [x] inside literal brackets: the outer ones stay text.
                flush(cx, plain_start, i);
                let link = Span { color: c.link, underline: c.link_underline, ..style };
                cx.push(&s[i..i + open - 1], style);
                cx.push(&s[i + open - 1..j + 1], link);
                cx.push("]", style);
                i = j + 2;
                plain_start = i;
                continue;
            }
        }
        // [text](url) / ![alt](src)
        if b == b'[' || (b == b'!' && s[i..].starts_with("![")) {
            let open = if b == b'!' { 2 } else { 1 };
            if let Some(close) = find_from(s, i + open, "](") {
                if !s[i + open..close].contains(['[', ']']) {
                    if let Some(end) = find_from(s, close + 2, ")") {
                        flush(cx, plain_start, i);
                        let link = Span { color: c.link, underline: c.link_underline, ..style };
                        cx.push(&s[i..close + 1], link);
                        cx.push(&s[close + 1..end + 1], link);
                        i = end + 1;
                        plain_start = i;
                        continue;
                    }
                }
            }
        }
        // <https://autolink> and inline HTML tags (<br>, <span …>)
        if b == b'<' {
            if let Some(close) = s[i + 1..].find('>').map(|k| i + 1 + k) {
                let inner = &s[i + 1..close];
                let url = inner.contains("://") || (inner.contains('@') && !inner.contains(' '));
                let tag = inner.trim_start_matches('/').chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
                    && !inner.contains('<');
                if url {
                    flush(cx, plain_start, i);
                    cx.push("<", marks);
                    cx.push(inner, Span { color: c.url, ..style });
                    cx.push(">", marks);
                    i = close + 1;
                    plain_start = i;
                    continue;
                }
                if tag {
                    flush(cx, plain_start, i);
                    let name_end = inner.find(|ch: char| ch.is_whitespace() || ch == '/' && !inner.starts_with('/')).unwrap_or(inner.len());
                    cx.push("<", marks);
                    cx.push(&inner[..name_end], Span { color: c.link, ..style });
                    cx.push(&inner[name_end..], Span { color: c.meta, ..style });
                    cx.push(">", marks);
                    i = close + 1;
                    plain_start = i;
                    continue;
                }
            }
        }
        // **strong** / __strong__ (no ~~strike~~: the editor's markdown is plain CommonMark)
        for (delim, kind) in [("**", 0), ("__", 0)] {
            if s[i..].starts_with(delim) && i + 2 < bytes.len() && !bytes[i + 2].is_ascii_whitespace() {
                if delim == "__" && i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
                    continue;
                }
                if let Some(j) = find_from(s, i + 2, delim) {
                    if j > i + 2 && !bytes[j - 1].is_ascii_whitespace() {
                        flush(cx, plain_start, i);
                        cx.push(delim, marks);
                        let inner = if kind == 0 { Span { bold: true, ..style } } else { Span { strike: true, ..style } };
                        inline(cx, &s[i + 2..j], inner);
                        cx.push(delim, marks);
                        i = j + 2;
                        plain_start = i;
                        continue 'outer;
                    }
                }
            }
        }
        // *emphasis* / _emphasis_
        if (b == b'*' || b == b'_') && i + 1 < bytes.len() && !bytes[i + 1].is_ascii_whitespace() && bytes[i + 1] != b {
            let intraword = b == b'_' && i > 0 && bytes[i - 1].is_ascii_alphanumeric();
            if !intraword {
                let delim = if b == b'*' { "*" } else { "_" };
                let mut search = i + 1;
                let mut found = None;
                while let Some(j) = find_from(s, search, delim) {
                    let doubled = bytes.get(j + 1) == Some(&b) || (j > 0 && bytes[j - 1] == b && j - 1 > i);
                    let closing_ok = !bytes[j - 1].is_ascii_whitespace()
                        && !(b == b'_' && bytes.get(j + 1).is_some_and(u8::is_ascii_alphanumeric));
                    if !doubled && closing_ok {
                        found = Some(j);
                        break;
                    }
                    search = j + 1;
                }
                if let Some(j) = found {
                    flush(cx, plain_start, i);
                    cx.push(delim, marks);
                    inline(cx, &s[i + 1..j], Span { italic: true, ..style });
                    cx.push(delim, marks);
                    i = j + 1;
                    plain_start = i;
                    continue;
                }
            }
        }
        i += s[i..].chars().next().map_or(1, char::len_utf8);
    }
    flush(cx, plain_start, bytes.len().min(s.len()));
}

/// Paints `bg` behind the characters in `ranges` (vim search matches), splitting sections.
pub fn mark_ranges(job: &mut LayoutJob, ranges: &[Range<usize>], bg: Color32) {
    if ranges.is_empty() {
        return;
    }
    let old = std::mem::take(&mut job.sections);
    let text = job.text.clone();
    for sec in old {
        let r = sec.byte_range.start.0..sec.byte_range.end.0;
        let mut cuts = vec![r.start, r.end];
        for m in ranges {
            if m.start > r.start && m.start < r.end {
                cuts.push(m.start);
            }
            if m.end > r.start && m.end < r.end {
                cuts.push(m.end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
                continue;
            }
            let mut f = sec.format.clone();
            if ranges.iter().any(|m| m.start <= a && b <= m.end) {
                f.background = bg;
            }
            job.sections.push(eframe::egui::text::LayoutSection { leading_space: 0.0, byte_range: (a.into())..(b.into()), format: f });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::PAPER;

    fn kinds(text: &str) -> Vec<(String, Color32)> {
        let h = highlight(text, &PAPER.editor, 19.0);
        h.job.sections.iter().map(|s| (h.job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(), s.format.color)).collect()
    }

    #[test]
    fn job_text_is_the_buffer() {
        for t in ["# H\n\n- [ ] a **b** `c` [[d]]\n```\ncode\n```\n> q *i*", "---\na: 1\n---\nx", "", "\n\n", "Título\n===\nç"] {
            assert_eq!(highlight(t, &PAPER.editor, 19.0).job.text, t);
        }
    }

    #[test]
    fn headings_marks_and_code() {
        let k = kinds("## Hi\n```\nx\n```");
        assert_eq!(k[0], ("##".into(), PAPER.editor.marks));
        assert!(k.iter().any(|(t, c)| t.starts_with(" Hi") && *c == PAPER.editor.heading));
        let h = highlight("a\n```\nx\n```\nb", &PAPER.editor, 19.0);
        assert_eq!(h.code_lines, vec![2..5, 6..7, 8..11]);
    }
}
