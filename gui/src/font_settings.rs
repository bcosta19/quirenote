// The font picker (FontSettings.svelte): a field per role (interface, notes, code) that shows
// the font it chooses, the list of fonts installed on this device under the field (filtered
// by typing, picked with the arrows and Enter or a click, each name drawn in its own font),
// a notice for a name this device does not have, reset per role and for all.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, ColorImage, Frame, Id, Key, Layout, Margin, Modifiers, Pos2, Rect,
    RichText, ScrollArea, Sense, Stroke, TextEdit, TextureHandle, TextureOptions, Vec2,
};

use crate::fonts::{self, ROLES, Role};
use crate::kit::{self, palette_ctx};
use crate::state::FontPrefs;

pub enum Outcome {
    None,
    Close,
}

pub struct FontSettings {
    /// None while the list is being read.
    installed: Option<Vec<String>>,
    files: Arc<HashMap<String, (String, u32)>>,
    open_role: Option<Role>,
    filtering: bool,
    active: Option<String>,
    opened: bool,
    reveal_active: bool,
    previews: HashMap<String, Option<(TextureHandle, Vec2)>>,
    missing: HashMap<String, bool>,
}

fn field_id(role: Role) -> Id {
    Id::new(("font-field", role.key()))
}

/// Families and the file of each (fc-list), for drawing every name in its own font.
pub fn list_with_files() -> (Vec<String>, HashMap<String, (String, u32)>) {
    let names = fonts::list_installed();
    let mut files = HashMap::new();
    if let Ok(out) = Command::new("fc-list")
        .arg(":")
        .arg("family")
        .arg("file")
        .arg("index")
        .arg("-f")
        .arg("%{family}\t%{file}\t%{index}\n")
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split('\t');
            let (Some(fam), Some(file)) = (it.next(), it.next()) else {
                continue;
            };
            let index = it
                .next()
                .and_then(|i| i.split(',').next()?.trim().parse().ok())
                .unwrap_or(0);
            let name = fonts::clean_font_name(fam.split(',').next().unwrap_or(""));
            // Prefer a regular face: the shortest file name of the family is usually it.
            let better = files
                .get(&name)
                .is_none_or(|(f, _): &(String, u32)| file.len() < f.len());
            if !name.is_empty() && better {
                files.insert(name, (file.to_string(), index));
            }
        }
    }
    (names, files)
}

/// The name drawn in its own font, as an alpha mask (ab_glyph), tinted when painted.
fn rasterize(name: &str, file: &str, index: u32, px: f32) -> Option<ColorImage> {
    use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
    let bytes = std::fs::read(file).ok()?;
    let font = FontVec::try_from_vec_and_index(bytes, index).ok()?;
    let scaled = font.as_scaled(PxScale::from(px));
    let mut glyphs = Vec::new();
    let mut x = 0.0f32;
    let mut prev = None;
    for c in name.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            x += scaled.kern(p, id);
        }
        glyphs.push(
            id.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, scaled.ascent())),
        );
        x += scaled.h_advance(id);
        prev = Some(id);
    }
    let w = x.ceil().max(1.0) as usize + 2;
    let h = (scaled.ascent() - scaled.descent()).ceil().max(1.0) as usize + 2;
    let mut alpha = vec![0u8; w * h];
    for g in glyphs {
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                let (px_, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px_ >= 0 && py >= 0 && (px_ as usize) < w && (py as usize) < h {
                    let a = &mut alpha[py as usize * w + px_ as usize];
                    *a = (*a).max((cov * 255.0) as u8);
                }
            });
        }
    }
    let rgba: Vec<u8> = alpha.iter().flat_map(|&a| [255, 255, 255, a]).collect();
    Some(ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

impl FontSettings {
    pub fn new() -> Self {
        FontSettings {
            installed: None,
            files: Arc::new(HashMap::new()),
            open_role: None,
            filtering: false,
            active: None,
            opened: false,
            reveal_active: false,
            previews: HashMap::new(),
            missing: HashMap::new(),
        }
    }

    pub fn listed(&mut self, names: Vec<String>, files: HashMap<String, (String, u32)>) {
        self.installed = Some(names);
        self.files = Arc::new(files);
    }

    fn suggestions(&self, prefs: &FontPrefs) -> Vec<String> {
        let (Some(role), Some(all)) = (self.open_role, &self.installed) else {
            return Vec::new();
        };
        let q = if self.filtering {
            prefs.get(role).to_lowercase()
        } else {
            String::new()
        };
        all.iter()
            .filter(|n| n.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    fn open_list(&mut self, role: Role, filter: bool, prefs: &FontPrefs) {
        self.open_role = Some(role);
        self.filtering = filter;
        // A name that is already one of the fonts starts as the highlighted one.
        let cur = prefs.get(role);
        self.active = self
            .installed
            .as_ref()
            .filter(|l| l.iter().any(|n| n == cur))
            .map(|_| cur.to_string());
        self.reveal_active = true;
    }

    /// Whether this device has the font (cached per name: fc-match is a process).
    fn is_missing(&mut self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        if let Some(&m) = self.missing.get(name) {
            return m;
        }
        let m = !fonts::available(name);
        self.missing.insert(name.to_string(), m);
        m
    }

    #[allow(clippy::too_many_lines)]
    pub fn show(&mut self, ctx: &egui::Context, prefs: &mut FontPrefs) -> Outcome {
        let p = palette_ctx(ctx);
        let list_open =
            |s: &Self, role: Role, sugg: &[String]| s.open_role == Some(role) && !sugg.is_empty();
        let mut sugg = self.suggestions(prefs);
        // The keys of the focused field first: the list's arrows, Enter and Escape.
        let focused_role = ROLES
            .into_iter()
            .find(|r| ctx.memory(|m| m.has_focus(field_id(*r))));
        if let Some(role) = focused_role {
            let open = list_open(self, role, &sugg);
            let (down, up, enter, esc) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    open && self.active.is_some() && i.consume_key(Modifiers::NONE, Key::Enter),
                    open && i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if down || up {
                if !open {
                    // A closed list (after Escape or a choice) comes back with the arrows.
                    self.open_list(role, false, prefs);
                } else {
                    let idx = self
                        .active
                        .as_ref()
                        .and_then(|a| sugg.iter().position(|n| n == a));
                    let last = sugg.len().saturating_sub(1);
                    let next = match idx {
                        None => {
                            if down {
                                0
                            } else {
                                last
                            }
                        }
                        Some(i) => {
                            if down {
                                (i + 1).min(last)
                            } else {
                                i.saturating_sub(1)
                            }
                        }
                    };
                    self.active = sugg.get(next).cloned();
                    self.reveal_active = true;
                }
            }
            if enter && let Some(a) = self.active.clone() {
                *prefs.get_mut(role) = a;
                self.open_role = None;
            }
            if esc {
                // Closes only the list: the dialog stays, and the next Escape closes it.
                self.open_role = None;
            }
        }
        // Keeps the app shortcuts from acting while typing here.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            return Outcome::Close;
        }
        let mut close = false;
        let mut list_rect: Option<(Role, Rect)> = None;
        let (_, outside) = kit::modal(ctx, "fonts-dialog", 0.18, 480.0, 16, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
            ui.label(RichText::new("Fonts").font(fonts::ui_bold(15.0)));
            let hint = match &self.installed {
                None => "Asking the system for the fonts installed on this device…".to_string(),
                Some(l) if !l.is_empty() => format!(
                    "{} font{} installed on this device. Any name also works, even one the system does not list.",
                    l.len(),
                    if l.len() == 1 { "" } else { "s" }
                ),
                Some(_) => "This system cannot list the fonts installed on this device: type the name, as in “Inter”.".to_string(),
            };
            ui.label(RichText::new(hint).font(fonts::ui(12.0)).color(p.muted));
            for role in ROLES {
                ui.add_space(8.0);
                ui.label(RichText::new(role.label()).font(fonts::ui(13.0)));
                ui.label(
                    RichText::new(role.hint())
                        .font(fonts::ui(12.0))
                        .color(p.muted),
                );
                ui.horizontal(|ui| {
                    let id = field_id(role);
                    let focused = ui.memory(|m| m.has_focus(id));
                    // The field itself uses the font it is choosing, so the choice is visible.
                    let font = match role {
                        Role::Ui => fonts::ui(13.0),
                        Role::Editor => fonts::editor(15.0),
                        Role::Mono => fonts::mono(13.0),
                    };
                    let reset_w = 30.0;
                    let w = ui.available_width() - reset_w - 8.0;
                    let r = Frame::new()
                        .fill(p.bg)
                        .stroke(Stroke::new(1.0, if focused { p.accent } else { p.border }))
                        .corner_radius(4)
                        .inner_margin(Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            let mut text = prefs.get(role).to_string();
                            let r = ui.add(
                                TextEdit::singleline(&mut text)
                                    .id(id)
                                    .event_filter(crate::dialogs::PANEL_KEYS)
                                    .frame(Frame::NONE)
                                    .font(font.clone())
                                    .desired_width(w - 16.0)
                                    .hint_text(
                                        RichText::new(format!("Default ({})", role.default_name()))
                                            .font(font.clone())
                                            .color(p.muted),
                                    ),
                            );
                            if r.changed() {
                                // Only what can be part of a font name; a dropped character
                                // disappears right away and the caret stays in place.
                                let kept = fonts::keep_font_chars(&text);
                                if kept != text {
                                    let caret = egui::TextEdit::load_state(ui.ctx(), id)
                                        .and_then(|s| s.cursor.char_range())
                                        .map_or(0, |c| c.primary.index.0);
                                    let before: String = text.chars().take(caret).collect();
                                    let new_caret = fonts::keep_font_chars(&before).chars().count();
                                    if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                                        st.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::one(
                                                egui::text::CCursor::new(new_caret),
                                            ),
                                        ));
                                        st.store(ui.ctx(), id);
                                    }
                                }
                                let clean = fonts::clean_font_name(&kept);
                                // Keep a trailing space being typed between two words.
                                *prefs.get_mut(role) = if kept.ends_with(' ') && !clean.is_empty() {
                                    format!("{clean} ")
                                } else {
                                    clean
                                };
                                self.open_role = Some(role);
                                self.filtering = true;
                                self.active = self
                                    .installed
                                    .as_ref()
                                    .filter(|l| l.iter().any(|n| n == prefs.get(role)))
                                    .map(|_| prefs.get(role).to_string());
                            }
                            r
                        })
                        .inner;
                    if !self.opened && role == ROLES[0] {
                        // Typing replaces the current name; the list stays closed on this first focus.
                        r.request_focus();
                        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                            let n = prefs.get(role).chars().count();
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                            st.store(ui.ctx(), id);
                        }
                    } else if r.gained_focus() || (r.clicked() && self.open_role != Some(role)) {
                        self.open_list(role, false, prefs);
                    }
                    if r.lost_focus()
                        && self.open_role == Some(role)
                        && !ui.ctx().input(|i| i.pointer.any_down())
                    {
                        self.open_role = None;
                    }
                    if self.open_role == Some(role) {
                        list_rect = Some((role, r.rect.expand2(Vec2::new(8.0, 5.0))));
                    }
                    let tip = format!("Default font for the {}", role.label().to_lowercase());
                    let reset = ui
                        .add_enabled(
                            !prefs.get(role).is_empty(),
                            egui::Button::new(kit::icon(crate::icons::ROTATE_CCW, 14.0))
                                .min_size(Vec2::new(reset_w, 26.0)),
                        )
                        .on_hover_text(tip);
                    if reset.clicked() {
                        prefs.get_mut(role).clear();
                    }
                });
                let name = fonts::clean_font_name(prefs.get(role));
                if self.is_missing(&name) {
                    ui.label(
                        RichText::new(
                            "Not found on this device: the default font is shown instead.",
                        )
                        .font(fonts::ui(12.0))
                        .color(p.warn),
                    );
                }
            }
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if kit::primary(ui, "Done", false, true).clicked() {
                    close = true;
                }
                let any = ROLES.iter().any(|r| !prefs.get(*r).is_empty());
                if ui
                    .add_enabled(any, egui::Button::new(kit::text("Reset all")))
                    .clicked()
                {
                    for r in ROLES {
                        prefs.get_mut(r).clear();
                    }
                }
            });
        });
        self.opened = true;
        // The suggestions, right under the field, over the dialog.
        sugg = self.suggestions(prefs);
        if let Some((role, rect)) = list_rect
            && !sugg.is_empty()
        {
            self.suggestion_list(ctx, role, rect, &sugg, prefs);
        }
        if close || outside {
            return Outcome::Close;
        }
        Outcome::None
    }

    fn suggestion_list(
        &mut self,
        ctx: &egui::Context,
        role: Role,
        field: Rect,
        sugg: &[String],
        prefs: &mut FontPrefs,
    ) {
        let p = palette_ctx(ctx);
        let max_h = 240.0f32.min(ctx.content_rect().height() * 0.4);
        let row_h = 15.0 * 1.3 + 10.0;
        let mut chosen = None;
        egui::Area::new(Id::new(("font-suggestions", role.key())))
            .order(egui::Order::Tooltip)
            .fixed_pos(Pos2::new(field.left(), field.bottom() + 4.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(p.bg)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(8)
                    .inner_margin(Margin::same(4))
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(64),
                    })
                    .show(ui, |ui| {
                        ui.set_width(field.width() - 8.0);
                        ScrollArea::vertical()
                            .max_height(max_h)
                            .auto_shrink([false, true])
                            .show_rows(ui, row_h, sugg.len(), |ui, range| {
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                for i in range {
                                    let name = &sugg[i];
                                    let (rect, resp) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), row_h),
                                        Sense::click(),
                                    );
                                    let active = self.active.as_deref() == Some(name.as_str());
                                    if active {
                                        ui.painter().rect_filled(rect, 5, p.accent_bg);
                                        if self.reveal_active {
                                            resp.scroll_to_me(None);
                                        }
                                    }
                                    if resp.hovered()
                                        && ui.input(|i| i.pointer.delta() != Vec2::ZERO)
                                    {
                                        self.active = Some(name.clone());
                                    }
                                    let color = if name == prefs.get(role) {
                                        p.accent
                                    } else {
                                        p.text
                                    };
                                    // Each option drawn in its own font.
                                    let preview = self.preview(ui.ctx(), name);
                                    match preview {
                                        Some((tex, size)) => {
                                            let scale = (row_h - 8.0) / size.y.max(1.0);
                                            let s = (size * scale)
                                                .min(Vec2::new(rect.width() - 16.0, row_h));
                                            let r = Rect::from_min_size(
                                                Pos2::new(
                                                    rect.left() + 8.0,
                                                    rect.center().y - s.y / 2.0,
                                                ),
                                                s,
                                            );
                                            egui::Image::new((tex.id(), s))
                                                .tint(color)
                                                .paint_at(ui, r);
                                        }
                                        None => {
                                            ui.painter().text(
                                                Pos2::new(rect.left() + 8.0, rect.center().y),
                                                egui::Align2::LEFT_CENTER,
                                                name,
                                                fonts::ui(15.0),
                                                color,
                                            );
                                        }
                                    }
                                    if resp.clicked() {
                                        chosen = Some(name.clone());
                                    }
                                }
                            });
                    });
            });
        self.reveal_active = false;
        if let Some(n) = chosen {
            *prefs.get_mut(role) = n;
            self.open_role = None;
        }
    }

    fn preview(&mut self, ctx: &egui::Context, name: &str) -> Option<(TextureHandle, Vec2)> {
        if let Some(p) = self.previews.get(name) {
            return p.clone();
        }
        let made = self
            .files
            .get(name)
            .and_then(|(file, index)| rasterize(name, file, *index, 30.0))
            .map(|img| {
                let size = Vec2::new(img.size[0] as f32, img.size[1] as f32);
                (
                    ctx.load_texture(format!("font-preview-{name}"), img, TextureOptions::LINEAR),
                    size,
                )
            });
        self.previews.insert(name.to_string(), made.clone());
        made
    }
}
