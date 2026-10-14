// Dialogs and panels over the window: PromptDialog, ContextMenu, QuickSwitcher, SearchPanel,
// BacklinksPanel and Outline of the Svelte app, with the same keys and texts.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, Frame, Id, Key, Layout, Margin, Modifiers, Order, Pos2, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Ui, Vec2, text::LayoutJob,
};

use crate::api::SearchResult;
use crate::fonts;
use crate::fuzzy;
use crate::icons;
use crate::kit::{self, palette_ctx};
use crate::links::{dir_of, file_name, strip_md};
use crate::markdown::Heading;
use crate::theme::Palette;

/// Consumes a key press with exactly these modifiers (ignoring shift when `any_shift`).
pub fn take(ctx: &egui::Context, mods: Modifiers, key: Key) -> bool {
    ctx.input_mut(|i| i.consume_key(mods, key))
}

fn ctrl() -> Modifiers {
    Modifiers::COMMAND
}

/// Up/Down, Ctrl+N/J, Ctrl+P/K: the list keys of the panels. Returns -1, 0 or 1.
fn list_move(ctx: &egui::Context, letters: bool) -> i32 {
    let mut d = 0;
    if take(ctx, Modifiers::NONE, Key::ArrowDown) || take(ctx, ctrl(), Key::N) || take(ctx, ctrl(), Key::J) {
        d = 1;
    }
    if take(ctx, Modifiers::NONE, Key::ArrowUp) || take(ctx, ctrl(), Key::P) || take(ctx, ctrl(), Key::K) {
        d = -1;
    }
    if letters {
        if take(ctx, Modifiers::NONE, Key::J) {
            d = 1;
        }
        if take(ctx, Modifiers::NONE, Key::K) {
            d = -1;
        }
    }
    d
}

/// The scrolling list of a panel. An egui Area lays its content out in last frame's size, so
/// the list asks for the height its rows need (up to the max) instead of growing a row a frame.
fn list_area(max_h: f32, rows: usize, row_h: f32) -> ScrollArea {
    let want = (rows as f32 * row_h + 12.0).min(max_h);
    ScrollArea::vertical().max_height(max_h).min_scrolled_height(want).auto_shrink([false, true])
}

fn wrap(selected: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    ((selected as i64 + delta as i64).rem_euclid(len as i64)) as usize
}

fn input_frame(ui: &mut Ui, p: &Palette, content: impl FnOnce(&mut Ui)) {
    Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
        content(ui);
    });
    let r = ui.min_rect();
    let y = ui.cursor().top();
    ui.painter().hline(r.x_range(), y, Stroke::new(1.0, p.border));
}

/// The search-style text field of the panels: 15px, no frame, transparent.
/// The keys the panels handle themselves: egui would move the focus with the arrows and drop
/// it with Escape before the panel sees them.
pub const PANEL_KEYS: egui::EventFilter = egui::EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: true };

fn panel_input(ui: &mut Ui, id: Id, text: &mut String, hint: &str, p: &Palette, width: f32) -> egui::Response {
    ui.add(
        TextEdit::singleline(text)
            .id(id)
            .event_filter(PANEL_KEYS)
            .frame(Frame::NONE)
            .font(fonts::ui(15.0))
            .hint_text(RichText::new(hint).color(p.muted).font(fonts::ui(15.0)))
            .desired_width(width),
    )
}

// ---------------------------------------------------------------- PromptDialog

pub struct Prompt {
    pub message: String,
    pub value: String,
    pub hint: String,
    pub input: bool,
    pub ok_label: String,
    pub danger: bool,
    reply: Option<Sender<Option<String>>>,
    opened: bool,
}

impl Prompt {
    pub fn ask(message: String, value: String, hint: String, reply: Sender<Option<String>>) -> Self {
        Prompt { message, value, hint, input: true, ok_label: "OK".into(), danger: false, reply: Some(reply), opened: false }
    }
    pub fn confirm(message: String, hint: String, ok: String, danger: bool, reply: Sender<Option<String>>) -> Self {
        Prompt { message, value: String::new(), hint, input: false, ok_label: ok, danger, reply: Some(reply), opened: false }
    }

    pub fn answer(&mut self, v: Option<String>) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(v);
        }
    }

    /// Shows the dialog; true once it was answered (submit or cancel).
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let p = palette_ctx(ctx);
        let input_id = Id::new("prompt-input");
        let ok_id = Id::new("prompt-ok");
        // Keeps the app shortcuts from acting while typing here: Enter and Escape are ours.
        let enter = take(ctx, Modifiers::NONE, Key::Enter);
        let escape = take(ctx, Modifiers::NONE, Key::Escape);
        let mut result: Option<Option<String>> = None;
        let (_, outside) = kit::modal(ctx, "prompt-dialog", 0.18, 480.0, 16, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            ui.label(RichText::new(&self.message).font(fonts::ui(14.0)));
            if self.input {
                let r = kit_input(ui, input_id, &mut self.value, p);
                if !self.opened {
                    r.request_focus();
                    // Selects only the name (no folder or .md), like renaming in a file manager.
                    let chars: Vec<char> = self.value.chars().collect();
                    let start = chars.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
                    let lower = self.value.to_lowercase();
                    let end = if lower.ends_with(".md") { chars.len() - 3 } else { chars.len() };
                    if let Some(mut st) = TextEdit::load_state(ctx, input_id) {
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(start),
                            egui::text::CCursor::new(end.max(start)),
                        )));
                        st.store(ctx, input_id);
                    }
                }
            }
            if !self.hint.is_empty() {
                ui.label(RichText::new(&self.hint).font(fonts::ui(12.0)).color(p.muted));
            }
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ok = kit::primary(ui, &self.ok_label, self.danger, true);
                let ok = ui.interact(ok.rect, ok_id, Sense::click()).union(ok);
                if !self.input && !self.opened {
                    ok.request_focus();
                }
                if !self.input && ok.has_focus() {
                    // The focused button's ring, as the browser draws it.
                    ui.painter().rect_stroke(ok.rect.expand(1.5), 5, Stroke::new(1.5, p.text), egui::StrokeKind::Outside);
                }
                if ok.clicked() {
                    result = Some(Some(self.value.clone()));
                }
                if kit::button(ui, None, "Cancel", "").clicked() {
                    result = Some(None);
                }
            });
        });
        self.opened = true;
        if enter {
            result = Some(Some(self.value.clone()));
        }
        if escape || outside {
            result = Some(None);
        }
        match result {
            Some(v) => {
                self.answer(v);
                true
            }
            None => false,
        }
    }
}

impl Drop for Prompt {
    fn drop(&mut self) {
        self.answer(None);
    }
}

fn kit_input(ui: &mut Ui, id: Id, text: &mut String, p: &Palette) -> egui::Response {
    let focused = ui.memory(|m| m.has_focus(id));
    Frame::new()
        .fill(p.bg)
        .stroke(Stroke::new(1.0, if focused { p.accent } else { p.border }))
        .corner_radius(4)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.add(
                TextEdit::singleline(text).id(id).event_filter(PANEL_KEYS).frame(Frame::NONE).font(fonts::ui(14.0)).desired_width(f32::INFINITY),
            )
        })
        .inner
}

// ---------------------------------------------------------------- ContextMenu

pub struct MenuItem {
    pub label: String,
    pub danger: bool,
    pub action: MenuAction,
}

#[derive(Clone, Debug)]
pub enum MenuAction {
    NewNoteIn(String),
    NewFolderIn(String),
    Open(String),
    Rename(crate::api::Entry),
    Trash(crate::api::Entry),
}

pub struct ContextMenu {
    pub pos: Pos2,
    pub items: Vec<MenuItem>,
    /// The highlighted item: by the pointer, or by the keyboard after an arrow key.
    focused: Option<usize>,
    opened_at: u64,
}

impl ContextMenu {
    pub fn new(pos: Pos2, items: Vec<MenuItem>, frame: u64) -> Self {
        ContextMenu { pos, items, focused: None, opened_at: frame }
    }

    /// Ok(Some(action)) when an item was chosen, Ok(None) while open, Err(()) when closed.
    pub fn show(&mut self, ctx: &egui::Context, frame: u64) -> Result<Option<MenuAction>, ()> {
        let p = palette_ctx(ctx);
        // Esc only closes the menu; it does not reach vim or the app shortcuts.
        if take(ctx, Modifiers::NONE, Key::Escape) {
            return Err(());
        }
        let d = if take(ctx, Modifiers::NONE, Key::ArrowDown) {
            1
        } else if take(ctx, Modifiers::NONE, Key::ArrowUp) {
            -1
        } else {
            0
        };
        if d != 0 {
            self.focused = Some(match self.focused {
                None if d > 0 => 0,
                None => self.items.len().saturating_sub(1),
                Some(f) => wrap(f, d, self.items.len()),
            });
        }
        let enter = take(ctx, Modifiers::NONE, Key::Enter);
        let screen = ctx.content_rect();
        // Keeps the menu inside the window (its size is known after the first frame).
        let size = egui::AreaState::load(ctx, Id::new("context-menu")).and_then(|s| s.size).unwrap_or(Vec2::new(200.0, 100.0));
        let pos = Pos2::new(
            self.pos.x.min(screen.right() - size.x - 4.0).max(0.0),
            self.pos.y.min(screen.bottom() - size.y - 4.0).max(0.0),
        );
        let mut chosen = None;
        let area = egui::Area::new(Id::new("context-menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            Frame::new()
                .fill(p.panel)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(8)
                .inner_margin(Margin::same(4))
                .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(51) })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    // min-width: 200px, or the longest label.
                    let widest = self.items.iter().map(|it| ui.fonts_mut(|f| f.layout_no_wrap(it.label.clone(), fonts::ui(13.0), p.text)).size().x).fold(0.0, f32::max);
                    let w = (widest + 20.0).max(192.0);
                    for (i, item) in self.items.iter().enumerate() {
                        let color = if item.danger { p.error } else { p.text };
                        let (rect, r) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
                        if r.hovered() {
                            self.focused = Some(i);
                        }
                        if Some(i) == self.focused {
                            ui.painter().rect_filled(rect, 5, p.accent_bg);
                        }
                        ui.painter().text(
                            Pos2::new(rect.left() + 10.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &item.label,
                            fonts::ui(13.0),
                            color,
                        );
                        if r.clicked() || (enter && Some(i) == self.focused) {
                            chosen = Some(item.action.clone());
                        }
                    }
                });
        });
        if let Some(a) = chosen {
            return Ok(Some(a));
        }
        // Closes before another click acts (capture), but not on the click that opened it.
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        let inside = ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| area.response.rect.contains(pos));
        if pressed && !inside && frame > self.opened_at {
            return Err(());
        }
        if ctx.input(|i| !i.focused) {
            return Err(());
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------- QuickSwitcher

#[derive(Clone, Debug, PartialEq)]
pub enum SwitchItem {
    Open(String),
    Create(String),
}

pub enum SwitchOutcome {
    None,
    Close,
    Open(String),
    Create(String),
}

pub struct Switcher {
    shown: usize,
    query: String,
    selected: usize,
    opened: bool,
    scroll_to_selected: bool,
}

const SWITCH_MAX: usize = 50;

impl Switcher {
    pub fn new() -> Self {
        Switcher { shown: usize::MAX, query: String::new(), selected: 0, opened: false, scroll_to_selected: false }
    }

    pub fn items(&self, files: &[String], recent: &[String]) -> Vec<SwitchItem> {
        let q = self.query.trim();
        if q.is_empty() {
            let known: std::collections::HashSet<&String> = files.iter().collect();
            let rec: Vec<&String> = recent.iter().filter(|p| known.contains(p)).collect();
            let rest: Vec<&String> = files.iter().filter(|p| !rec.contains(p)).collect();
            return rec.into_iter().chain(rest).take(SWITCH_MAX).map(|p| SwitchItem::Open(p.clone())).collect();
        }
        let mut found: Vec<SwitchItem> =
            fuzzy::rank(q, files, |s| s.as_str()).into_iter().take(SWITCH_MAX).map(|p| SwitchItem::Open(p.clone())).collect();
        // No note with this exact name: offer to create it (like Obsidian).
        let target = if q.to_lowercase().ends_with(".md") { q.to_string() } else { format!("{q}.md") };
        if !files.iter().any(|f| f.to_lowercase() == target.to_lowercase()) {
            found.push(SwitchItem::Create(target));
        }
        found
    }

    pub fn show(&mut self, ctx: &egui::Context, files: &[String], recent: &[String]) -> SwitchOutcome {
        let p = palette_ctx(ctx);
        let items = self.items(files, recent);
        if items.len() != self.shown {
            self.shown = items.len();
            ctx.request_repaint();
        }
        let d = list_move(ctx, false);
        if d != 0 && !items.is_empty() {
            self.selected = wrap(self.selected, d, items.len());
            self.scroll_to_selected = true;
        }
        let shift_enter = take(ctx, Modifiers::SHIFT, Key::Enter);
        let enter = take(ctx, Modifiers::NONE, Key::Enter);
        let escape = take(ctx, Modifiers::NONE, Key::Escape);
        let mut out = SwitchOutcome::None;
        let choose = |item: Option<&SwitchItem>| match item {
            Some(SwitchItem::Open(p)) => SwitchOutcome::Open(p.clone()),
            Some(SwitchItem::Create(p)) => SwitchOutcome::Create(p.clone()),
            None => SwitchOutcome::None,
        };
        if shift_enter && !self.query.trim().is_empty() {
            // Shift+Enter creates a note with the typed text even if there are results.
            let it = items.iter().find(|i| matches!(i, SwitchItem::Create(_))).or(items.first());
            out = choose(it);
        } else if enter {
            out = choose(items.get(self.selected));
        }
        if escape {
            return SwitchOutcome::Close;
        }
        if !matches!(out, SwitchOutcome::None) {
            return out;
        }
        let before = self.query.clone();
        let (_, outside) = kit::modal(ctx, "quick-switcher", 0.12, 640.0, 0, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let w = ui.available_width();
            input_frame(ui, p, |ui| {
                let r = panel_input(
                    ui,
                    Id::new("switcher-input"),
                    &mut self.query,
                    "Find or create a note…  (↑↓ navigate · Enter open · Shift+Enter create · Esc close)",
                    p,
                    w - 28.0,
                );
                if !self.opened {
                    r.request_focus();
                }
            });
            let max_h = ui.ctx().content_rect().height() * 0.5;
            list_area(max_h, items.len(), 30.0).show(ui, |ui| {
                Frame::new().inner_margin(Margin::same(6)).show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 0.0;
                    if items.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new("No notes in the vault.").color(p.muted).font(fonts::ui(13.0)));
                    }
                    for (i, item) in items.iter().enumerate() {
                        let sel = i == self.selected;
                        let color = if sel { p.accent } else { p.text };
                        let r = kit::row_frame(sel, p)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 5.0;
                                    let (name, dir) = match item {
                                        SwitchItem::Create(path) => {
                                            let dir = dir_of(path);
                                            ui.label(
                                                RichText::new(format!("＋ Create note “{}”", strip_md(file_name(path))))
                                                    .font(fonts::ui(14.0))
                                                    .color(color),
                                            );
                                            ("", if dir.is_empty() { "vault root" } else { dir })
                                        }
                                        SwitchItem::Open(path) => {
                                            ui.label(kit::icon(icons::file_icon(path), 14.0).color(color));
                                            (strip_md(file_name(path)), dir_of(path))
                                        }
                                    };
                                    if !name.is_empty() {
                                        ui.add(egui::Label::new(RichText::new(name).font(fonts::ui(14.0)).color(color)).truncate());
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.add(egui::Label::new(RichText::new(dir).font(fonts::ui(12.0)).color(p.muted)).truncate());
                                    });
                                });
                            })
                            .response;
                        let r = ui.interact(r.rect, Id::new(("switch-row", i)), Sense::click());
                        if r.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                            self.selected = i;
                        }
                        if r.clicked() {
                            out = choose(Some(item));
                        }
                        if sel && self.scroll_to_selected {
                            r.scroll_to_me(None);
                        }
                    }
                });
            });
        });
        self.scroll_to_selected = false;
        self.opened = true;
        if self.query != before {
            self.selected = 0;
        }
        if outside {
            return SwitchOutcome::Close;
        }
        out
    }
}

// ---------------------------------------------------------------- results list (search, backlinks)

fn snippet_job(r: &SearchResult, p: &Palette, selected: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    for part in &r.parts {
        let mut f = egui::TextFormat::simple(fonts::editor(15.0), p.muted);
        if part.hit {
            f.color = p.text;
            f.background = if selected { p.warn_bg } else { p.accent_bg };
        }
        job.append(&part.text, 0.0, f);
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width(f32::INFINITY);
    job
}

/// One result: "<b>name</b> folder · line N" and the snippet with the matches marked.
fn result_row(ui: &mut Ui, r: &SearchResult, i: usize, selected: bool, p: &Palette, salt: &str) -> egui::Response {
    let resp = kit::row_frame(selected, p)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 2.0);
            let mut job = LayoutJob::default();
            job.append(strip_md(file_name(&r.path)), 0.0, egui::TextFormat::simple(fonts::ui_bold(13.0), p.text));
            let dir = dir_of(&r.path);
            let where_ = format!(" {}{}line {}", dir, if dir.is_empty() { "" } else { " · " }, r.line);
            job.append(&where_, 0.0, egui::TextFormat::simple(fonts::ui(12.0), p.muted));
            ui.label(job);
            let mut sj = snippet_job(r, p, selected);
            sj.wrap.max_width = ui.available_width();
            sj.wrap.max_rows = 1;
            ui.label(sj);
        })
        .response;
    ui.interact(resp.rect, Id::new((salt, i)), Sense::click())
}

// ---------------------------------------------------------------- SearchPanel

pub enum PanelOutcome {
    None,
    /// Closed; carries the query (search) to reopen with it later.
    Close,
    Open(String, usize),
}

pub struct SearchPanel {
    shown: usize,
    pub query: String,
    pub results: Vec<SearchResult>,
    pub status: String,
    selected: usize,
    opened: bool,
    due: Option<Instant>,
    pub generation: u64,
    scroll_to_selected: bool,
}

pub const SEARCH_LIMIT: usize = 200; // search.MaxTotal on the server

impl SearchPanel {
    pub fn new(initial: String) -> Self {
        SearchPanel {
            shown: usize::MAX,
            due: Some(Instant::now()),
            query: initial,
            results: Vec::new(),
            status: String::new(),
            selected: 0,
            opened: false,
            generation: 0,
            scroll_to_selected: false,
        }
    }

    /// The search result for request `generation` (older ones are ignored).
    pub fn finish(&mut self, generation: u64, res: Result<Vec<SearchResult>, String>) {
        if generation != self.generation {
            return;
        }
        match res {
            Ok(mut found) => {
                // The server sends one result past the limit just to signal truncation.
                let cut = found.len() > SEARCH_LIMIT;
                found.truncate(SEARCH_LIMIT);
                self.results = found;
                self.selected = 0;
                self.status = if self.results.is_empty() {
                    "nothing found".into()
                } else {
                    format!("{}{} match(es)", self.results.len(), if cut { "+" } else { "" })
                };
            }
            Err(e) => self.status = e,
        }
    }

    /// Returns the query to search now (after the 200 ms pause), if any.
    pub fn show(&mut self, ctx: &egui::Context) -> (PanelOutcome, Option<String>) {
        let p = palette_ctx(ctx);
        if self.results.len() != self.shown {
            self.shown = self.results.len();
            ctx.request_repaint();
        }
        let d = list_move(ctx, false);
        if d != 0 && !self.results.is_empty() {
            self.selected = wrap(self.selected, d, self.results.len());
            self.scroll_to_selected = true;
        }
        let enter = take(ctx, Modifiers::NONE, Key::Enter);
        if take(ctx, Modifiers::NONE, Key::Escape) {
            return (PanelOutcome::Close, None);
        }
        let mut out = PanelOutcome::None;
        if enter {
            if let Some(r) = self.results.get(self.selected) {
                out = PanelOutcome::Open(r.path.clone(), r.line);
            }
        }
        let before = self.query.clone();
        let (_, outside) = kit::modal(ctx, "search-panel", 0.10, 760.0, 0, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let w = ui.available_width();
            input_frame(ui, p, |ui| {
                ui.horizontal(|ui| {
                    let status_w = if self.status.is_empty() { 0.0 } else { 140.0 };
                    let r = panel_input(ui, Id::new("search-input"), &mut self.query, "Search note contents…", p, w - 28.0 - status_w);
                    if !self.opened {
                        r.request_focus();
                        // Selects the previous term, so typing replaces it.
                        if let Some(mut st) = TextEdit::load_state(ui.ctx(), Id::new("search-input")) {
                            let n = self.query.chars().count();
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                            st.store(ui.ctx(), Id::new("search-input"));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(&self.status).font(fonts::ui(12.0)).color(p.muted));
                    });
                });
            });
            let max_h = ui.ctx().content_rect().height() * 0.6;
            list_area(max_h, self.results.len(), 52.0).show(ui, |ui| {
                Frame::new().inner_margin(Margin::same(6)).show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 0.0;
                    for (i, r) in self.results.iter().enumerate() {
                        let sel = i == self.selected;
                        let resp = result_row(ui, r, i, sel, p, "search-row");
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                            self.selected = i;
                        }
                        if resp.clicked() {
                            out = PanelOutcome::Open(r.path.clone(), r.line);
                        }
                        if sel && self.scroll_to_selected {
                            resp.scroll_to_me(None);
                        }
                    }
                });
            });
        });
        self.opened = true;
        self.scroll_to_selected = false;
        if self.query != before {
            // Searches while typing, after a short pause.
            self.due = Some(Instant::now() + Duration::from_millis(200));
        }
        let mut run = None;
        if let Some(due) = self.due {
            let now = Instant::now();
            if now >= due {
                self.due = None;
                self.generation += 1;
                if self.query.trim().is_empty() {
                    self.results.clear();
                    self.status.clear();
                } else {
                    self.status = "buscando…".into();
                    run = Some(self.query.clone());
                }
            } else {
                ctx.request_repaint_after(due - now);
            }
        }
        if outside {
            return (PanelOutcome::Close, run);
        }
        (out, run)
    }
}

// ---------------------------------------------------------------- BacklinksPanel

pub struct BacklinksPanel {
    shown: usize,
    selected: usize,
    scroll_to_selected: bool,
}

impl BacklinksPanel {
    pub fn new() -> Self {
        BacklinksPanel { shown: usize::MAX, selected: 0, scroll_to_selected: false }
    }

    pub fn show(&mut self, ctx: &egui::Context, note: &str, results: &[SearchResult]) -> PanelOutcome {
        let p = palette_ctx(ctx);
        if results.len() != self.shown {
            self.shown = results.len();
            ctx.request_repaint();
        }
        let d = list_move(ctx, true);
        if d != 0 && !results.is_empty() {
            self.selected = wrap(self.selected, d, results.len());
            self.scroll_to_selected = true;
        }
        if take(ctx, Modifiers::NONE, Key::Escape) {
            return PanelOutcome::Close;
        }
        let mut out = PanelOutcome::None;
        if take(ctx, Modifiers::NONE, Key::Enter) {
            if let Some(r) = results.get(self.selected) {
                out = PanelOutcome::Open(r.path.clone(), r.line);
            }
        }
        let (_, outside) = kit::modal(ctx, "backlinks-panel", 0.10, 760.0, 0, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                    ui.label(kit::icon(icons::CORNER_UP_LEFT, 16.0));
                    ui.label(RichText::new("Notes linking to").font(fonts::ui(14.0)));
                    ui.label(RichText::new(strip_md(file_name(note))).font(fonts::ui_bold(14.0)));
                    ui.label(RichText::new(format!("({})", results.len())).font(fonts::ui(12.0)).color(p.muted));
                });
            });
            let y = ui.cursor().top();
            ui.painter().hline(ui.min_rect().x_range(), y, Stroke::new(1.0, p.border));
            let max_h = ui.ctx().content_rect().height() * 0.6;
            list_area(max_h, results.len(), 52.0).show(ui, |ui| {
                Frame::new().inner_margin(Margin::same(6)).show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 0.0;
                    if results.is_empty() {
                        ui.label(RichText::new("No notes link here yet.").color(p.muted).font(fonts::ui(13.0)));
                    }
                    for (i, r) in results.iter().enumerate() {
                        let sel = i == self.selected;
                        let resp = result_row(ui, r, i, sel, p, "backlink-row");
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                            self.selected = i;
                        }
                        if resp.clicked() {
                            out = PanelOutcome::Open(r.path.clone(), r.line);
                        }
                        if sel && self.scroll_to_selected {
                            resp.scroll_to_me(None);
                        }
                    }
                });
            });
        });
        self.scroll_to_selected = false;
        if outside {
            return PanelOutcome::Close;
        }
        out
    }
}

// ---------------------------------------------------------------- Outline

pub enum OutlineOutcome {
    None,
    Close,
    Jump(usize),
}

pub struct Outline {
    shown: usize,
    query: String,
    selected: usize,
    opened: bool,
    scroll_to_selected: bool,
}

impl Outline {
    pub fn new() -> Self {
        Outline { shown: usize::MAX, query: String::new(), selected: 0, opened: false, scroll_to_selected: false }
    }

    pub fn show(&mut self, ctx: &egui::Context, all: &[Heading]) -> OutlineOutcome {
        let p = palette_ctx(ctx);
        let items: Vec<&Heading> =
            if self.query.trim().is_empty() { all.iter().collect() } else { fuzzy::rank(&self.query, all, |h| h.title.as_str()) };
        if items.len() != self.shown {
            self.shown = items.len();
            ctx.request_repaint();
        }
        let d = list_move(ctx, false);
        if d != 0 && !items.is_empty() {
            self.selected = wrap(self.selected, d, items.len());
            self.scroll_to_selected = true;
        }
        if take(ctx, Modifiers::NONE, Key::Escape) || take(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::O) {
            return OutlineOutcome::Close;
        }
        let mut out = OutlineOutcome::None;
        if take(ctx, Modifiers::NONE, Key::Enter) {
            if let Some(h) = items.get(self.selected) {
                out = OutlineOutcome::Jump(h.line);
            }
        }
        let filtering = !self.query.trim().is_empty();
        let before = self.query.clone();
        let (_, outside) = kit::modal(ctx, "outline", 0.12, 560.0, 0, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let w = ui.available_width();
            input_frame(ui, p, |ui| {
                let r = panel_input(ui, Id::new("outline-input"), &mut self.query, "Go to heading…", p, w - 28.0);
                if !self.opened {
                    r.request_focus();
                }
            });
            let max_h = ui.ctx().content_rect().height() * 0.55;
            list_area(max_h, items.len(), 30.0).show(ui, |ui| {
                Frame::new().inner_margin(Margin::same(6)).show(ui, |ui| {
                    ui.spacing_mut().interact_size.y = 0.0;
                    if items.is_empty() {
                        let msg = if all.is_empty() { "This note has no headings." } else { "No heading matches." };
                        ui.label(RichText::new(msg).color(p.muted).font(fonts::ui(13.0)));
                    }
                    for (i, h) in items.iter().enumerate() {
                        let sel = i == self.selected;
                        let color = if sel { p.accent } else { p.text };
                        let indent = if filtering { 0.0 } else { (h.level as f32 - 1.0) * 14.0 };
                        let resp = Frame::new()
                            .fill(if sel { p.accent_bg } else { Color32::TRANSPARENT })
                            .corner_radius(6)
                            .inner_margin(Margin { left: (10.0 + indent) as i8, right: 10, top: 5, bottom: 5 })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                                    ui.label(RichText::new(format!("H{}", h.level)).font(fonts::mono(10.0)).color(p.muted));
                                    ui.label(RichText::new(&h.title).font(fonts::editor(16.0)).color(color));
                                });
                            })
                            .response;
                        let resp = ui.interact(resp.rect, Id::new(("outline-row", i)), Sense::click());
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                            self.selected = i;
                        }
                        if resp.clicked() {
                            out = OutlineOutcome::Jump(h.line);
                        }
                        if sel && self.scroll_to_selected {
                            resp.scroll_to_me(None);
                        }
                    }
                });
            });
        });
        self.opened = true;
        self.scroll_to_selected = false;
        if self.query != before {
            self.selected = 0;
        }
        if outside {
            return OutlineOutcome::Close;
        }
        out
    }
}
