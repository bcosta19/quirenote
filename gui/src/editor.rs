// The note editor (Editor.svelte + writing.ts + visualLines.ts + codeblocks.ts +
// linkComplete.ts): loads and autosaves the note one second after the last change, detects
// conflicts with the version on disk, follows [[links]], toggles tasks, completes [[links]],
// stores pasted/dropped images, numbers every line shown on screen and keeps vim keys.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, Event, Frame, Galley, Id, Key, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Stroke, TextEdit, Ui, Vec2,
    text::{CCursor, CCursorRange},
};
use regex::Regex;

use crate::api::{Api, ApiError, Note};
use crate::bridge::Bridge;
use crate::fonts;
use crate::fuzzy;
use crate::highlight;
use crate::kit;
use crate::links::{self, WikiLink, link_text};
use crate::state::Theme;
use crate::theme;
use crate::undo::History;
use crate::vim::{self, Buf, Host, Mode, Scroll, VKey, Vim};

const AUTOSAVE: Duration = Duration::from_secs(1);
/// Font size of the notes (.cm-editor font-size: 19px), line height 1.6.
pub const SIZE: f32 = 19.0;
/// Centered reading column (~100 characters), like Neovim zen mode: 44em.
const COLUMN: f32 = 44.0 * SIZE;
const GUTTER: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Loading,
    Saved,
    Dirty,
    Saving,
    Conflict,
    Error,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Loading => "loading…",
            Status::Saved => "saved",
            Status::Dirty => "editing…",
            Status::Saving => "saving…",
            Status::Conflict => "conflict",
            Status::Error => "error",
        }
    }
}

struct Completion {
    /// Character index right after "[[".
    from: usize,
    items: Vec<(String, String, String)>,
    selected: usize,
}

#[derive(Default)]
struct LayoutCache {
    text_key: u64,
    job: Option<Arc<egui::text::LayoutJob>>,
    code_lines: Vec<Range<usize>>,
}

/// What happened in the editor this frame, for the app to act on.
#[derive(Default)]
pub struct Out {
    /// Ctrl+click or Ctrl+Enter on a [[link]].
    pub link: Option<WikiLink>,
    /// Images pasted or dropped: bytes and the position to insert the link at.
    pub images: Vec<(Vec<u8>, usize)>,
}

pub struct Editor {
    pub path: String,
    pub text: String,
    base: Option<Note>,
    /// Keeps the original line endings (CRLF from notes created on Windows).
    crlf: bool,
    pub status: Status,
    pub error: String,
    pub conflict: Option<Note>,
    save_due: Option<Instant>,
    load_req: u64,
    pub locked: bool,
    pub vim: Vim,
    history: History,
    cursor: usize,
    insert_before: Option<String>,
    completion: Option<Completion>,
    reveal_done: u64,
    /// Character to bring into view (and how), after the next layout.
    scroll_to: Option<(usize, Option<Align>)>,
    want_focus: bool,
    layout: LayoutCache,
    galley: Option<Arc<Galley>>,
    galley_pos: Pos2,
    viewport_h: f32,
    message_until: Option<Instant>,
    font_gen: u64,
    /// Cursor to apply to the text field at the next frame (search results, outline).
    pending_cursor: Option<usize>,
    /// Hash of the text at the last frame: a difference means it was edited.
    last_text_hash: u64,
    /// `:w` typed in vim.
    save_requested: bool,
    /// zz / zt / zb: how to bring the cursor into view.
    scroll_align: Option<Align>,
}

/// The cursor on a character: at a wrap, the first character of the next screen line (vim's
/// cursor sits on characters; egui's default puts a boundary index at the end of the row above).
fn on_char(i: usize) -> CCursor {
    CCursor {
        index: i.into(),
        prefer_next_row: true,
    }
}

fn hash_str(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn to_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

fn char_to_byte(s: &str, c: usize) -> usize {
    s.char_indices().nth(c).map_or(s.len(), |(b, _)| b)
}

impl Editor {
    pub fn new(path: String) -> Self {
        Editor {
            path,
            text: String::new(),
            base: None,
            crlf: false,
            status: Status::Loading,
            error: String::new(),
            conflict: None,
            save_due: None,
            load_req: 0,
            locked: false,
            vim: Vim::default(),
            history: History::default(),
            cursor: 0,
            insert_before: None,
            completion: None,
            reveal_done: 0,
            scroll_to: None,
            want_focus: false,
            layout: LayoutCache::default(),
            galley: None,
            galley_pos: Pos2::ZERO,
            viewport_h: 600.0,
            message_until: None,
            font_gen: 0,
            pending_cursor: None,
            last_text_hash: hash_str(""),
            save_requested: false,
            scroll_align: None,
        }
    }

    pub fn has_base(&self) -> bool {
        self.base.is_some()
    }

    pub fn readonly(&self) -> bool {
        self.locked || self.base.is_none()
    }

    pub fn base_mtime(&self) -> Option<String> {
        self.base.as_ref().map(|b| b.mtime.clone())
    }

    /// Fonts changed: everything is laid out again.
    pub fn fonts_changed(&mut self) {
        self.font_gen += 1;
    }

    // ------------------------------------------------------------ loading and saving

    pub fn load(&mut self, bridge: &Bridge, api: &Api) {
        self.load_req += 1;
        let request = self.load_req;
        self.save_due = None;
        self.base = None;
        self.status = Status::Loading;
        self.conflict = None;
        self.text.clear();
        self.history.clear();
        let register = std::mem::take(&mut self.vim.register);
        self.vim = Vim::default();
        self.vim.register = register;
        self.completion = None;
        self.cursor = 0;
        let p = self.path.clone();
        let api = api.clone();
        bridge.spawn(
            move || api.note(&p),
            move |app, res| {
                if app.editor.as_ref().is_none_or(|ed| ed.load_req != request) {
                    return; // the user already switched notes
                }
                let reveal = app.reveal.clone();
                let ctx = app_ctx(app);
                let Some(ed) = app.editor.as_mut() else {
                    return;
                };
                match res {
                    Ok(note) => {
                        ed.crlf = note.content.contains("\r\n");
                        let content = to_lf(&note.content);
                        ed.text = content.clone();
                        ed.last_text_hash = hash_str(&ed.text);
                        ed.base = Some(Note { content, ..note });
                        ed.status = Status::Saved;
                        ed.want_focus = true;
                        ed.set_cursor_state(&ctx, 0, None);
                        if let Some(r) = reveal.filter(|r| r.path == ed.path) {
                            ed.reveal(r.line, r.n);
                        }
                    }
                    Err(e) => {
                        ed.error = e.to_string();
                        ed.status = Status::Error;
                    }
                }
            },
        );
    }

    fn to_disk(&self, s: &str) -> String {
        if self.crlf {
            s.replace('\n', "\r\n")
        } else {
            s.to_string()
        }
    }

    pub fn save(&mut self, bridge: &Bridge, api: &Api) {
        self.save_due = None;
        let Some(base) = &self.base else { return };
        if matches!(self.status, Status::Conflict | Status::Saving) {
            return;
        }
        if self.text == base.content {
            return;
        }
        self.status = Status::Saving;
        let content = self.text.clone();
        let disk = self.to_disk(&content);
        let (path, mtime) = (base.path.clone(), base.mtime.clone());
        let api = api.clone();
        let request = self.load_req;
        bridge.spawn(
            move || api.save(&path, &disk, &mtime),
            move |app, res| {
                let bridge = app.bridge.clone();
                let api = app.api.clone();
                let Some(ed) = app.editor.as_mut() else {
                    return;
                };
                if ed.load_req != request {
                    return;
                }
                match res {
                    Ok(note) => {
                        let content = to_lf(&note.content);
                        ed.base = Some(Note { content, ..note });
                        let clean = ed.base.as_ref().is_some_and(|b| b.content == ed.text);
                        ed.status = if clean { Status::Saved } else { Status::Dirty };
                        if !clean {
                            ed.save_due = Some(Instant::now() + AUTOSAVE);
                            bridge.ctx().request_repaint_after(AUTOSAVE);
                        }
                    }
                    Err(ApiError::Conflict(current)) => {
                        ed.conflict = Some(Note {
                            content: to_lf(&current.content),
                            ..current
                        });
                        ed.status = Status::Conflict;
                    }
                    Err(e) => {
                        ed.error = e.to_string();
                        ed.status = Status::Error;
                    }
                }
                let _ = api;
            },
        );
    }

    /// Runs the autosave timer; called every frame, even with the editor hidden by the preview.
    pub fn tick(&mut self, bridge: &Bridge, api: &Api) {
        if let Some(due) = self.save_due {
            let now = Instant::now();
            if now >= due {
                self.save(bridge, api);
            } else {
                bridge.ctx().request_repaint_after(due - now);
            }
        }
    }

    /// Called after every change of the text (typing, vim, completion, tasks).
    fn changed(&mut self, ctx: &egui::Context) {
        let Some(base) = &self.base else { return };
        if self.text == base.content {
            if self.status == Status::Dirty {
                self.status = Status::Saved;
            }
            self.save_due = None;
            return;
        }
        if matches!(self.status, Status::Saved | Status::Error) {
            self.status = Status::Dirty;
        }
        self.save_due = Some(Instant::now() + AUTOSAVE);
        ctx.request_repaint_after(AUTOSAVE);
    }

    /// The file changed on disk (Neovim, Obsidian, Syncthing).
    pub fn external_change(&mut self, bridge: &Bridge, api: &Api) {
        let Some(base) = &self.base else { return };
        let (path, mtime) = (base.path.clone(), base.mtime.clone());
        let request = self.load_req;
        let api = api.clone();
        bridge.spawn(
            {
                let path = path.clone();
                move || api.note(&path).ok()
            },
            move |app, note| {
                let ctx = app_ctx(app);
                let Some(ed) = app.editor.as_mut() else {
                    return;
                };
                let Some(note) = note else { return };
                // Another note by now, or a newer save of ours meanwhile.
                if ed.load_req != request
                    || ed
                        .base
                        .as_ref()
                        .is_none_or(|b| b.mtime != mtime || b.path != path)
                {
                    return;
                }
                if note.mtime == mtime {
                    return; // our own save
                }
                let note = Note {
                    content: to_lf(&note.content),
                    ..note
                };
                if ed.status == Status::Saved {
                    let content = note.content.clone();
                    ed.base = Some(note);
                    ed.set_doc(&ctx, &content);
                    ed.status = Status::Saved;
                } else {
                    ed.conflict = Some(note);
                    ed.status = Status::Conflict;
                }
            },
        );
    }

    fn set_doc(&mut self, ctx: &egui::Context, content: &str) {
        self.history.checkpoint(&self.text, self.cursor);
        let head = self.cursor.min(content.chars().count());
        self.text = content.to_string();
        self.set_cursor_state(ctx, head, None);
    }

    pub fn use_disk(&mut self, ctx: &egui::Context) {
        let Some(c) = self.conflict.take() else {
            return;
        };
        let content = c.content.clone();
        self.base = Some(c);
        self.set_doc(ctx, &content);
        self.status = Status::Saved;
    }

    pub fn keep_mine(&mut self, bridge: &Bridge, api: &Api) {
        let (Some(c), Some(b)) = (self.conflict.take(), self.base.as_mut()) else {
            return;
        };
        b.mtime = c.mtime;
        self.status = Status::Dirty;
        self.save(bridge, api);
    }

    pub fn retry(&mut self, bridge: &Bridge, api: &Api) {
        if self.base.is_some() {
            self.save(bridge, api);
        } else {
            self.load(bridge, api);
        }
    }

    /// Read-only while the note is renamed/moved/deleted: anything typed in between would be
    /// saved to the old path, recreating it.
    pub fn set_locked(&mut self, v: bool) {
        self.locked = v;
    }

    /// For flush(): cancels the timer and tells what is left to do.
    pub fn prepare_flush(&mut self) -> Status {
        self.save_due = None;
        self.status
    }

    pub fn flushed(&self) -> bool {
        self.base.is_none() || self.status == Status::Saved
    }

    // ------------------------------------------------------------ edits from outside

    /// Toggles "- [ ]" / "- [x]" on the line (1 = first). False if it is not a task.
    pub fn toggle_task(&mut self, ctx: &egui::Context, n: usize) -> bool {
        static TASK: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^(\s*(?:>\s*)*(?:[-*+]|\d+[.)])\s+\[)([ xX])\]")
                .unwrap_or_else(|_| unreachable!())
        });
        if self.readonly() || n < 1 {
            return false;
        }
        let Some((start, line)) = self
            .text
            .split('\n')
            .scan(0usize, |pos, l| {
                let s = *pos;
                *pos += l.len() + 1;
                Some((s, l))
            })
            .nth(n - 1)
        else {
            return false;
        };
        let Some(m) = TASK.captures(line) else {
            return false;
        };
        let at = start + m.get(1).map_or(0, |g| g.end());
        let mark = if &m[2] == " " { "x" } else { " " };
        self.history.checkpoint(&self.text, self.cursor);
        self.text.replace_range(at..at + 1, mark);
        self.changed(ctx);
        true
    }

    /// Inserts text at the cursor (replacing the selection), like replaceSelection.
    pub fn insert(&mut self, ctx: &egui::Context, s: &str) {
        if self.readonly() {
            return;
        }
        let id = self.id();
        let (a, z) = TextEdit::load_state(ctx, id)
            .and_then(|st| st.cursor.char_range())
            .map(|r| {
                let (x, y) = (r.primary.index.0, r.secondary.index.0);
                (x.min(y), x.max(y))
            })
            .unwrap_or((self.cursor, self.cursor));
        self.history.checkpoint(&self.text, self.cursor);
        let ba = char_to_byte(&self.text, a);
        let bz = char_to_byte(&self.text, z);
        self.text.replace_range(ba..bz, s);
        let pos = a + s.chars().count();
        self.set_cursor_state(ctx, pos, None);
        self.want_focus = true;
        self.changed(ctx);
    }

    /// Inserts at a character position (image links); returns where the insertion ended.
    pub fn insert_at(&mut self, ctx: &egui::Context, at: usize, s: &str) -> usize {
        let at = at.min(self.text.chars().count());
        self.history.checkpoint(&self.text, self.cursor);
        let b = char_to_byte(&self.text, at);
        self.text.insert_str(b, s);
        let end = at + s.chars().count();
        self.set_cursor_state(ctx, end, None);
        self.changed(ctx);
        end
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn focus(&mut self) {
        self.want_focus = true;
    }

    pub fn id(&self) -> Id {
        Id::new(("editor", &self.path))
    }

    /// Moves the cursor to a line (search result, outline); each request applies once.
    pub fn reveal(&mut self, line: usize, n: u64) {
        if n == self.reveal_done || self.status == Status::Loading {
            return;
        }
        self.reveal_done = n;
        let pos: usize = self
            .text
            .split('\n')
            .take(line.saturating_sub(1))
            .map(|l| l.chars().count() + 1)
            .sum();
        let pos = pos.min(self.text.chars().count());
        self.cursor = pos;
        self.scroll_to = Some((pos, Some(Align::Center)));
        self.want_focus = true;
        self.pending_cursor = Some(pos);
    }

    fn set_cursor_state(&mut self, ctx: &egui::Context, pos: usize, sel: Option<usize>) {
        self.cursor = pos;
        let id = self.id();
        let mut st = TextEdit::load_state(ctx, id).unwrap_or_default();
        let range = match sel {
            Some(a) => CCursorRange::two(CCursor::new(a), CCursor::new(pos)),
            None => CCursorRange::one(on_char(pos)),
        };
        st.cursor.set_char_range(Some(range));
        st.store(ctx, id);
    }

    // ------------------------------------------------------------ drawing

    #[allow(clippy::too_many_lines)]
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        theme: Theme,
        files: &[String],
        bridge: &Bridge,
        api: &Api,
    ) -> Out {
        let mut out = Out::default();
        let p = theme::palette(theme);
        let ec = &p.editor;
        let ctx = ui.ctx().clone();
        if let Some(pos) = self.pending_cursor.take() {
            self.set_cursor_state(&ctx, pos, None);
        }
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, ec.bg);

        // Banners: conflict and error.
        if self.conflict.is_some() {
            Frame::new()
                .fill(p.warn_bg)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("This note changed on disk (Neovim/Obsidian?).")
                                .font(fonts::ui(13.0)),
                        );
                        if kit::button(ui, None, "Use the disk version", "").clicked() {
                            self.use_disk(&ctx);
                        }
                        if kit::button(ui, None, "Manter a minha", "").clicked() {
                            self.keep_mine(bridge, api);
                        }
                    });
                });
        }
        if self.status == Status::Error {
            Frame::new()
                .fill(p.error_bg)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&self.error).font(fonts::ui(13.0)));
                        if kit::button(ui, None, "try again", "").clicked() {
                            self.retry(bridge, api);
                        }
                    });
                });
        }

        let id = self.id();
        let focused = ctx.memory(|m| m.has_focus(id));
        let readonly = self.readonly();

        // Keys before the text field sees them: completion list, vim, undo, save, links.
        if focused {
            self.keys(&ctx, files, &mut out, bridge, api);
        }

        let area = ui.available_rect_before_wrap();
        self.viewport_h = area.height();
        let scroll = egui::ScrollArea::vertical()
            .id_salt(("editor-scroll", &self.path))
            .auto_shrink([false, false]);
        let colors = *ec;
        let search = if self.vim.highlight {
            self.vim.last_search.as_ref().map(|s| s.0.clone())
        } else {
            None
        };
        let font_gen = self.font_gen;
        let mut layout = std::mem::take(&mut self.layout);
        let code_bg = p.code_bg;
        let mut galley_out: Option<(Arc<Galley>, Pos2, Rect)> = None;
        let mut text_resp: Option<egui::Response> = None;
        let normal_like = self.vim.mode != Mode::Insert;
        let caret = ec.caret;
        let sel_color = ec.selection;
        // Bringing the cursor into view must happen inside the scroll area, this frame.
        let scroll_req = self.scroll_to.take();
        let scroll_out = scroll.show(ui, |ui| {
            let avail = ui.available_width();
            let col = COLUMN.min((avail - GUTTER - 16.0).max(120.0));
            let left = ((avail - col - GUTTER) / 2.0).max(0.0);
            ui.add_space(16.0);
            let bg_index = ui.painter().add(egui::Shape::Noop);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.add_space(left + GUTTER);
                let mut layouter =
                    |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| -> Arc<Galley> {
                        let text = buf.as_str();
                        let text_key = hash_str(text)
                            ^ (theme as u64) << 1
                            ^ font_gen.wrapping_mul(0x9e37_79b9);
                        if layout.text_key != text_key || layout.job.is_none() {
                            let h = highlight::highlight(text, &colors, SIZE);
                            layout.job = Some(Arc::new(h.job));
                            layout.code_lines = h.code_lines;
                            layout.text_key = text_key;
                        }
                        // No galley is kept across frames: a galley points into the font atlas,
                        // which egui rebuilds (theme, fonts, new glyphs); epaint caches per frame.
                        let mut job = layout.job.as_deref().cloned().unwrap_or_default();
                        job.wrap.max_width = wrap;
                        if let Some(pat) = &search
                            && let Some(re) = vim::search_regex(pat)
                        {
                            let ranges: Vec<Range<usize>> = re
                                .find_iter(text)
                                .filter(|m| m.start() < m.end())
                                .map(|m| m.range())
                                .collect();
                            highlight::mark_ranges(&mut job, &ranges, colors.search_match);
                        }
                        crate::hanging::layout(ui, &job, wrap)
                    };
                let style_scope = |ui: &mut Ui| {
                    let v = ui.visuals_mut();
                    v.selection.bg_fill = sel_color;
                    v.text_cursor.stroke = Stroke::new(if normal_like { 0.0 } else { 1.5 }, caret);
                    v.extreme_bg_color = Color32::TRANSPARENT;
                    v.text_edit_bg_color = Some(Color32::TRANSPARENT);
                };
                style_scope(ui);
                let before = if readonly {
                    Some(self.text.clone())
                } else {
                    None
                };
                let te = TextEdit::multiline(&mut self.text)
                    .id(id)
                    .frame(Frame::NONE)
                    .margin(Margin::ZERO)
                    .desired_width(col)
                    .desired_rows(1)
                    .lock_focus(true)
                    // Escape is vim's: egui would otherwise take the focus away from the field.
                    .event_filter(egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    })
                    .layouter(&mut layouter);
                let o = te.show(ui);
                if let Some(b) = before
                    && self.text != b
                {
                    self.text = b; // read-only: undo whatever got through
                }
                galley_out = Some((o.galley.clone(), o.galley_pos, o.text_clip_rect));
                text_resp = Some(o.response.response.clone());
            });
            // Breathing room at the end: you can write in the middle of the screen (40vh).
            ui.add_space(self.viewport_h * 0.4);
            if let (Some((g, pos, _)), Some((cpos, align))) = (&galley_out, scroll_req) {
                let c = g.pos_from_cursor(on_char(cpos));
                let r = Rect::from_min_size(*pos + c.min.to_vec2(), Vec2::new(2.0, c.height()));
                ui.scroll_to_rect(r.expand2(Vec2::new(0.0, 30.0)), align);
            }
            if let Some((g, pos, _)) = &galley_out {
                // Code block lines on their own background, the full column wide.
                let mut shapes = Vec::new();
                let mut start = 0usize;
                for row in &g.rows {
                    let n = row.row.char_count_excluding_newline().0
                        + usize::from(row.ends_with_newline);
                    let r = start..start + n.max(1);
                    if layout
                        .code_lines
                        .iter()
                        .any(|c| c.start < r.end && r.start <= c.end)
                    {
                        let rr = row.rect();
                        shapes.push(egui::Shape::rect_filled(
                            Rect::from_min_max(
                                Pos2::new(pos.x, pos.y + rr.min.y),
                                Pos2::new(pos.x + col, pos.y + rr.max.y),
                            ),
                            0.0,
                            code_bg,
                        ));
                    }
                    start += n;
                }
                ui.painter().set(bg_index, egui::Shape::Vec(shapes));
            }
        });
        self.layout = layout;
        let Some((galley, gpos, clip)) = galley_out else {
            return out;
        };
        let resp = text_resp.unwrap_or_else(|| ui.interact(Rect::NOTHING, id, Sense::hover()));
        self.galley = Some(galley.clone());
        self.galley_pos = gpos;

        // Text changed by typing (insert mode) or by vim before the layout.
        let text_now_hash = hash_str(&self.text);
        let text_changed = text_now_hash != self.last_text_hash;
        if text_changed {
            self.last_text_hash = text_now_hash;
            if self.base.is_some() {
                self.changed(&ctx);
            }
        }

        // Cursor and selection after the text field handled the mouse/keys. In visual mode
        // the field shows our selection (end exclusive): it is not read back, unless the
        // mouse is selecting.
        let range = TextEdit::load_state(&ctx, id).and_then(|s| s.cursor.char_range());
        let visual = matches!(self.vim.mode, Mode::Visual | Mode::VisualLine);
        if let Some(r) = range {
            let (p, sec) = (r.primary.index.0, r.secondary.index.0);
            if visual && resp.dragged() {
                // Dragging extends the selection: the cursor is on the last selected character.
                self.vim.anchor = sec;
                self.cursor = if p > sec { p - 1 } else { p };
            } else if !visual && (p != self.cursor || text_changed) {
                self.cursor = p;
                if self.vim.mode == Mode::Insert {
                    self.update_completion(files);
                }
            }
            // A mouse selection in normal mode starts visual mode, like codemirror-vim.
            if p != sec && self.vim.mode == Mode::Normal && resp.dragged() {
                self.vim.mode = Mode::Visual;
                self.vim.anchor = sec;
                self.cursor = if p > sec { p - 1 } else { p };
            }
        }
        if resp.clicked() && self.vim.mode != Mode::Insert {
            if matches!(self.vim.mode, Mode::Visual | Mode::VisualLine) {
                self.vim.mode = Mode::Normal;
            }
            let mut b = Buf::new(&self.text, self.cursor);
            b.clamp_normal();
            if b.cursor != self.cursor {
                self.set_cursor_state(&ctx, b.cursor, None);
            }
        }
        // Ctrl+click follows the [[link]] under the pointer.
        if resp.clicked()
            && ctx.input(|i| i.modifiers.command)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let c = galley.cursor_from_pos(pos - gpos).index.0;
            if let Some(l) = self.link_at(c) {
                out.link = Some(l);
            }
        }

        // Selection of visual mode: shown as the text field's selection.
        if matches!(self.vim.mode, Mode::Visual | Mode::VisualLine) {
            let b = Buf::new(&self.text, self.cursor);
            let (s, e) = self.vim.visual_range(&b);
            let mut st = TextEdit::load_state(&ctx, id).unwrap_or_default();
            st.cursor
                .set_char_range(Some(CCursorRange::two(CCursor::new(s), CCursor::new(e))));
            st.store(&ctx, id);
        }

        // Visual line numbers: every line shown on screen, including wrapped ones.
        let painter = ui
            .painter()
            .with_clip_rect(clip.expand2(Vec2::new(GUTTER + 20.0, 0.0)).intersect(area));
        let gutter_x = gpos.x - 10.0;
        for (i, row) in galley.rows.iter().enumerate() {
            let rr = row.rect();
            let y0 = gpos.y + rr.min.y;
            if y0 > area.bottom() || gpos.y + rr.max.y < area.top() {
                continue;
            }
            painter.text(
                Pos2::new(gutter_x, y0 + rr.height() / 2.0),
                Align2::RIGHT_CENTER,
                (i + 1).to_string(),
                fonts::mono(11.0),
                ec.gutter,
            );
        }

        // The block cursor of normal/visual mode (.cm-fat-cursor).
        if self.vim.mode != Mode::Insert && self.base.is_some() {
            let c = galley.pos_from_cursor(on_char(self.cursor));
            let next = galley.pos_from_cursor(on_char(self.cursor + 1));
            let ch = self.text.chars().nth(self.cursor);
            let w = if ch.is_none_or(|c| c == '\n') || next.min.y != c.min.y {
                SIZE * 0.5
            } else {
                (next.min.x - c.min.x).max(2.0)
            };
            let r = Rect::from_min_size(gpos + c.min.to_vec2(), Vec2::new(w, c.height()));
            if focused {
                painter.rect_filled(r, 0.0, ec.caret.gamma_multiply(0.55));
            } else {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, ec.caret), egui::StrokeKind::Inside);
            }
        }

        let _ = scroll_out;

        // [[link]] completion list under the cursor.
        if let Some(comp) = &self.completion
            && focused
            && self.vim.mode == Mode::Insert
        {
            let c = galley.pos_from_cursor(on_char(self.cursor));
            let below = gpos + c.min.to_vec2() + Vec2::new(0.0, c.height() + 2.0);
            // Like CodeMirror's tooltip: above the line when it does not fit below.
            let popup_h = (comp.items.len() as f32 * 22.0 + 4.0).min(18.0 * 14.0 * 1.2);
            let at = if below.y + popup_h > ctx.content_rect().bottom() {
                gpos + c.min.to_vec2() - Vec2::new(0.0, popup_h + 2.0)
            } else {
                below
            };
            if let Some(i) = completion_popup(&ctx, comp, at, ec) {
                self.accept_completion(&ctx, i, files);
            }
        }

        // Status in the corner, and vim's command line.
        let status_color = if matches!(self.status, Status::Conflict | Status::Error) {
            p.warn
        } else {
            p.muted
        };
        ui.painter().text(
            Pos2::new(area.right() - 10.0, area.bottom() - 6.0),
            Align2::RIGHT_BOTTOM,
            self.status.label(),
            fonts::ui(11.0),
            status_color,
        );
        self.vim_bar(ui, area, p);

        if self.want_focus && (!readonly || self.base.is_some()) {
            resp.request_focus();
            self.want_focus = false;
        }
        // Dropped image files go into the vault, like Obsidian.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty()
            && ctx
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| area.contains(p))
        {
            let at = ctx
                .input(|i| i.pointer.hover_pos())
                .map_or(self.cursor, |p| galley.cursor_from_pos(p - gpos).index.0);
            for f in dropped {
                let path = f.path().to_path_buf();
                if !links::IMAGE.is_match(&path.to_string_lossy()) {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&path) {
                    out.images.push((bytes, at));
                }
            }
        }
        out
    }

    fn vim_bar(&mut self, ui: &mut Ui, area: Rect, p: &theme::Palette) {
        let text = if let Some((k, t)) = &self.vim.cmdline {
            Some(format!("{k}{t}▏"))
        } else if let Some(m) = &self.vim.message {
            if self.message_until.is_none() {
                self.message_until = Some(Instant::now() + Duration::from_secs(3));
            }
            if self.message_until.is_some_and(|t| Instant::now() > t) {
                self.vim.message = None;
                self.message_until = None;
                None
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(500));
                Some(m.clone())
            }
        } else {
            let pending = self.vim.pending_text();
            (!pending.is_empty()).then_some(pending)
        };
        if let Some(t) = text {
            let h = 26.0;
            let r = Rect::from_min_max(Pos2::new(area.left(), area.bottom() - h), area.max);
            ui.painter().rect_filled(r, 0.0, p.editor.tooltip_bg);
            ui.painter().text(
                Pos2::new(r.left() + 10.0, r.center().y),
                Align2::LEFT_CENTER,
                t,
                fonts::mono(13.0),
                p.editor.text,
            );
        }
    }

    fn link_at(&self, c: usize) -> Option<WikiLink> {
        let mut start = 0;
        for line in self.text.split('\n') {
            let n = line.chars().count();
            if c <= start + n {
                return links::link_at(line, c - start);
            }
            start += n + 1;
        }
        None
    }

    fn line_of(&self, c: usize) -> usize {
        self.text.chars().take(c).filter(|&ch| ch == '\n').count() + 1
    }

    // ------------------------------------------------------------ keys

    fn keys(
        &mut self,
        ctx: &egui::Context,
        files: &[String],
        out: &mut Out,
        bridge: &Bridge,
        api: &Api,
    ) {
        // Completion list: arrows, Enter/Tab accept (like Obsidian), Escape closes only the list.
        if self.completion.is_some() && self.vim.mode == Mode::Insert {
            let n = self.completion.as_ref().map_or(0, |c| c.items.len());
            let mut accept = None;
            ctx.input_mut(|i| {
                if (i.consume_key(Modifiers::NONE, Key::ArrowDown)
                    || i.consume_key(Modifiers::COMMAND, Key::N))
                    && let Some(c) = &mut self.completion
                {
                    c.selected = (c.selected + 1) % n.max(1);
                }
                if (i.consume_key(Modifiers::NONE, Key::ArrowUp)
                    || i.consume_key(Modifiers::COMMAND, Key::P))
                    && let Some(c) = &mut self.completion
                {
                    c.selected = (c.selected + n.max(1) - 1) % n.max(1);
                }
                if i.consume_key(Modifiers::NONE, Key::Enter)
                    || i.consume_key(Modifiers::NONE, Key::Tab)
                {
                    accept = self.completion.as_ref().map(|c| c.selected);
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    self.completion = None;
                }
            });
            if let Some(i) = accept {
                self.accept_completion(ctx, i, files);
            }
        }
        // Ctrl+S saves; Ctrl+Enter follows the [[link]] under the cursor, or toggles the task.
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) {
            self.save(bridge, api);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            match self.link_at(self.cursor) {
                Some(l) => out.link = Some(l),
                None => {
                    let line = self.line_of(self.cursor);
                    self.toggle_task(ctx, line);
                }
            }
        }
        // Image in the clipboard: Ctrl+V stores it in the vault (text, if any, pastes as text).
        let paste = ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command && !modifiers.shift)
                    || matches!(e, Event::Paste(t) if t.is_empty())
            })
        });
        if paste
            && !self.readonly()
            && let Some(png) = clipboard_image()
        {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Paste(_))));
            out.images.push((png, self.cursor));
        }
        if self.readonly() {
            return;
        }
        let in_cmdline = self.vim.cmdline.is_some();
        if self.vim.mode == Mode::Insert && !in_cmdline {
            self.close_brackets(ctx);
            // Insert mode: the text field types; we keep Escape and the undo keys.
            let mut leave = false;
            let mut undo = 0i32;
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    Event::Key {
                        key: Key::Escape,
                        pressed,
                        ..
                    } => {
                        leave |= *pressed;
                        false
                    }
                    Event::Key {
                        key: Key::OpenBracket,
                        pressed,
                        modifiers,
                        ..
                    } if modifiers.ctrl => {
                        leave |= *pressed;
                        false
                    }
                    Event::Key {
                        key: Key::Z,
                        pressed,
                        modifiers,
                        ..
                    } if modifiers.command => {
                        if *pressed {
                            undo = if modifiers.shift { 1 } else { -1 };
                        }
                        false
                    }
                    Event::Key {
                        key: Key::Y,
                        pressed,
                        modifiers,
                        ..
                    } if modifiers.command => {
                        if *pressed {
                            undo = 1;
                        }
                        false
                    }
                    _ => true,
                });
            });
            if undo != 0 {
                let mut b = Buf::new(&self.text, self.cursor);
                let ok = {
                    let mut host = self.host(ctx);
                    if undo < 0 {
                        host.undo(&mut b)
                    } else {
                        host.redo(&mut b)
                    }
                };
                if ok {
                    self.text = b.text();
                    self.set_cursor_state(ctx, b.cursor, None);
                }
            }
            if leave {
                self.completion = None;
                let typed = self
                    .insert_before
                    .take()
                    .and_then(|before| inserted(&before, &self.text));
                let mut b = Buf::new(&self.text, self.cursor);
                self.vim.leave_insert(&mut b, typed);
                if b.changed {
                    self.text = b.text();
                }
                self.set_cursor_state(ctx, b.cursor, None);
            }
            return;
        }
        // Normal/visual mode (and the command line): every key is a vim key.
        let mut keys = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                match e {
                    Event::Text(t) => {
                        keys.extend(t.chars().map(VKey::Char));
                        false
                    }
                    Event::Key {
                        key,
                        pressed,
                        modifiers,
                        ..
                    } => {
                        if *pressed && let Some(k) = vkey(*key, *modifiers) {
                            keys.push(k);
                        }
                        // Shortcuts with Alt/Ctrl that vim does not use stay for the app.
                        modifiers.alt
                    }
                    Event::Copy => {
                        keys.push(VKey::Ctrl('c'));
                        false
                    }
                    Event::Paste(_) | Event::Cut => false,
                    _ => true,
                }
            });
        });
        if keys.is_empty() {
            return;
        }
        let mut b = Buf::new(&self.text, self.cursor);
        let mut vim = std::mem::take(&mut self.vim);
        let mut entered_insert = false;
        {
            let mut host = self.host(ctx);
            for k in keys {
                if entered_insert {
                    // Typed right after i/a/o in the same frame: insert it.
                    if let VKey::Char(c) = k {
                        b.chars.insert(b.cursor, c);
                        b.cursor += 1;
                        b.changed = true;
                    }
                    continue;
                }
                if let vim::Outcome::Insert = vim.key(k, &mut b, &mut host) {
                    entered_insert = true;
                }
            }
        }
        self.vim = vim;
        if entered_insert {
            self.insert_before = Some(if b.changed {
                b.text()
            } else {
                self.text.clone()
            });
        }
        if b.changed {
            self.text = b.text();
        }
        self.set_cursor_state(ctx, b.cursor, None);
        self.scroll_to = Some((b.cursor, self.scroll_align.take()));
        if self.save_requested {
            self.save_requested = false;
            self.save(bridge, api);
        }
    }

    /// CodeMirror's closeBrackets: typing ( [ { " ' inserts the pair (or wraps the selection),
    /// typing the closer in front of the same closer steps over it, Backspace between an empty
    /// pair deletes both. The typing of this frame is applied here, in order, up to the first
    /// key the text field must handle itself (arrows, Enter…), so nothing is reordered.
    fn close_brackets(&mut self, ctx: &egui::Context) {
        let id = self.id();
        let events = ctx.input(|i| i.events.clone());
        const BRACKETS: [char; 8] = ['(', '[', '{', '"', '\'', ')', ']', '}'];
        let interesting = events.iter().any(|e| match e {
            Event::Text(t) => t.contains(BRACKETS),
            Event::Key {
                key: Key::Backspace,
                pressed: true,
                ..
            } => true,
            _ => false,
        });
        if !interesting {
            return;
        }
        let Some(r) = TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range()) else {
            return;
        };
        let (x, y) = (r.primary.index.0, r.secondary.index.0);
        let (mut a, mut z) = (x.min(y), x.max(y));
        let mut chars: Vec<char> = self.text.chars().collect();
        let mut handled = 0;
        let closer = |c: char| match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            _ => None,
        };
        for e in &events {
            match e {
                Event::Text(t) => {
                    for c in t.chars() {
                        let next = chars.get(z).copied();
                        let prev = a.checked_sub(1).and_then(|i| chars.get(i)).copied();
                        if a == z && matches!(c, ')' | ']' | '}' | '"' | '\'') && next == Some(c) {
                            a += 1; // step over the closer already there
                            z = a;
                            continue;
                        }
                        let quote = c == '"' || c == '\'';
                        let free_after =
                            next.is_none_or(|ch| ch.is_whitespace() || ")]}".contains(ch));
                        let word_before = prev.is_some_and(char::is_alphanumeric);
                        match closer(c) {
                            Some(close) if a != z => {
                                chars.insert(z, close);
                                chars.insert(a, c);
                                a += 1;
                                z += 1;
                            }
                            Some(close) if free_after && !(quote && word_before) => {
                                chars.splice(a..z, [c, close]);
                                a += 1;
                                z = a;
                            }
                            _ => {
                                chars.splice(a..z, [c]);
                                a += 1;
                                z = a;
                            }
                        }
                    }
                    handled += 1;
                }
                Event::Key {
                    key: Key::Backspace,
                    pressed,
                    modifiers,
                    ..
                } if modifiers.is_none() => {
                    if *pressed {
                        if a != z {
                            chars.drain(a..z);
                            z = a;
                        } else if a > 0 {
                            let pair = chars
                                .get(a)
                                .is_some_and(|&nx| closer(chars[a - 1]) == Some(nx));
                            chars.drain(a - 1..if pair { a + 1 } else { a });
                            a -= 1;
                            z = a;
                        }
                    }
                    handled += 1;
                }
                // Keys of printed characters (their Text event does the typing) and releases.
                Event::Key {
                    modifiers,
                    pressed,
                    key,
                    ..
                } if !*pressed
                    || (modifiers.is_none() || *modifiers == Modifiers::SHIFT)
                        && key.name().chars().count() == 1 =>
                {
                    handled += 1;
                }
                _ => break,
            }
        }
        if handled == 0 {
            return;
        }
        ctx.input_mut(|i| {
            let mut n = 0;
            i.events.retain(|e| {
                n += 1;
                // The handled prefix goes (except key events, harmless and wanted by others).
                n > handled || matches!(e, Event::Key { .. } if !matches!(e, Event::Key { key: Key::Backspace, .. }))
            });
        });
        self.text = chars.into_iter().collect();
        if a == z {
            self.set_cursor_state(ctx, a, None);
        } else {
            self.set_cursor_state(ctx, z, Some(a));
        }
    }

    fn host<'a>(&'a mut self, ctx: &'a egui::Context) -> EditorHost<'a> {
        EditorHost { ed: self, ctx }
    }

    // ------------------------------------------------------------ [[link]] completion

    fn update_completion(&mut self, files: &[String]) {
        static OPEN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\[\[[^\]|#\n]*$").unwrap_or_else(|_| unreachable!()));
        let b = char_to_byte(&self.text, self.cursor);
        let line_start = self.text[..b].rfind('\n').map_or(0, |i| i + 1);
        let before = &self.text[line_start..b];
        let Some(m) = OPEN.find(before) else {
            self.completion = None;
            return;
        };
        let query = &before[m.start() + 2..];
        let from = self.cursor - query.chars().count();
        const MAX: usize = 40;
        let ranked = fuzzy::rank(query, files, |s| s.as_str());
        let items = ranked
            .into_iter()
            .take(MAX)
            .map(|path| {
                let text = link_text(path, files);
                let label = links::file_name(&text).to_string();
                let detail = links::dir_of(path).to_string();
                (label, detail, text)
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.completion = None;
            return;
        }
        let selected = self
            .completion
            .as_ref()
            .filter(|c| c.from == from)
            .map_or(0, |c| c.selected.min(items.len() - 1));
        self.completion = Some(Completion {
            from,
            items,
            selected,
        });
    }

    /// Replaces the whole target name, not just up to the cursor: completing in the middle of
    /// [[Nov|alias]] swaps the name and keeps |alias and #section. Only extends when the link
    /// really closes with "]]" on this line.
    fn accept_completion(&mut self, ctx: &egui::Context, i: usize, files: &[String]) {
        static REST: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^[^\[\]|#]*(?:\]\]|[|#][^\[\]]*\]\])").unwrap_or_else(|_| unreachable!())
        });
        static NAME: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^[^\[\]|#]*").unwrap_or_else(|_| unreachable!()));
        let Some(comp) = self.completion.take() else {
            return;
        };
        let Some((_, _, text)) = comp.items.get(i) else {
            return;
        };
        let from = comp.from;
        let mut to = self.cursor;
        let bto = char_to_byte(&self.text, to);
        let line_end = self.text[bto..]
            .find('\n')
            .map_or(self.text.len(), |e| bto + e);
        let rest = &self.text[bto..line_end];
        let closes = REST.is_match(rest);
        if closes && let Some(m) = NAME.find(rest) {
            to += m.as_str().chars().count();
        }
        let bt = char_to_byte(&self.text, to);
        let next: String = self.text[bt..].chars().take(2).collect();
        let insert = if closes {
            text.clone()
        } else {
            format!("{text}]]")
        };
        let anchor = from + insert.chars().count() + if next == "]]" { 2 } else { 0 };
        self.history.checkpoint(&self.text, self.cursor);
        let bf = char_to_byte(&self.text, from);
        self.text.replace_range(bf..bt, &insert);
        self.set_cursor_state(ctx, anchor, None);
        self.changed(ctx);
        let _ = files;
    }
}

/// The text typed during an insert session, when it was a plain insertion.
pub fn inserted(before: &str, after: &str) -> Option<String> {
    let a: Vec<char> = before.chars().collect();
    let b: Vec<char> = after.chars().collect();
    if b.len() < a.len() {
        return None;
    }
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(a.len() - prefix)
        .take_while(|(x, y)| x == y)
        .count();
    if prefix + suffix != a.len() {
        return None;
    }
    let s: String = b[prefix..b.len() - suffix].iter().collect();
    (!s.is_empty()).then_some(s)
}

fn vkey(key: Key, m: Modifiers) -> Option<VKey> {
    if m.alt {
        return None;
    }
    if m.ctrl || m.command {
        let name = key.name();
        if name.len() == 1 {
            return name
                .chars()
                .next()
                .map(|c| VKey::Ctrl(c.to_ascii_lowercase()));
        }
        return match key {
            Key::OpenBracket => Some(VKey::Ctrl('[')),
            _ => None,
        };
    }
    Some(match key {
        Key::Escape => VKey::Esc,
        Key::Enter => VKey::Enter,
        Key::Backspace => VKey::Backspace,
        Key::Delete => VKey::Delete,
        Key::Tab => VKey::Tab,
        Key::ArrowLeft => VKey::Left,
        Key::ArrowRight => VKey::Right,
        Key::ArrowUp => VKey::Up,
        Key::ArrowDown => VKey::Down,
        Key::Home => VKey::Home,
        Key::End => VKey::End,
        Key::PageUp => VKey::PageUp,
        Key::PageDown => VKey::PageDown,
        _ => return None,
    })
}

/// An image in the system clipboard (and no text), as PNG.
fn clipboard_image() -> Option<Vec<u8>> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if cb.get_text().is_ok_and(|t| !t.is_empty()) {
        return None;
    }
    let img = cb.get_image().ok()?;
    let rgba =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

fn completion_popup(
    ctx: &egui::Context,
    comp: &Completion,
    at: Pos2,
    ec: &theme::EditorColors,
) -> Option<usize> {
    let mut chosen = None;
    egui::Area::new(Id::new("link-completion"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            Frame::new()
                .fill(ec.tooltip_bg)
                .stroke(Stroke::new(1.0, ec.tooltip_border))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(18.0 * 14.0 * 1.2)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            for (i, (label, detail, _)) in comp.items.iter().enumerate() {
                                let sel = i == comp.selected;
                                let r = Frame::new()
                                    .fill(if sel {
                                        ec.tooltip_selected
                                    } else {
                                        Color32::TRANSPARENT
                                    })
                                    .inner_margin(Margin::symmetric(10, 3))
                                    .show(ui, |ui| {
                                        ui.set_min_width(260.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(label)
                                                    .font(fonts::ui(14.0))
                                                    .color(ec.text),
                                            );
                                            if !detail.is_empty() {
                                                ui.add_space(14.0);
                                                ui.label(
                                                    RichText::new(detail)
                                                        .font(fonts::ui(12.0))
                                                        .color(ec.gutter),
                                                );
                                            }
                                        });
                                    })
                                    .response;
                                let r =
                                    ui.interact(r.rect, Id::new(("completion", i)), Sense::click());
                                if sel {
                                    r.scroll_to_me(None);
                                }
                                if r.clicked() {
                                    chosen = Some(i);
                                }
                            }
                        });
                });
        });
    chosen
}

fn app_ctx(app: &crate::app::App) -> egui::Context {
    app.bridge.ctx().clone()
}

struct EditorHost<'a> {
    ed: &'a mut Editor,
    ctx: &'a egui::Context,
}

impl Host for EditorHost<'_> {
    fn rows(&mut self, buf: &Buf, delta: i32, col_x: &mut Option<f32>, stale: bool) -> usize {
        let galley = self.ed.galley.clone();
        match galley {
            Some(g) if !stale && g.job.text.chars().count() == buf.chars.len() => {
                let c = on_char(buf.cursor);
                let (n, x) = if delta > 0 {
                    g.cursor_down_one_row(&c, *col_x)
                } else {
                    g.cursor_up_one_row(&c, *col_x)
                };
                *col_x = x.or(*col_x);
                // At the first/last row the cursor stays where it was (no jump to the ends).
                let row_now = g.layout_from_cursor(c).row;
                let row_new = g.layout_from_cursor(n).row;
                if row_now == row_new {
                    buf.cursor
                } else {
                    n.index.0
                }
            }
            _ => {
                let line = buf.line_no(buf.cursor);
                let col = buf.cursor - buf.line_start(buf.cursor);
                let t = (line as i64 + delta as i64).clamp(0, buf.lines() as i64 - 1) as usize;
                let s = buf.line_pos(t);
                (s + col).min(buf.line_end(s))
            }
        }
    }
    fn page_rows(&self) -> usize {
        (self.ed.viewport_h / (SIZE * 1.6)).max(2.0) as usize
    }
    fn checkpoint(&mut self, buf: &Buf) {
        self.ed.history.checkpoint(&buf.text(), buf.cursor);
    }
    fn undo(&mut self, buf: &mut Buf) -> bool {
        match self.ed.history.undo(&buf.text(), buf.cursor) {
            Some((t, c)) => {
                buf.chars = t.chars().collect();
                buf.cursor = c.min(buf.chars.len());
                buf.changed = true;
                true
            }
            None => false,
        }
    }
    fn redo(&mut self, buf: &mut Buf) -> bool {
        match self.ed.history.redo(&buf.text(), buf.cursor) {
            Some((t, c)) => {
                buf.chars = t.chars().collect();
                buf.cursor = c.min(buf.chars.len());
                buf.changed = true;
                true
            }
            None => false,
        }
    }
    fn save(&mut self) {
        self.ed.save_requested = true;
    }
    fn scroll(&mut self, how: Scroll) {
        self.ed.scroll_align = Some(match how {
            Scroll::Center => Align::Center,
            Scroll::Top => Align::Min,
            Scroll::Bottom => Align::Max,
        });
    }
    fn clipboard_get(&mut self) -> Option<String> {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }
    fn clipboard_set(&mut self, text: String) {
        self.ctx.copy_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(text: &str) -> Editor {
        let mut ed = Editor::new("n.md".into());
        ed.text = text.into();
        ed.base = Some(Note {
            path: "n.md".into(),
            content: text.into(),
            mtime: "1".into(),
        });
        ed.status = Status::Saved;
        ed
    }

    #[test]
    fn toggles_tasks_and_marks_the_note_dirty() {
        let ctx = egui::Context::default();
        let mut ed = loaded("x\n- [ ] a\n> - [x] b\nplain");
        assert!(ed.toggle_task(&ctx, 2));
        assert!(ed.toggle_task(&ctx, 3));
        assert!(!ed.toggle_task(&ctx, 4));
        assert_eq!(ed.text, "x\n- [x] a\n> - [ ] b\nplain");
        assert_eq!(ed.status, Status::Dirty);
        ed.locked = true;
        assert!(!ed.toggle_task(&ctx, 2));
    }

    #[test]
    fn completes_links_keeping_alias_and_closing_brackets() {
        let ctx = egui::Context::default();
        let files = vec!["a/Novela.md".to_string(), "Other.md".to_string()];
        let mut ed = loaded("see [[Nov");
        ed.cursor = 9;
        ed.update_completion(&files);
        assert_eq!(
            ed.completion.as_ref().map(|c| c.items[0].2.clone()),
            Some("Novela".into())
        );
        ed.accept_completion(&ctx, 0, &files);
        assert_eq!(ed.text, "see [[Novela]]");
        let mut ed = loaded("[[Nov|alias]] x");
        ed.cursor = 5;
        ed.update_completion(&files);
        ed.accept_completion(&ctx, 0, &files);
        assert_eq!(ed.text, "[[Novela|alias]] x");
        let mut ed = loaded("x [[Nov");
        ed.cursor = 3;
        ed.update_completion(&files);
        assert!(ed.completion.is_none());
    }

    #[test]
    fn typed_text_of_an_insert_session() {
        assert_eq!(inserted("ab", "axyb").as_deref(), Some("xy"));
        assert_eq!(inserted("ab", "ab"), None);
        assert_eq!(inserted("abc", "ab"), None);
        assert_eq!(inserted("aaa", "aaaa").as_deref(), Some("a"));
    }
}
