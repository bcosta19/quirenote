// Vim keys for the editor, standing in for @replit/codemirror-vim: normal, insert, visual and
// visual-line modes, counts, operators with motions and text objects, registers, search, the
// `.` repeat and the ex commands notes need (:w, :s, :N, :noh). j/k and the arrows move through
// the lines shown on screen (gj/gk), as the web app maps them.
//
// Typing in insert mode is left to egui's TextEdit (IME, mouse, selection); this engine handles
// everything else on a character buffer and reports what the editor must do (save, scroll…).

use std::sync::LazyLock;

use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VKey {
    Char(char),
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Ctrl(char),
    /// Text typed in insert mode, recorded for the `.` repeat.
    Text(String),
}

/// The text being edited, as characters, with the cursor (a character index).
pub struct Buf {
    pub chars: Vec<char>,
    pub cursor: usize,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    Center,
    Top,
    Bottom,
}

/// What the editor provides: screen lines, undo history, the clipboard, saving.
pub trait Host {
    /// The position `delta` screen rows away (gj/gk), keeping the column in `col_x`.
    /// `stale` means the text changed since the last layout: logical lines must do.
    fn rows(&mut self, buf: &Buf, delta: i32, col_x: &mut Option<f32>, stale: bool) -> usize;
    /// How many screen rows are visible (Ctrl-d/u/f/b).
    fn page_rows(&self) -> usize;
    /// Records the state before a change (one undo step).
    fn checkpoint(&mut self, buf: &Buf);
    fn undo(&mut self, buf: &mut Buf) -> bool;
    fn redo(&mut self, buf: &mut Buf) -> bool;
    fn save(&mut self);
    fn scroll(&mut self, how: Scroll);
    fn clipboard_get(&mut self) -> Option<String>;
    fn clipboard_set(&mut self, text: String);
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    ToggleCase,
    Lower,
    Upper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Clone, Debug, PartialEq)]
enum Motion {
    Left,
    Right,
    RowUp,
    RowDown,
    LineUp,
    LineDown,
    LineDownFirst,
    LineUpFirst,
    WordFwd(bool),
    WordBack(bool),
    WordEnd(bool),
    WordEndBack(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastNonBlank,
    GotoLine(Option<usize>),
    GotoFirst,
    Find(char, char),
    RepeatFind(bool),
    MatchPair,
    ParaFwd,
    ParaBack,
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    SearchNext(bool),
}

#[derive(Clone, Debug, PartialEq)]
enum Object {
    Word(bool, bool),
    Quote(char, bool),
    Pair(char, char, bool),
    Paragraph(bool),
}

enum Parse {
    Incomplete,
    Invalid,
    Done(Cmd),
}

#[derive(Clone, Debug)]
struct Cmd {
    count: Option<usize>,
    register: Option<char>,
    action: Action,
}

#[derive(Clone, Debug)]
enum Action {
    Move(Motion),
    Operate(Op, Option<usize>, Target),
    Simple(String),
    Replace(char),
    Object(Object),
}

#[derive(Clone, Debug)]
enum Target {
    Motion(Motion),
    Object(Object),
    Line,
}

pub struct Vim {
    pub mode: Mode,
    pending: Vec<VKey>,
    pub register: Register,
    pub anchor: usize,
    /// ':' '/' '?' and what was typed after it.
    pub cmdline: Option<(char, String)>,
    pub last_search: Option<(String, bool)>,
    /// Search matches are highlighted until :noh (hlsearch).
    pub highlight: bool,
    last_find: Option<(char, char)>,
    col_x: Option<f32>,
    last_change: Vec<VKey>,
    recording: Option<Vec<VKey>>,
    replaying: bool,
    pub message: Option<String>,
    /// Count typed before i/a/o: the inserted text is repeated at Escape.
    insert_count: usize,
}

impl Default for Vim {
    fn default() -> Self {
        Vim {
            mode: Mode::Normal,
            pending: Vec::new(),
            register: Register::default(),
            anchor: 0,
            cmdline: None,
            last_search: None,
            highlight: false,
            last_find: None,
            col_x: None,
            last_change: Vec::new(),
            recording: None,
            replaying: false,
            message: None,
            insert_count: 1,
        }
    }
}

// ---------------------------------------------------------------- buffer helpers

impl Buf {
    pub fn new(text: &str, cursor: usize) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cursor = cursor.min(chars.len());
        Buf { chars, cursor, changed: false }
    }
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }
    pub fn len(&self) -> usize {
        self.chars.len()
    }
    pub fn line_start(&self, pos: usize) -> usize {
        let pos = pos.min(self.len());
        self.chars[..pos].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
    }
    /// Index of the '\n' ending the line (or the length).
    pub fn line_end(&self, pos: usize) -> usize {
        let pos = pos.min(self.len());
        self.chars[pos..].iter().position(|&c| c == '\n').map_or(self.len(), |i| pos + i)
    }
    pub fn line_no(&self, pos: usize) -> usize {
        self.chars[..pos.min(self.len())].iter().filter(|&&c| c == '\n').count()
    }
    pub fn lines(&self) -> usize {
        self.chars.iter().filter(|&&c| c == '\n').count() + 1
    }
    pub fn line_pos(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let mut n = 0;
        for (i, &c) in self.chars.iter().enumerate() {
            if c == '\n' {
                n += 1;
                if n == line {
                    return i + 1;
                }
            }
        }
        self.line_start(self.len())
    }
    fn first_non_blank(&self, pos: usize) -> usize {
        let s = self.line_start(pos);
        let e = self.line_end(pos);
        (s..e).find(|&i| !matches!(self.chars[i], ' ' | '\t')).unwrap_or(e)
    }
    fn at(&self, i: usize) -> char {
        self.chars.get(i).copied().unwrap_or('\n')
    }
    fn delete(&mut self, from: usize, to: usize) -> String {
        let to = to.min(self.len());
        let from = from.min(to);
        self.changed = true;
        self.chars.drain(from..to).collect()
    }
    fn insert(&mut self, at: usize, s: &str) {
        let at = at.min(self.len());
        self.changed = true;
        let v: Vec<char> = s.chars().collect();
        self.chars.splice(at..at, v);
    }
    /// The normal-mode cursor sits on a character, never on the line break of a non-empty line.
    pub fn clamp_normal(&mut self) {
        let len = self.len();
        if self.cursor > len {
            self.cursor = len;
        }
        let s = self.line_start(self.cursor);
        if (self.cursor == len || self.chars[self.cursor] == '\n') && self.cursor > s {
            self.cursor -= 1;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punct,
    Newline,
}

fn class(c: char, big: bool) -> Class {
    if c == '\n' {
        Class::Newline
    } else if c.is_whitespace() {
        Class::Space
    } else if big || c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

fn word_fwd(b: &Buf, pos: usize, big: bool) -> usize {
    let n = b.len();
    if pos >= n {
        return n;
    }
    let start = class(b.chars[pos], big);
    let mut i = pos;
    if start != Class::Space && start != Class::Newline {
        while i < n && class(b.chars[i], big) == start {
            i += 1;
        }
    }
    // Whitespace and line breaks; an empty line is a word of its own.
    while i < n {
        let c = class(b.chars[i], big);
        if c == Class::Newline {
            i += 1;
            if i < n && b.chars[i] == '\n' {
                return i;
            }
            continue;
        }
        if c == Class::Space {
            i += 1;
            continue;
        }
        break;
    }
    i
}

fn word_back(b: &Buf, pos: usize, big: bool) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut i = pos - 1;
    loop {
        let c = class(b.chars[i], big);
        if c == Class::Newline && i > 0 && b.chars[i - 1] == '\n' {
            return i;
        }
        if c != Class::Space && c != Class::Newline {
            break;
        }
        if i == 0 {
            return 0;
        }
        i -= 1;
    }
    let k = class(b.chars[i], big);
    while i > 0 && class(b.chars[i - 1], big) == k {
        i -= 1;
    }
    i
}

fn word_end(b: &Buf, pos: usize, big: bool) -> usize {
    let n = b.len();
    let mut i = pos + 1;
    while i < n && matches!(class(b.chars[i], big), Class::Space | Class::Newline) {
        i += 1;
    }
    if i >= n {
        return n.saturating_sub(1);
    }
    let k = class(b.chars[i], big);
    while i + 1 < n && class(b.chars[i + 1], big) == k {
        i += 1;
    }
    i
}

fn word_end_back(b: &Buf, pos: usize, big: bool) -> usize {
    if pos == 0 {
        return 0;
    }
    let k = class(b.at(pos), big);
    let mut i = pos;
    while i > 0 && class(b.chars[i], big) == k && k != Class::Space {
        i -= 1;
    }
    while i > 0 && matches!(class(b.chars[i], big), Class::Space | Class::Newline) {
        i -= 1;
    }
    i
}

fn is_blank_line(b: &Buf, pos: usize) -> bool {
    let s = b.line_start(pos);
    let e = b.line_end(pos);
    b.chars[s..e].iter().all(|c| c.is_whitespace())
}

fn para_fwd(b: &Buf, pos: usize) -> usize {
    let mut line = b.line_no(pos);
    let total = b.lines();
    // Skip blank lines, then the paragraph, stop at the next blank line.
    while line + 1 < total && is_blank_line(b, b.line_pos(line)) {
        line += 1;
    }
    while line + 1 < total {
        line += 1;
        if is_blank_line(b, b.line_pos(line)) {
            return b.line_pos(line);
        }
    }
    b.len()
}

fn para_back(b: &Buf, pos: usize) -> usize {
    let mut line = b.line_no(pos);
    while line > 0 && is_blank_line(b, b.line_pos(line)) {
        line -= 1;
    }
    while line > 0 {
        line -= 1;
        if is_blank_line(b, b.line_pos(line)) {
            return b.line_pos(line);
        }
    }
    0
}

fn match_pair(b: &Buf, pos: usize) -> Option<usize> {
    let e = b.line_end(pos);
    let start = (pos..e).find(|&i| "()[]{}".contains(b.chars[i]))?;
    let c = b.chars[start];
    let (open, close, fwd) = match c {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        _ => ('{', '}', false),
    };
    let mut depth = 0i32;
    if fwd {
        for i in start..b.len() {
            if b.chars[i] == open {
                depth += 1;
            } else if b.chars[i] == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    } else {
        for i in (0..=start).rev() {
            if b.chars[i] == close {
                depth += 1;
            } else if b.chars[i] == open {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

fn find_char(b: &Buf, pos: usize, kind: char, c: char, count: usize, repeat: bool) -> Option<usize> {
    let s = b.line_start(pos);
    let e = b.line_end(pos);
    let mut p = pos;
    for n in 0..count {
        match kind {
            'f' | 't' => {
                // A repeated t must not stay stuck right before the same character.
                let from = if kind == 't' && (n > 0 || repeat) && p + 1 < e && b.chars[p + 1] == c { p + 2 } else { p + 1 };
                p = (from..e).find(|&i| b.chars[i] == c)?;
            }
            _ => {
                let to = if kind == 'T' && (n > 0 || repeat) && p > s && b.chars[p - 1] == c { p - 1 } else { p };
                p = (s..to).rev().find(|&i| b.chars[i] == c)?;
            }
        }
    }
    Some(match kind {
        't' => p - 1,
        'T' => p + 1,
        _ => p,
    })
}

fn object_range(b: &Buf, pos: usize, obj: &Object, count: usize) -> Option<(usize, usize, bool)> {
    match obj {
        Object::Word(big, around) => {
            if b.len() == 0 {
                return None;
            }
            let pos = pos.min(b.len() - 1);
            let k = class(b.chars[pos], *big);
            if k == Class::Newline {
                return Some((pos, pos, false));
            }
            let mut s = pos;
            while s > 0 && class(b.chars[s - 1], *big) == k {
                s -= 1;
            }
            let mut e = pos + 1;
            while e < b.len() && class(b.chars[e], *big) == k {
                e += 1;
            }
            for _ in 1..count {
                let k2 = class(b.at(e), *big);
                if k2 == Class::Newline {
                    break;
                }
                while e < b.len() && class(b.chars[e], *big) == k2 {
                    e += 1;
                }
            }
            if *around {
                let ws_after = e;
                while e < b.len() && class(b.chars[e], *big) == Class::Space {
                    e += 1;
                }
                if e == ws_after {
                    while s > 0 && class(b.chars[s - 1], *big) == Class::Space {
                        s -= 1;
                    }
                }
            }
            Some((s, e, false))
        }
        Object::Quote(q, around) => {
            let ls = b.line_start(pos);
            let le = b.line_end(pos);
            let quotes: Vec<usize> = (ls..le).filter(|&i| b.chars[i] == *q && (i == 0 || b.chars[i - 1] != '\\')).collect();
            let mut pair = None;
            for w in quotes.chunks(2) {
                if w.len() == 2 && pos >= w[0] && pos <= w[1] {
                    pair = Some((w[0], w[1]));
                }
            }
            if pair.is_none() {
                // After the cursor on the same line, like vim.
                pair = quotes.iter().position(|&i| i > pos).and_then(|k| Some((quotes[k], *quotes.get(k + 1)?)));
            }
            let (a, z) = pair?;
            if *around { Some((a, z + 1, false)) } else { Some((a + 1, z, false)) }
        }
        Object::Pair(open, close, around) => {
            let mut depth = 0;
            let mut a = None;
            let mut i = pos.min(b.len());
            if b.at(i) == *close {
                depth = -1;
            }
            loop {
                if b.at(i) == *close && i != pos {
                    depth += 1;
                } else if b.at(i) == *open {
                    if depth <= 0 {
                        a = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                if i == 0 {
                    break;
                }
                i -= 1;
            }
            // Not inside a pair: the next one on the line, like codemirror-vim.
            let a = match a {
                Some(a) => a,
                None => {
                    let e = b.line_end(pos);
                    (pos..e).find(|&i| b.chars[i] == *open)?
                }
            };
            let mut depth = 0;
            let mut z = None;
            for j in a + 1..b.len() {
                if b.chars[j] == *open {
                    depth += 1;
                } else if b.chars[j] == *close {
                    if depth == 0 {
                        z = Some(j);
                        break;
                    }
                    depth -= 1;
                }
            }
            let z = z?;
            if *around { Some((a, z + 1, false)) } else { Some((a + 1, z, false)) }
        }
        Object::Paragraph(around) => {
            let line = b.line_no(pos);
            let blank = is_blank_line(b, pos);
            let mut first = line;
            while first > 0 && is_blank_line(b, b.line_pos(first - 1)) == blank {
                first -= 1;
            }
            let mut last = line;
            let total = b.lines();
            while last + 1 < total && is_blank_line(b, b.line_pos(last + 1)) == blank {
                last += 1;
            }
            if *around {
                while last + 1 < total && is_blank_line(b, b.line_pos(last + 1)) {
                    last += 1;
                }
            }
            let s = b.line_pos(first);
            let e = b.line_end(b.line_pos(last));
            Some((s, (e + 1).min(b.len()), true))
        }
    }
}

// ---------------------------------------------------------------- parsing

fn count_of(keys: &[VKey], i: &mut usize) -> Option<usize> {
    let mut n: Option<usize> = None;
    while let Some(VKey::Char(c)) = keys.get(*i) {
        match c.to_digit(10) {
            Some(d) if d > 0 || n.is_some() => {
                n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d as usize));
                *i += 1;
            }
            _ => break,
        }
    }
    n
}

fn motion(keys: &[VKey], i: &mut usize, after_op: bool) -> Result<Option<Motion>, ()> {
    let Some(k) = keys.get(*i) else { return Err(()) };
    *i += 1;
    let m = match k {
        VKey::Char('h') | VKey::Left | VKey::Backspace => Motion::Left,
        VKey::Char('l') | VKey::Right | VKey::Char(' ') => Motion::Right,
        // j/k and the arrows move through the lines shown on screen; with an operator, lines.
        VKey::Char('j') | VKey::Down => {
            if after_op {
                Motion::LineDown
            } else {
                Motion::RowDown
            }
        }
        VKey::Char('k') | VKey::Up => {
            if after_op {
                Motion::LineUp
            } else {
                Motion::RowUp
            }
        }
        VKey::Ctrl('n') => Motion::LineDown,
        VKey::Ctrl('p') => Motion::LineUp,
        VKey::Char('+') | VKey::Enter => Motion::LineDownFirst,
        VKey::Char('-') => Motion::LineUpFirst,
        VKey::Char('w') => Motion::WordFwd(false),
        VKey::Char('W') => Motion::WordFwd(true),
        VKey::Char('b') => Motion::WordBack(false),
        VKey::Char('B') => Motion::WordBack(true),
        VKey::Char('e') => Motion::WordEnd(false),
        VKey::Char('E') => Motion::WordEnd(true),
        VKey::Char('0') | VKey::Home => Motion::LineStart,
        VKey::Char('^') => Motion::FirstNonBlank,
        VKey::Char('$') | VKey::End => Motion::LineEnd,
        VKey::Char('G') => Motion::GotoLine(None),
        VKey::Char('%') => Motion::MatchPair,
        VKey::Char('}') => Motion::ParaFwd,
        VKey::Char('{') => Motion::ParaBack,
        VKey::Char(';') => Motion::RepeatFind(false),
        VKey::Char(',') => Motion::RepeatFind(true),
        VKey::Char('n') => Motion::SearchNext(false),
        VKey::Char('N') => Motion::SearchNext(true),
        VKey::Ctrl('d') => Motion::HalfDown,
        VKey::Ctrl('u') => Motion::HalfUp,
        VKey::Ctrl('f') | VKey::PageDown => Motion::PageDown,
        VKey::Ctrl('b') | VKey::PageUp => Motion::PageUp,
        VKey::Char(c @ ('f' | 'F' | 't' | 'T')) => match keys.get(*i) {
            None => return Ok(None),
            Some(VKey::Char(x)) => {
                *i += 1;
                Motion::Find(*c, *x)
            }
            Some(_) => return Err(()),
        },
        VKey::Char('g') => match keys.get(*i) {
            None => return Ok(None),
            Some(VKey::Char('g')) => {
                *i += 1;
                Motion::GotoFirst
            }
            Some(VKey::Char('j')) | Some(VKey::Down) => {
                *i += 1;
                Motion::RowDown
            }
            Some(VKey::Char('k')) | Some(VKey::Up) => {
                *i += 1;
                Motion::RowUp
            }
            Some(VKey::Char('e')) => {
                *i += 1;
                Motion::WordEndBack(false)
            }
            Some(VKey::Char('E')) => {
                *i += 1;
                Motion::WordEndBack(true)
            }
            Some(VKey::Char('_')) => {
                *i += 1;
                Motion::LastNonBlank
            }
            Some(VKey::Char('0')) => {
                *i += 1;
                Motion::LineStart
            }
            Some(VKey::Char('$')) => {
                *i += 1;
                Motion::LineEnd
            }
            _ => {
                *i -= 1;
                return Err(());
            }
        },
        _ => {
            *i -= 1;
            return Err(());
        }
    };
    Ok(Some(m))
}

fn object(keys: &[VKey], i: &mut usize) -> Result<Option<Object>, ()> {
    let around = match keys.get(*i) {
        Some(VKey::Char('i')) => false,
        Some(VKey::Char('a')) => true,
        _ => return Err(()),
    };
    let Some(k) = keys.get(*i + 1) else { return Ok(None) };
    let VKey::Char(c) = k else { return Err(()) };
    let o = match c {
        'w' => Object::Word(false, around),
        'W' => Object::Word(true, around),
        '"' | '\'' | '`' => Object::Quote(*c, around),
        '(' | ')' | 'b' => Object::Pair('(', ')', around),
        '[' | ']' => Object::Pair('[', ']', around),
        '{' | '}' | 'B' => Object::Pair('{', '}', around),
        '<' | '>' => Object::Pair('<', '>', around),
        'p' => Object::Paragraph(around),
        _ => return Err(()),
    };
    *i += 2;
    Ok(Some(o))
}

fn parse(keys: &[VKey], visual: bool) -> Parse {
    let mut i = 0;
    let mut register = None;
    if keys.first() == Some(&VKey::Char('"')) {
        match keys.get(1) {
            None => return Parse::Incomplete,
            Some(VKey::Char(r)) => register = Some(*r),
            Some(_) => return Parse::Invalid,
        }
        i = 2;
    }
    let count = count_of(keys, &mut i);
    let Some(k) = keys.get(i) else { return Parse::Incomplete };
    let done = |action| Parse::Done(Cmd { count, register, action });
    // Operators (in visual mode they apply to the selection right away).
    let op = match k {
        VKey::Char('d') => Some((Op::Delete, 'd')),
        VKey::Char('c') => Some((Op::Change, 'c')),
        VKey::Char('y') => Some((Op::Yank, 'y')),
        VKey::Char('>') => Some((Op::Indent, '>')),
        VKey::Char('<') => Some((Op::Outdent, '<')),
        VKey::Char('g') => match keys.get(i + 1) {
            None => return Parse::Incomplete,
            Some(VKey::Char('~')) => Some((Op::ToggleCase, '~')),
            Some(VKey::Char('u')) => Some((Op::Lower, 'u')),
            Some(VKey::Char('U')) => Some((Op::Upper, 'U')),
            _ => None,
        },
        _ => None,
    };
    if let Some((op, letter)) = op {
        let mut j = i + if matches!(k, VKey::Char('g')) { 2 } else { 1 };
        if visual {
            return done(Action::Operate(op, None, Target::Line));
        }
        let count2 = count_of(keys, &mut j);
        match keys.get(j) {
            None => return Parse::Incomplete,
            // dd, cc, yy, >>, <<, g~~, guu, gUU (and gugu, gUgU)
            Some(VKey::Char(c)) if *c == letter || (*c == 'g' && matches!(op, Op::ToggleCase | Op::Lower | Op::Upper)) => {
                if *c == 'g' {
                    match keys.get(j + 1) {
                        None => return Parse::Incomplete,
                        Some(VKey::Char(x)) if *x == letter => {}
                        _ => {}
                    }
                }
                return done(Action::Operate(op, count2, Target::Line));
            }
            _ => {}
        }
        match object(keys, &mut j) {
            Ok(Some(o)) => return done(Action::Operate(op, count2, Target::Object(o))),
            Ok(None) => return Parse::Incomplete,
            Err(()) => {}
        }
        return match motion(keys, &mut j, true) {
            Ok(Some(m)) => done(Action::Operate(op, count2, Target::Motion(m))),
            Ok(None) => Parse::Incomplete,
            Err(()) => Parse::Invalid,
        };
    }
    if visual {
        if let VKey::Char('i' | 'a') = k {
            let mut j = i;
            return match object(keys, &mut j) {
                Ok(Some(o)) => done(Action::Object(o)),
                Ok(None) => Parse::Incomplete,
                Err(()) => Parse::Invalid,
            };
        }
    }
    if let VKey::Char('r') = k {
        return match keys.get(i + 1) {
            None => Parse::Incomplete,
            Some(VKey::Char(c)) => done(Action::Replace(*c)),
            Some(VKey::Enter) => done(Action::Replace('\n')),
            Some(_) => Parse::Invalid,
        };
    }
    if let VKey::Char('z') = k {
        return match keys.get(i + 1) {
            None => Parse::Incomplete,
            Some(VKey::Char(c @ ('z' | 't' | 'b' | '.'))) => done(Action::Simple(format!("z{c}"))),
            Some(VKey::Enter) => done(Action::Simple("zt".into())),
            _ => Parse::Invalid,
        };
    }
    if let VKey::Char('g') = k {
        match keys.get(i + 1) {
            None => return Parse::Incomplete,
            Some(VKey::Char('J')) => return done(Action::Simple("gJ".into())),
            Some(VKey::Char('v')) => return done(Action::Simple("gv".into())),
            Some(VKey::Char('I')) => return done(Action::Simple("gI".into())),
            _ => {}
        }
    }
    let simple = match k {
        VKey::Char(c @ ('x' | 'X' | 's' | 'S' | 'D' | 'C' | 'Y' | 'p' | 'P' | 'J' | '~' | 'i' | 'a' | 'I' | 'A' | 'o' | 'O' | 'v' | 'V' | 'u' | '.' | ':' | '/' | '?' | '*' | '#' | 'U')) => {
            Some(c.to_string())
        }
        VKey::Delete => Some("x".into()),
        VKey::Ctrl('r') => Some("redo".into()),
        VKey::Esc | VKey::Ctrl('[') | VKey::Ctrl('c') => Some("esc".into()),
        VKey::Ctrl('a') => Some("inc".into()),
        VKey::Ctrl('x') => Some("dec".into()),
        _ => None,
    };
    if let Some(s) = simple {
        return done(Action::Simple(s));
    }
    let mut j = i;
    match motion(keys, &mut j, false) {
        Ok(Some(Motion::GotoLine(_))) => done(Action::Move(Motion::GotoLine(count))),
        Ok(Some(m)) => done(Action::Move(m)),
        Ok(None) => Parse::Incomplete,
        Err(()) => Parse::Invalid,
    }
}

// ---------------------------------------------------------------- execution

/// `:[range]s/pattern/replacement/flags` with any delimiter: (range, pattern, replacement, flags).
fn parse_subst(cmd: &str) -> Option<(String, String, String, String)> {
    let range_len = if cmd.starts_with('%') || cmd.starts_with('.') {
        1
    } else {
        cmd.find(|c: char| !(c.is_ascii_digit() || c == ',')).unwrap_or(cmd.len())
    };
    let (range, rest) = cmd.split_at(range_len);
    let rest = rest.strip_prefix('s')?;
    let mut chars = rest.chars();
    let delim = chars.next()?;
    if delim.is_alphanumeric() || delim == '\\' || delim.is_whitespace() {
        return None;
    }
    // Split on unescaped delimiters; an escaped delimiter stands for itself.
    let mut parts = vec![String::new()];
    let mut it = chars.peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some(n) if n == delim => parts.last_mut()?.push(n),
                Some(n) => {
                    let last = parts.last_mut()?;
                    last.push('\\');
                    last.push(n);
                }
                None => parts.last_mut()?.push('\\'),
            }
        } else if c == delim && parts.len() < 3 {
            parts.push(String::new());
        } else {
            parts.last_mut()?.push(c);
        }
    }
    let mut p = parts.into_iter();
    let pat = p.next().unwrap_or_default();
    let rep = p.next().unwrap_or_default();
    let flags = p.next().unwrap_or_default();
    if !flags.chars().all(|c| matches!(c, 'g' | 'i' | 'c')) {
        return None;
    }
    Some((range.to_string(), pat, rep, flags))
}

pub enum Outcome {
    /// Nothing special.
    None,
    /// Entered insert mode: further typing goes to the text field.
    Insert,
}

impl Vim {
    pub fn pending_text(&self) -> String {
        self.pending
            .iter()
            .map(|k| match k {
                VKey::Char(c) => c.to_string(),
                VKey::Ctrl(c) => format!("^{c}"),
                _ => String::new(),
            })
            .collect()
    }

    fn set_register(&mut self, host: &mut dyn Host, reg: Option<char>, text: String, linewise: bool) {
        if matches!(reg, Some('+' | '*')) {
            host.clipboard_set(text.clone());
        }
        if reg != Some('_') {
            self.register = Register { text, linewise };
        }
    }

    fn get_register(&self, host: &mut dyn Host, reg: Option<char>) -> Register {
        if matches!(reg, Some('+' | '*')) {
            if let Some(t) = host.clipboard_get() {
                let linewise = t.ends_with('\n');
                return Register { text: t, linewise };
            }
        }
        self.register.clone()
    }

    /// One key in normal/visual mode (or any mode while replaying `.`).
    pub fn key(&mut self, k: VKey, b: &mut Buf, host: &mut dyn Host) -> Outcome {
        self.message = None;
        if let Some((kind, text)) = &mut self.cmdline {
            let kind = *kind;
            match k {
                VKey::Esc | VKey::Ctrl('c') | VKey::Ctrl('[') => self.cmdline = None,
                VKey::Backspace => {
                    if text.pop().is_none() {
                        self.cmdline = None;
                    }
                }
                VKey::Enter => {
                    let t = text.clone();
                    self.cmdline = None;
                    if kind == ':' {
                        self.ex(&t, b, host);
                    } else {
                        let pat = if t.is_empty() { self.last_search.as_ref().map(|s| s.0.clone()).unwrap_or_default() } else { t };
                        self.last_search = Some((pat, kind == '/'));
                        self.highlight = true;
                        if let Some(p) = self.search(b, b.cursor, kind == '/', 1) {
                            b.cursor = p;
                        }
                    }
                    self.finish_normal(b);
                }
                VKey::Char(c) => text.push(c),
                VKey::Text(s) => text.push_str(&s),
                _ => {}
            }
            return Outcome::None;
        }
        if self.mode == Mode::Insert {
            // Only while replaying `.`: typing is the text field's job.
            match k {
                VKey::Text(s) => b.insert_at_cursor(&s),
                VKey::Char(c) => b.insert_at_cursor(&c.to_string()),
                VKey::Enter => b.insert_at_cursor("\n"),
                VKey::Esc | VKey::Ctrl('[') | VKey::Ctrl('c') => self.leave_insert(b, None),
                _ => {}
            }
            return Outcome::None;
        }
        self.pending.push(k);
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        match parse(&self.pending, visual) {
            Parse::Incomplete => Outcome::None,
            Parse::Invalid => {
                self.pending.clear();
                Outcome::None
            }
            Parse::Done(cmd) => {
                let keys = std::mem::take(&mut self.pending);
                self.run(cmd, keys, b, host)
            }
        }
    }

    fn start_recording(&mut self, keys: Vec<VKey>) {
        if !self.replaying {
            self.recording = Some(keys);
        }
    }

    fn finish_recording(&mut self) {
        if let Some(r) = self.recording.take() {
            self.last_change = r;
        }
    }

    fn finish_normal(&mut self, b: &mut Buf) {
        if self.mode == Mode::Normal {
            b.clamp_normal();
        }
    }

    fn enter_insert(&mut self, keys: Vec<VKey>, count: usize) -> Outcome {
        self.mode = Mode::Insert;
        self.insert_count = count.max(1);
        self.start_recording(keys);
        Outcome::Insert
    }

    /// Escape in insert mode. `typed` is the text inserted during the session, when it was a
    /// plain insertion (it is what `.` and a count repeat).
    pub fn leave_insert(&mut self, b: &mut Buf, typed: Option<String>) {
        if let (Some(t), true) = (&typed, self.insert_count > 1) {
            for _ in 1..self.insert_count {
                b.insert_at_cursor(t);
            }
        }
        self.insert_count = 1;
        if let Some(r) = &mut self.recording {
            if let Some(t) = typed {
                r.push(VKey::Text(t));
            }
            r.push(VKey::Esc);
        }
        self.finish_recording();
        self.mode = Mode::Normal;
        let s = b.line_start(b.cursor);
        if b.cursor > s {
            b.cursor -= 1;
        }
        b.clamp_normal();
    }

    fn apply_motion(&mut self, m: &Motion, count: Option<usize>, b: &mut Buf, host: &mut dyn Host) -> Option<(usize, Kind)> {
        let n = count.unwrap_or(1).max(1);
        let pos = b.cursor;
        let stale = b.changed;
        let r = match m {
            Motion::Left => (pos.saturating_sub(n).max(b.line_start(pos)), Kind::Exclusive),
            Motion::Right => {
                let e = b.line_end(pos);
                ((pos + n).min(e), Kind::Exclusive)
            }
            Motion::RowDown | Motion::RowUp => {
                let mut x = self.col_x;
                let mut tmp = Buf { chars: Vec::new(), cursor: pos, changed: stale };
                std::mem::swap(&mut tmp.chars, &mut b.chars);
                let mut p = pos;
                for _ in 0..n {
                    tmp.cursor = p;
                    p = host.rows(&tmp, if matches!(m, Motion::RowDown) { 1 } else { -1 }, &mut x, stale);
                }
                std::mem::swap(&mut tmp.chars, &mut b.chars);
                self.col_x = x;
                return Some((p, Kind::Exclusive));
            }
            Motion::LineDown | Motion::LineUp => {
                let line = b.line_no(pos);
                let col = pos - b.line_start(pos);
                let target = if matches!(m, Motion::LineDown) { (line + n).min(b.lines() - 1) } else { line.saturating_sub(n) };
                let s = b.line_pos(target);
                ((s + col).min(b.line_end(s)), Kind::Linewise)
            }
            Motion::LineDownFirst | Motion::LineUpFirst => {
                let line = b.line_no(pos);
                let target = if matches!(m, Motion::LineDownFirst) { (line + n).min(b.lines() - 1) } else { line.saturating_sub(n) };
                (b.first_non_blank(b.line_pos(target)), Kind::Linewise)
            }
            Motion::WordFwd(big) => {
                let mut p = pos;
                for _ in 0..n {
                    p = word_fwd(b, p, *big);
                }
                (p, Kind::Exclusive)
            }
            Motion::WordBack(big) => {
                let mut p = pos;
                for _ in 0..n {
                    p = word_back(b, p, *big);
                }
                (p, Kind::Exclusive)
            }
            Motion::WordEnd(big) => {
                let mut p = pos;
                for _ in 0..n {
                    p = word_end(b, p, *big);
                }
                (p, Kind::Inclusive)
            }
            Motion::WordEndBack(big) => {
                let mut p = pos;
                for _ in 0..n {
                    p = word_end_back(b, p, *big);
                }
                (p, Kind::Inclusive)
            }
            Motion::LineStart => (b.line_start(pos), Kind::Exclusive),
            Motion::FirstNonBlank => (b.first_non_blank(pos), Kind::Exclusive),
            Motion::LineEnd => {
                let line = b.line_no(pos) + n - 1;
                let e = b.line_end(b.line_pos(line.min(b.lines() - 1)));
                (e.saturating_sub(1).max(b.line_start(e)), Kind::Inclusive)
            }
            Motion::LastNonBlank => {
                let s = b.line_start(pos);
                let e = b.line_end(pos);
                ((s..e).rev().find(|&i| !b.chars[i].is_whitespace()).unwrap_or(s), Kind::Inclusive)
            }
            Motion::GotoLine(l) => {
                let line = l.map_or(b.lines() - 1, |l| l.saturating_sub(1).min(b.lines() - 1));
                (b.first_non_blank(b.line_pos(line)), Kind::Linewise)
            }
            Motion::GotoFirst => {
                let line = count.map_or(0, |l| l.saturating_sub(1).min(b.lines() - 1));
                (b.first_non_blank(b.line_pos(line)), Kind::Linewise)
            }
            Motion::Find(kind, c) => {
                self.last_find = Some((*kind, *c));
                let p = find_char(b, pos, *kind, *c, n, false)?;
                (p, if matches!(kind, 'f' | 't') { Kind::Inclusive } else { Kind::Exclusive })
            }
            Motion::RepeatFind(reverse) => {
                let (kind, c) = self.last_find?;
                let kind = if *reverse {
                    match kind {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        _ => 't',
                    }
                } else {
                    kind
                };
                let p = find_char(b, pos, kind, c, n, true)?;
                (p, if matches!(kind, 'f' | 't') { Kind::Inclusive } else { Kind::Exclusive })
            }
            Motion::MatchPair => (match_pair(b, pos)?, Kind::Inclusive),
            Motion::ParaFwd => {
                let mut p = pos;
                for _ in 0..n {
                    p = para_fwd(b, p);
                }
                (p, Kind::Exclusive)
            }
            Motion::ParaBack => {
                let mut p = pos;
                for _ in 0..n {
                    p = para_back(b, p);
                }
                (p, Kind::Exclusive)
            }
            Motion::HalfDown | Motion::HalfUp | Motion::PageDown | Motion::PageUp => {
                let rows = host.page_rows().max(2);
                let step = match m {
                    Motion::HalfDown | Motion::HalfUp => rows / 2,
                    _ => rows.saturating_sub(2),
                } as i32;
                let dir = if matches!(m, Motion::HalfDown | Motion::PageDown) { 1 } else { -1 };
                let mut x = self.col_x;
                let mut p = pos;
                let mut tmp = Buf { chars: Vec::new(), cursor: pos, changed: stale };
                std::mem::swap(&mut tmp.chars, &mut b.chars);
                for _ in 0..step * n as i32 {
                    tmp.cursor = p;
                    p = host.rows(&tmp, dir, &mut x, stale);
                }
                std::mem::swap(&mut tmp.chars, &mut b.chars);
                (p, Kind::Linewise)
            }
            Motion::SearchNext(reverse) => {
                let (_, fwd) = self.last_search.clone()?;
                self.highlight = true;
                (self.search(b, pos, fwd != *reverse, n)?, Kind::Exclusive)
            }
        };
        if !matches!(m, Motion::RowDown | Motion::RowUp | Motion::HalfDown | Motion::HalfUp | Motion::PageDown | Motion::PageUp) {
            self.col_x = None;
        }
        Some(r)
    }

    /// The position of the `n`th match of the last search from `pos`, wrapping around.
    pub fn search(&self, b: &Buf, pos: usize, forward: bool, n: usize) -> Option<usize> {
        let (pat, _) = self.last_search.as_ref()?;
        let re = search_regex(pat)?;
        let text = b.text();
        let starts: Vec<usize> = {
            let mut v = Vec::new();
            let mut char_i = 0;
            let mut byte_i = 0;
            for m in re.find_iter(&text) {
                if m.start() == m.end() {
                    continue;
                }
                char_i += text[byte_i..m.start()].chars().count();
                byte_i = m.start();
                v.push(char_i);
            }
            v
        };
        if starts.is_empty() {
            return None;
        }
        let mut p = pos;
        for _ in 0..n {
            p = if forward {
                starts.iter().copied().find(|&s| s > p).unwrap_or(starts[0])
            } else {
                starts.iter().copied().rev().find(|&s| s < p).unwrap_or(starts[starts.len() - 1])
            };
        }
        Some(p)
    }

    fn run(&mut self, cmd: Cmd, keys: Vec<VKey>, b: &mut Buf, host: &mut dyn Host) -> Outcome {
        let n = cmd.count.unwrap_or(1).max(1);
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        match cmd.action {
            Action::Move(m) => {
                if let Some((p, _)) = self.apply_motion(&m, cmd.count, b, host) {
                    b.cursor = p.min(b.len());
                    if matches!(m, Motion::HalfDown | Motion::HalfUp | Motion::PageDown | Motion::PageUp) {
                        b.cursor = b.first_non_blank(b.cursor);
                    }
                }
                if !visual {
                    b.clamp_normal();
                } else if b.cursor == b.len() && b.cursor > 0 {
                    b.cursor -= 1;
                }
                Outcome::None
            }
            Action::Object(o) => {
                if let Some((s, e, linewise)) = object_range(b, b.cursor, &o, n) {
                    if e > s {
                        self.anchor = s;
                        b.cursor = e - 1;
                        if linewise {
                            self.mode = Mode::VisualLine;
                        }
                    }
                }
                Outcome::None
            }
            Action::Replace(c) => {
                if visual {
                    let (s, e) = self.visual_range(b);
                    host.checkpoint(b);
                    for i in s..e {
                        if b.chars[i] != '\n' {
                            b.chars[i] = c;
                        }
                    }
                    b.changed = true;
                    self.mode = Mode::Normal;
                    b.cursor = s;
                    b.clamp_normal();
                    return Outcome::None;
                }
                let e = b.line_end(b.cursor);
                if b.cursor + n > e {
                    return Outcome::None;
                }
                host.checkpoint(b);
                self.start_recording(keys);
                if c == '\n' {
                    b.delete(b.cursor, b.cursor + n);
                    b.insert(b.cursor, "\n");
                    b.cursor += 1;
                } else {
                    for i in b.cursor..b.cursor + n {
                        b.chars[i] = c;
                    }
                    b.changed = true;
                    b.cursor += n - 1;
                }
                self.finish_recording();
                Outcome::None
            }
            Action::Operate(op, count2, target) => {
                if visual {
                    let (s, e) = self.visual_range(b);
                    let linewise = self.mode == Mode::VisualLine;
                    self.mode = Mode::Normal;
                    return self.operate(op, s, e, linewise, cmd.register, Vec::new(), b, host, n);
                }
                let total = n * count2.unwrap_or(1).max(1);
                let (s, e, linewise) = match target {
                    Target::Line => {
                        let line = b.line_no(b.cursor);
                        let last = (line + total - 1).min(b.lines() - 1);
                        let s = b.line_pos(line);
                        let e = b.line_end(b.line_pos(last));
                        (s, e, true)
                    }
                    Target::Object(o) => match object_range(b, b.cursor, &o, total) {
                        Some((s, e, lw)) => {
                            if lw {
                                (s, e.saturating_sub(1).max(s), true)
                            } else {
                                (s, e, false)
                            }
                        }
                        None => return Outcome::None,
                    },
                    Target::Motion(m) => {
                        let start = b.cursor;
                        // cw on a word changes to the end of the word, like ce, but from the last
                        // character of a word it changes only that character.
                        let cw = matches!(m, Motion::WordFwd(_)) && op == Op::Change && !b.at(start).is_whitespace();
                        let (p, kind) = if let (true, Motion::WordFwd(big)) = (cw, &m) {
                            let big = *big;
                            let k = class(b.at(start), big);
                            let mut p = if class(b.at(start + 1), big) == k { word_end(b, start, big) } else { start };
                            for _ in 1..total {
                                p = word_end(b, p, big);
                            }
                            (p, Kind::Inclusive)
                        } else {
                            match self.apply_motion(&m, Some(total), b, host) {
                                Some(r) => r,
                                None => return Outcome::None,
                            }
                        };
                        let (mut s, mut e) = if p < start { (p, start) } else { (start, p) };
                        match kind {
                            Kind::Inclusive => e = (e + 1).min(b.len()),
                            Kind::Linewise => {
                                s = b.line_start(s);
                                e = b.line_end(e);
                            }
                            Kind::Exclusive => {
                                // dw on the last word of a line does not join the next line.
                                if matches!(m, Motion::WordFwd(_)) && b.line_no(e) > b.line_no(s) {
                                    let le = b.line_end(s);
                                    if le > s {
                                        e = le;
                                    }
                                }
                            }
                        }
                        (s, e, kind == Kind::Linewise)
                    }
                };
                self.operate(op, s, e, linewise, cmd.register, keys, b, host, 1)
            }
            Action::Simple(s) => self.simple(&s, cmd.count, cmd.register, keys, b, host),
        }
    }

    /// The selection of visual mode as a range of characters (inclusive of the cursor).
    pub fn visual_range(&self, b: &Buf) -> (usize, usize) {
        let (a, z) = if self.anchor <= b.cursor { (self.anchor, b.cursor) } else { (b.cursor, self.anchor) };
        if self.mode == Mode::VisualLine {
            (b.line_start(a), b.line_end(z))
        } else {
            (a.min(b.len()), (z + 1).min(b.len()))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn operate(&mut self, op: Op, s: usize, e: usize, linewise: bool, reg: Option<char>, keys: Vec<VKey>, b: &mut Buf, host: &mut dyn Host, times: usize) -> Outcome {
        let e = e.min(b.len());
        let s = s.min(e);
        match op {
            Op::Yank => {
                let mut text: String = b.chars[s..e].iter().collect();
                if linewise {
                    text.push('\n');
                }
                self.set_register(host, reg, text, linewise);
                b.cursor = s;
                b.clamp_normal();
                Outcome::None
            }
            Op::Delete | Op::Change => {
                host.checkpoint(b);
                if !keys.is_empty() {
                    self.start_recording(keys);
                }
                if linewise {
                    // Whole lines: with the line break, so the next line moves up.
                    let (ds, de) = if e < b.len() {
                        (s, e + 1)
                    } else if s > 0 {
                        (s - 1, e)
                    } else {
                        (s, e)
                    };
                    let mut text: String = b.chars[s..e].iter().collect();
                    text.push('\n');
                    self.set_register(host, reg, text, true);
                    if op == Op::Change {
                        // cc keeps the indentation and an empty line to type on.
                        let indent: String = b.chars[s..e].iter().take_while(|c| matches!(c, ' ' | '\t')).collect();
                        b.delete(s, e);
                        b.insert(s, &indent);
                        b.cursor = s + indent.chars().count();
                        return self.enter_insert_recorded();
                    }
                    b.delete(ds, de);
                    b.cursor = b.first_non_blank(ds.min(b.len()).min(if ds < s { b.line_start(ds) } else { ds }));
                    b.clamp_normal();
                    self.finish_recording();
                    return Outcome::None;
                }
                let text = b.delete(s, e);
                self.set_register(host, reg, text, false);
                b.cursor = s;
                if op == Op::Change {
                    return self.enter_insert_recorded();
                }
                b.clamp_normal();
                self.finish_recording();
                Outcome::None
            }
            Op::Indent | Op::Outdent => {
                host.checkpoint(b);
                if !keys.is_empty() {
                    self.start_recording(keys);
                }
                let first = b.line_no(s);
                let last = b.line_no(e.saturating_sub(if e > s && !linewise && b.at(e - 1) == '\n' { 1 } else { 0 }));
                for _ in 0..times {
                    for line in first..=last {
                        let ls = b.line_pos(line);
                        if op == Op::Indent {
                            if b.line_end(ls) > ls {
                                b.insert(ls, "\t");
                            }
                        } else if b.at(ls) == '\t' {
                            b.delete(ls, ls + 1);
                        } else {
                            let spaces = (ls..b.line_end(ls)).take_while(|&i| b.chars[i] == ' ').count().min(4);
                            b.delete(ls, ls + spaces);
                        }
                    }
                }
                b.cursor = b.first_non_blank(b.line_pos(first));
                self.finish_recording();
                Outcome::None
            }
            Op::ToggleCase | Op::Lower | Op::Upper => {
                host.checkpoint(b);
                if !keys.is_empty() {
                    self.start_recording(keys);
                }
                for i in s..e {
                    let c = b.chars[i];
                    let swapped: Vec<char> = match op {
                        Op::Lower => c.to_lowercase().collect(),
                        Op::Upper => c.to_uppercase().collect(),
                        _ if c.is_uppercase() => c.to_lowercase().collect(),
                        _ => c.to_uppercase().collect(),
                    };
                    if swapped.len() == 1 {
                        b.chars[i] = swapped[0];
                    }
                }
                b.changed = true;
                b.cursor = s;
                b.clamp_normal();
                self.finish_recording();
                Outcome::None
            }
        }
    }

    fn enter_insert_recorded(&mut self) -> Outcome {
        self.mode = Mode::Insert;
        self.insert_count = 1;
        Outcome::Insert
    }

    fn simple(&mut self, s: &str, count: Option<usize>, reg: Option<char>, keys: Vec<VKey>, b: &mut Buf, host: &mut dyn Host) -> Outcome {
        let n = count.unwrap_or(1).max(1);
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        if visual {
            return self.visual_simple(s, reg, b, host);
        }
        match s {
            "esc" => {
                self.pending.clear();
                Outcome::None
            }
            "x" | "X" => {
                let (from, to) = if s == "x" {
                    let e = b.line_end(b.cursor);
                    (b.cursor, (b.cursor + n).min(e))
                } else {
                    let ls = b.line_start(b.cursor);
                    (b.cursor.saturating_sub(n).max(ls), b.cursor)
                };
                if from == to {
                    return Outcome::None;
                }
                host.checkpoint(b);
                self.start_recording(keys);
                let t = b.delete(from, to);
                self.set_register(host, reg, t, false);
                b.cursor = from;
                b.clamp_normal();
                self.finish_recording();
                Outcome::None
            }
            "s" => {
                host.checkpoint(b);
                let e = b.line_end(b.cursor);
                let t = b.delete(b.cursor, (b.cursor + n).min(e));
                self.set_register(host, reg, t, false);
                self.enter_insert(keys, 1)
            }
            "S" | "C" | "D" => {
                host.checkpoint(b);
                let line = b.line_no(b.cursor);
                let last = (line + n - 1).min(b.lines() - 1);
                let e = b.line_end(b.line_pos(last));
                if s == "S" {
                    let ls = b.line_start(b.cursor);
                    let indent: String = (ls..e).map(|i| b.chars[i]).take_while(|c| matches!(c, ' ' | '\t')).collect();
                    let t = b.delete(ls, e);
                    self.set_register(host, reg, t + "\n", true);
                    b.insert(ls, &indent);
                    b.cursor = ls + indent.chars().count();
                    return self.enter_insert(keys, 1);
                }
                let t = b.delete(b.cursor, e);
                self.set_register(host, reg, t, false);
                if s == "C" {
                    return self.enter_insert(keys, 1);
                }
                self.start_recording(keys);
                b.clamp_normal();
                self.finish_recording();
                Outcome::None
            }
            "Y" => {
                let line = b.line_no(b.cursor);
                let last = (line + n - 1).min(b.lines() - 1);
                let s0 = b.line_start(b.cursor);
                let e = b.line_end(b.line_pos(last));
                let t: String = b.chars[s0..e].iter().collect::<String>() + "\n";
                self.set_register(host, reg, t, true);
                Outcome::None
            }
            "p" | "P" => {
                let r = self.get_register(host, reg);
                if r.text.is_empty() {
                    return Outcome::None;
                }
                host.checkpoint(b);
                self.start_recording(keys);
                let text = r.text.repeat(n);
                if r.linewise {
                    let body = text.strip_suffix('\n').unwrap_or(&text).to_string();
                    if s == "p" {
                        let e = b.line_end(b.cursor);
                        b.insert(e, &format!("\n{body}"));
                        b.cursor = b.first_non_blank(e + 1);
                    } else {
                        let ls = b.line_start(b.cursor);
                        b.insert(ls, &format!("{body}\n"));
                        b.cursor = b.first_non_blank(ls);
                    }
                } else {
                    let at = if s == "p" && b.cursor < b.line_end(b.cursor) { b.cursor + 1 } else { b.cursor };
                    b.insert(at, &text);
                    b.cursor = at + text.chars().count() - 1;
                }
                b.clamp_normal();
                self.finish_recording();
                Outcome::None
            }
            "J" | "gJ" => {
                let joins = n.max(2) - 1;
                host.checkpoint(b);
                self.start_recording(keys);
                for _ in 0..joins {
                    let e = b.line_end(b.cursor);
                    if e >= b.len() {
                        break;
                    }
                    if s == "gJ" {
                        b.delete(e, e + 1);
                        b.cursor = e;
                        continue;
                    }
                    let mut ws = e + 1;
                    while ws < b.len() && matches!(b.chars[ws], ' ' | '\t') {
                        ws += 1;
                    }
                    b.delete(e, ws);
                    let next_empty = b.at(e) == '\n' || e >= b.len();
                    let prev_space = e > 0 && matches!(b.chars[e - 1], ' ' | '\t');
                    if !next_empty && !prev_space && e > b.line_start(e) && b.at(e) != ')' {
                        b.insert(e, " ");
                    }
                    b.cursor = e;
                }
                b.clamp_normal();
                self.finish_recording();
                Outcome::None
            }
            "~" => {
                let e = b.line_end(b.cursor);
                if b.cursor >= e {
                    return Outcome::None;
                }
                host.checkpoint(b);
                self.start_recording(keys);
                let to = (b.cursor + n).min(e);
                for i in b.cursor..to {
                    let c = b.chars[i];
                    let sw: Vec<char> = if c.is_uppercase() { c.to_lowercase().collect() } else { c.to_uppercase().collect() };
                    if sw.len() == 1 {
                        b.chars[i] = sw[0];
                    }
                }
                b.changed = true;
                b.cursor = to;
                b.clamp_normal();
                self.finish_recording();
                Outcome::None
            }
            "i" | "a" | "I" | "A" | "gI" => {
                host.checkpoint(b);
                match s {
                    "a" => {
                        if b.cursor < b.line_end(b.cursor) {
                            b.cursor += 1;
                        }
                    }
                    "I" => b.cursor = b.first_non_blank(b.cursor),
                    "gI" => b.cursor = b.line_start(b.cursor),
                    "A" => b.cursor = b.line_end(b.cursor),
                    _ => {}
                }
                self.enter_insert(keys, n)
            }
            "o" | "O" => {
                host.checkpoint(b);
                let ls = b.line_start(b.cursor);
                let indent: String = b.chars[ls..b.line_end(ls)].iter().take_while(|c| matches!(c, ' ' | '\t')).collect();
                if s == "o" {
                    let e = b.line_end(b.cursor);
                    b.insert(e, &format!("\n{indent}"));
                    b.cursor = e + 1 + indent.chars().count();
                } else {
                    b.insert(ls, &format!("{indent}\n"));
                    b.cursor = ls + indent.chars().count();
                }
                self.enter_insert(keys, 1)
            }
            "v" => {
                self.mode = Mode::Visual;
                self.anchor = b.cursor;
                Outcome::None
            }
            "V" => {
                self.mode = Mode::VisualLine;
                self.anchor = b.cursor;
                Outcome::None
            }
            "gv" => {
                self.mode = Mode::Visual;
                Outcome::None
            }
            "u" => {
                for _ in 0..n {
                    if !host.undo(b) {
                        break;
                    }
                }
                b.clamp_normal();
                Outcome::None
            }
            "redo" => {
                for _ in 0..n {
                    if !host.redo(b) {
                        break;
                    }
                }
                b.clamp_normal();
                Outcome::None
            }
            "." => {
                if self.last_change.is_empty() {
                    return Outcome::None;
                }
                let mut keys = self.last_change.clone();
                if let Some(c) = count {
                    // A new count replaces the recorded one.
                    let mut i = 0;
                    let _ = count_of(&keys, &mut i);
                    keys.drain(..i);
                    let digits: Vec<VKey> = c.to_string().chars().map(VKey::Char).collect();
                    keys.splice(0..0, digits);
                }
                self.replaying = true;
                for k in keys {
                    self.key(k, b, host);
                }
                self.replaying = false;
                if self.mode == Mode::Insert {
                    self.mode = Mode::Normal;
                }
                b.clamp_normal();
                Outcome::None
            }
            ":" | "/" | "?" => {
                let c = s.chars().next().unwrap_or(':');
                self.cmdline = Some((c, String::new()));
                Outcome::None
            }
            "*" | "#" => {
                if let Some((ws, we, _)) = object_range(b, b.cursor, &Object::Word(false, false), 1) {
                    let word: String = b.chars[ws..we].iter().collect();
                    if word.trim().is_empty() {
                        return Outcome::None;
                    }
                    // codemirror-vim's * and # ignore case.
                    self.last_search = Some((format!("(?i)\\b{}\\b", regex::escape(&word)), s == "*"));
                    self.highlight = true;
                    if let Some(p) = self.search(b, if s == "*" { b.cursor.max(ws) } else { ws }, s == "*", n) {
                        b.cursor = p;
                    }
                }
                Outcome::None
            }
            "zz" | "z." => {
                host.scroll(Scroll::Center);
                Outcome::None
            }
            "zt" => {
                host.scroll(Scroll::Top);
                Outcome::None
            }
            "zb" => {
                host.scroll(Scroll::Bottom);
                Outcome::None
            }
            "inc" | "dec" => {
                // Ctrl-A / Ctrl-X on the number at or after the cursor.
                let ls = b.line_start(b.cursor);
                let le = b.line_end(b.cursor);
                let line: String = b.chars[ls..le].iter().collect();
                let col = b.cursor - ls;
                static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-?\d+").unwrap_or_else(|_| unreachable!()));
                let found = NUM.find_iter(&line).find(|m| line[..m.end()].chars().count() > col);
                if let Some(m) = found {
                    let v: i64 = m.as_str().parse().unwrap_or(0);
                    let nv = if s == "inc" { v + n as i64 } else { v - n as i64 };
                    host.checkpoint(b);
                    self.start_recording(keys);
                    let start = ls + line[..m.start()].chars().count();
                    let end = ls + line[..m.end()].chars().count();
                    b.delete(start, end);
                    let t = nv.to_string();
                    b.insert(start, &t);
                    b.cursor = start + t.chars().count() - 1;
                    self.finish_recording();
                }
                Outcome::None
            }
            "U" => Outcome::None,
            _ => Outcome::None,
        }
    }

    fn visual_simple(&mut self, s: &str, reg: Option<char>, b: &mut Buf, host: &mut dyn Host) -> Outcome {
        let (vs, ve) = self.visual_range(b);
        let linewise = self.mode == Mode::VisualLine;
        match s {
            "esc" => {
                self.mode = Mode::Normal;
                b.clamp_normal();
            }
            "v" => {
                self.mode = if self.mode == Mode::Visual { Mode::Normal } else { Mode::Visual };
                b.clamp_normal();
            }
            "V" => {
                self.mode = if self.mode == Mode::VisualLine { Mode::Normal } else { Mode::VisualLine };
                b.clamp_normal();
            }
            "o" => {
                std::mem::swap(&mut self.anchor, &mut b.cursor);
            }
            "x" | "X" | "D" => {
                self.mode = Mode::Normal;
                return self.operate(Op::Delete, vs, ve, linewise || s != "x", reg, Vec::new(), b, host, 1);
            }
            "s" | "S" | "C" => {
                self.mode = Mode::Normal;
                return self.operate(Op::Change, vs, ve, linewise || s != "s", reg, Vec::new(), b, host, 1);
            }
            "Y" => {
                self.mode = Mode::Normal;
                return self.operate(Op::Yank, vs, ve, true, reg, Vec::new(), b, host, 1);
            }
            "~" | "u" | "U" => {
                self.mode = Mode::Normal;
                let op = match s {
                    "u" => Op::Lower,
                    "U" => Op::Upper,
                    _ => Op::ToggleCase,
                };
                return self.operate(op, vs, ve, linewise, reg, Vec::new(), b, host, 1);
            }
            "J" | "gJ" => {
                self.mode = Mode::Normal;
                let lines = b.line_no(ve) - b.line_no(vs) + 1;
                b.cursor = vs;
                return self.simple(s, Some(lines.max(2)), reg, Vec::new(), b, host);
            }
            "p" | "P" => {
                let r = self.get_register(host, reg);
                host.checkpoint(b);
                let old = b.delete(vs, ve);
                b.insert(vs, r.text.strip_suffix('\n').filter(|_| r.linewise && linewise).unwrap_or(&r.text));
                b.cursor = vs;
                self.set_register(host, reg, old, linewise);
                self.mode = Mode::Normal;
                b.clamp_normal();
            }
            "I" | "A" => {
                self.mode = Mode::Normal;
                b.cursor = if s == "I" { vs } else { ve };
                host.checkpoint(b);
                return self.enter_insert(Vec::new(), 1);
            }
            ":" => {
                let (l1, l2) = (b.line_no(vs) + 1, b.line_no(ve.saturating_sub(1).max(vs)) + 1);
                self.mode = Mode::Normal;
                self.cmdline = Some((':', format!("{l1},{l2}")));
            }
            "/" | "?" => {
                self.cmdline = Some((s.chars().next().unwrap_or('/'), String::new()));
            }
            "*" | "#" => {}
            _ => {}
        }
        Outcome::None
    }

    fn ex(&mut self, cmd: &str, b: &mut Buf, host: &mut dyn Host) {
        let c = cmd.trim();
        match c {
            "w" | "write" | "wq" | "x" | "wa" | "wall" | "up" | "update" => host.save(),
            "q" | "q!" | "quit" => {}
            "noh" | "nohl" | "nohlsearch" => self.highlight = false,
            _ if !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()) => {
                let line: usize = c.parse().unwrap_or(1);
                b.cursor = b.first_non_blank(b.line_pos(line.saturating_sub(1).min(b.lines() - 1)));
            }
            "$" => b.cursor = b.first_non_blank(b.line_pos(b.lines() - 1)),
            _ => {
                if let Some((range, pat, rep, flags)) = parse_subst(c) {
                    let (range, rep, flags) = (range.as_str(), rep.as_str(), flags.as_str());
                    let pat = if pat.is_empty() { self.last_search.as_ref().map(|s| s.0.clone()).unwrap_or_default() } else { pat };
                    let full = if flags.contains('i') { format!("(?i){pat}") } else { pat.clone() };
                    let Some(re) = search_regex(&full) else {
                        self.message = Some(format!("Invalid regex: {pat}"));
                        return;
                    };
                    let (first, last) = match range {
                        "%" => (0, b.lines() - 1),
                        "" | "." => (b.line_no(b.cursor), b.line_no(b.cursor)),
                        r => {
                            let mut it = r.split(',').map(|x| x.parse::<usize>().unwrap_or(1).saturating_sub(1));
                            let a = it.next().unwrap_or(0);
                            (a, it.next().unwrap_or(a))
                        }
                    };
                    let rep = vim_replacement(rep);
                    let text = b.text();
                    let lines: Vec<&str> = text.split('\n').collect();
                    let mut out = Vec::with_capacity(lines.len());
                    let mut changed = false;
                    for (i, l) in lines.iter().enumerate() {
                        if i >= first && i <= last.min(lines.len() - 1) {
                            let new = if flags.contains('g') { re.replace_all(l, rep.as_str()) } else { re.replace(l, rep.as_str()) };
                            changed |= new != *l;
                            out.push(new.into_owned());
                        } else {
                            out.push((*l).to_string());
                        }
                    }
                    if changed {
                        host.checkpoint(b);
                        let joined = out.join("\n");
                        b.chars = joined.chars().collect();
                        b.changed = true;
                        b.cursor = b.first_non_blank(b.line_pos(last.min(b.lines() - 1)));
                        self.last_search = Some((pat, true));
                    } else {
                        self.message = Some(format!("Pattern not found: {pat}"));
                    }
                } else {
                    self.message = Some(format!("Not an editor command: {c}"));
                }
            }
        }
    }
}

/// vim's \1 / & in a :s replacement, as the regex crate's ${1} / ${0}.
fn vim_replacement(rep: &str) -> String {
    let mut out = String::new();
    let mut chars = rep.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(d @ '0'..='9') => out.push_str(&format!("${{{d}}}")),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            },
            '&' => out.push_str("${0}"),
            '$' => out.push_str("$$"),
            o => out.push(o),
        }
    }
    out
}

/// The search pattern as a regex (vim's \< \> become \b); an invalid one is searched literally.
pub fn search_regex(pat: &str) -> Option<Regex> {
    if pat.is_empty() {
        return None;
    }
    // smartcase, like codemirror-vim: a pattern without capitals ignores case.
    let ci = !pat.chars().any(char::is_uppercase) && !pat.starts_with("(?");
    let conv = pat.replace("\\<", "\\b").replace("\\>", "\\b");
    let conv = if ci { format!("(?i){conv}") } else { conv };
    Regex::new(&conv).ok().or_else(|| Regex::new(&format!("{}{}", if ci { "(?i)" } else { "" }, regex::escape(pat))).ok())
}

impl Buf {
    fn insert_at_cursor(&mut self, s: &str) {
        let at = self.cursor;
        self.insert(at, s);
        self.cursor = at + s.chars().count();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct H {
        undo: Vec<(String, usize)>,
        saved: bool,
    }
    impl Host for H {
        fn rows(&mut self, b: &Buf, delta: i32, _: &mut Option<f32>, _: bool) -> usize {
            let line = b.line_no(b.cursor);
            let col = b.cursor - b.line_start(b.cursor);
            let t = (line as i32 + delta).clamp(0, b.lines() as i32 - 1) as usize;
            let s = b.line_pos(t);
            (s + col).min(b.line_end(s))
        }
        fn page_rows(&self) -> usize {
            10
        }
        fn checkpoint(&mut self, b: &Buf) {
            self.undo.push((b.text(), b.cursor));
        }
        fn undo(&mut self, b: &mut Buf) -> bool {
            match self.undo.pop() {
                Some((t, c)) => {
                    b.chars = t.chars().collect();
                    b.cursor = c;
                    true
                }
                None => false,
            }
        }
        fn redo(&mut self, _: &mut Buf) -> bool {
            false
        }
        fn save(&mut self) {
            self.saved = true;
        }
        fn scroll(&mut self, _: Scroll) {}
        fn clipboard_get(&mut self) -> Option<String> {
            None
        }
        fn clipboard_set(&mut self, _: String) {}
    }

    fn run(text: &str, cursor: usize, keys: &str) -> (String, usize, Mode) {
        let mut v = Vim::default();
        let mut h = H::default();
        let mut b = Buf::new(text, cursor);
        let mut before: Option<String> = None;
        for c in keys.chars() {
            let k = match c {
                '\u{1b}' => VKey::Esc,
                '\u{1}' => VKey::Ctrl('a'),
                '\u{18}' => VKey::Ctrl('x'),
                '\r' => VKey::Enter,
                c => VKey::Char(c),
            };
            if v.mode == Mode::Insert && k != VKey::Esc {
                b.insert_at_cursor(&c.to_string());
                continue;
            }
            if v.mode == Mode::Insert {
                let typed = before.take().and_then(|bf| crate::editor::inserted(&bf, &b.text()));
                v.leave_insert(&mut b, typed);
                continue;
            }
            if let Outcome::Insert = v.key(k, &mut b, &mut h) {
                before = Some(b.text());
            }
        }
        (b.text(), b.cursor, v.mode)
    }

    #[test]
    fn motions_and_deletes() {
        assert_eq!(run("one two three", 0, "dw").0, "two three");
        assert_eq!(run("one two three", 0, "2dw").0, "three");
        assert_eq!(run("one two three", 0, "d2w").0, "three");
        assert_eq!(run("one two", 4, "dw").0, "one ");
        assert_eq!(run("a\nb\nc", 0, "dd").0, "b\nc");
        assert_eq!(run("a\nb\nc", 2, "dj").0, "a");
        assert_eq!(run("a\nb\nc", 4, "dd").0, "a\nb");
        assert_eq!(run("hello world", 0, "x").0, "ello world");
        assert_eq!(run("hello world", 0, "3x").0, "lo world");
        assert_eq!(run("hello world", 0, "D").0, "");
        assert_eq!(run("hello world", 6, "d0").0, "world");
        assert_eq!(run("hello world", 0, "dfo").0, " world");
        assert_eq!(run("hello world", 0, "dto").0, "o world");
        assert_eq!(run("a (b c) d", 4, "di(").0, "a () d");
        assert_eq!(run("a \"b c\" d", 4, "da\"").0, "a  d");
        assert_eq!(run("foo bar", 1, "diw").0, " bar");
        assert_eq!(run("foo bar", 1, "daw").0, "bar");
    }

    #[test]
    fn changes_and_insert() {
        assert_eq!(run("one two", 0, "cwxx\u{1b}").0, "xx two");
        assert_eq!(run("one two", 0, "ccnew\u{1b}").0, "new");
        assert_eq!(run("one", 0, "Ax\u{1b}").0, "onex");
        assert_eq!(run("one", 0, "oz\u{1b}").0, "one\nz");
        assert_eq!(run("one", 0, "Oz\u{1b}").0, "z\none");
        assert_eq!(run("ab", 0, "ix\u{1b}").0, "xab");
        assert_eq!(run("ab", 0, "ax\u{1b}").0, "axb");
    }

    #[test]
    fn yank_put_join_case_undo() {
        assert_eq!(run("a\nb", 0, "yyp").0, "a\na\nb");
        assert_eq!(run("a\nb", 0, "yyP").0, "a\na\nb");
        assert_eq!(run("ab", 0, "xp").0, "ba");
        assert_eq!(run("a\n  b", 0, "J").0, "a b");
        assert_eq!(run("abc", 0, "~~").0, "ABc");
        assert_eq!(run("abc", 0, "gUiw").0, "ABC");
        assert_eq!(run("abc", 0, "xu").0, "abc");
        assert_eq!(run("abc", 0, "x.").0, "c");
        assert_eq!(run("a b c", 0, "dw.").0, "c");
        assert_eq!(run("x", 0, ">>").0, "\tx");
        assert_eq!(run("\tx", 0, "<<").0, "x");
        assert_eq!(run("abc", 0, "rz").0, "zbc");
        assert_eq!(run("n 9 x", 0, "\u{1}").0, "n 10 x");
        assert_eq!(run("n 9 x", 0, "3\u{18}").0, "n 6 x");
    }

    #[test]
    fn visual_and_search_and_ex() {
        assert_eq!(run("hello world", 0, "vld").0, "llo world");
        assert_eq!(run("a\nb\nc", 2, "Vd").0, "a\nc");
        assert_eq!(run("a\nb\nc", 0, "Vjd").0, "c");
        assert_eq!(run("one two one", 0, "/one\r").1, 8);
        assert_eq!(run("one two one", 0, "*").1, 8);
        assert_eq!(run("a\nb\nc", 0, ":3\r").1, 4);
        assert_eq!(run("foo foo\nfoo", 0, ":%s/foo/x/g\r").0, "x x\nx");
        assert_eq!(run("foo foo", 0, ":s/foo/x\r").0, "x foo");
        assert_eq!(run("ab cd", 0, ":s/\\w+/[&]/\r").0, "[ab] cd");
        assert_eq!(run("a/b", 0, ":s#/#-#\r").0, "a-b");
        assert_eq!(run("a/b", 0, ":s/\\//-/\r").0, "a-b");
        assert_eq!(run("ab cd", 0, ":s/(\\w+)/[\\1]/g\r").0, "[ab] [cd]");
    }

    #[test]
    fn repeat_and_case_and_forward_pairs() {
        assert_eq!(run("a\nb\nc", 0, "ccnovo\u{1b}j.").0, "novo\nnovo\nc");
        assert_eq!(run("one two", 0, "cwX\u{1b}w.").0, "X X");
        assert_eq!(run("Um X", 3, "0~~").0, "uM X");
        assert_eq!(run("ab [cd] e", 0, "ca[X\u{1b}").0, "ab X e");
        assert_eq!(run("ab (cd) e", 0, "di(").0, "ab () e");
    }

    #[test]
    fn word_motions() {
        assert_eq!(run("one two", 0, "w").1, 4);
        assert_eq!(run("one two", 4, "b").1, 0);
        assert_eq!(run("one two", 0, "e").1, 2);
        assert_eq!(run("one\n\ntwo", 0, "w").1, 4);
        assert_eq!(run("a.b c", 0, "W").1, 4);
        assert_eq!(run("abc", 0, "$").1, 2);
        assert_eq!(run("  abc", 4, "^").1, 2);
        assert_eq!(run("a\nb\nc", 0, "G").1, 4);
        assert_eq!(run("a\nb\nc", 4, "gg").1, 0);
        assert_eq!(run("(a b)", 0, "%").1, 4);
        assert_eq!(run("a\n\nb\n\nc", 0, "}").1, 2);
    }
}
