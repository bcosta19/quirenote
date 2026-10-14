// Small building blocks with the app's look (app.css): buttons with an icon and a label,
// the primary button, keyboard hints, dialogs over a dimmed window.

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, Frame, Id, Margin, Modal, Response, RichText,
    Stroke, Ui, Vec2,
};

use crate::fonts;
use crate::theme::{self, Palette};

pub fn icon(c: char, size: f32) -> RichText {
    RichText::new(c.to_string()).font(fonts::icons(size))
}

pub fn text(s: impl Into<String>) -> RichText {
    RichText::new(s.into()).font(fonts::ui(13.0))
}

/// A toolbar button: optional icon, optional label, the tooltip of the web app.
pub fn button(ui: &mut Ui, ic: Option<char>, label: &str, tip: &str) -> Response {
    button_ex(ui, ic, label, tip, false, false)
}

pub fn button_ex(
    ui: &mut Ui,
    ic: Option<char>,
    label: &str,
    tip: &str,
    active: bool,
    icon_after: bool,
) -> Response {
    let p = palette(ui);
    let color = if active { p.accent } else { p.text };
    let mut b = match (ic, label.is_empty()) {
        (Some(c), true) => Button::new(icon(c, 16.0).color(color)),
        (Some(c), false) if icon_after => {
            Button::new((text(label).color(color), icon(c, 16.0).color(color)))
        }
        (Some(c), false) => Button::new((icon(c, 16.0).color(color), text(label).color(color))),
        (None, _) => Button::new(text(label).color(color)),
    };
    b = b.gap(5.0).corner_radius(4).min_size(Vec2::new(0.0, 24.0));
    if active {
        b = b.stroke(Stroke::new(1.0, p.accent));
    }
    let r = ui.add(b);
    if tip.is_empty() {
        r
    } else {
        r.on_hover_text(tip)
    }
}

/// The accent-filled button of dialogs (OK, Done, Create vault…).
pub fn primary(ui: &mut Ui, label: &str, danger: bool, enabled: bool) -> Response {
    let p = palette(ui);
    let fill = if danger { p.error } else { p.accent };
    let b = Button::new(text(label).color(p.panel))
        .fill(fill)
        .stroke(Stroke::new(1.0, fill))
        .corner_radius(4)
        .min_size(Vec2::new(0.0, 24.0));
    ui.add_enabled(enabled, b)
}

pub fn palette(ui: &Ui) -> &'static Palette {
    palette_ctx(ui.ctx())
}

pub fn palette_ctx(ctx: &egui::Context) -> &'static Palette {
    if ctx.global_style().visuals.dark_mode {
        &theme::DARK
    } else {
        &theme::PAPER
    }
}

/// <kbd>: mono 12px, a thin border, the panel background.
pub fn kbd(ui: &mut Ui, s: &str) {
    let p = palette(ui);
    Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(3)
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(s).font(fonts::mono(12.0))).extend())
        });
}

/// The frame of dialogs and panels over the window (backdrop rgba(0,0,0,.25), 10px corners).
pub fn dialog_frame(p: &Palette, padding: i8) -> Frame {
    Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(padding))
        .shadow(egui::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(64),
        })
}

/// A modal centered horizontally, `top` (fraction of the window height) from the top, `width`
/// wide at most (min(width, 92vw)). Returns the content's result and whether it should close
/// (backdrop click; Escape is left to the caller, which may need it for something else first).
pub fn modal<R>(
    ctx: &egui::Context,
    id: &str,
    top: f32,
    width: f32,
    padding: i8,
    content: impl FnOnce(&mut Ui) -> R,
) -> (R, bool) {
    let p = palette_ctx(ctx);
    let screen = ctx.content_rect();
    let w = width.min(screen.width() * 0.92);
    let area = Modal::default_area(Id::new(id))
        .anchor(Align2::CENTER_TOP, Vec2::new(0.0, screen.height() * top));
    let r = Modal::new(Id::new(id))
        .area(area)
        .backdrop_color(theme::BACKDROP)
        .frame(dialog_frame(p, padding))
        .show(ctx, |ui| {
            ui.set_width(w - 2.0 * padding as f32);
            content(ui)
        });
    let clicked_outside = r.backdrop_response.clicked();
    (r.inner, clicked_outside)
}

/// A clickable row of a list (switcher, search, outline): highlighted when selected.
pub fn row_frame(selected: bool, p: &Palette) -> Frame {
    Frame::new()
        .fill(if selected {
            p.accent_bg
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(6)
        .inner_margin(Margin::symmetric(10, 6))
}

/// A single-line text field with the app look (bg, border, accent border when focused).
pub fn field(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    font: egui::FontId,
    bg: Color32,
) -> egui::Response {
    let p = palette(ui);
    let id = ui.next_auto_id();
    let focused = ui.memory(|m| m.has_focus(id));
    let stroke = Stroke::new(1.0, if focused { p.accent } else { p.border });
    Frame::new()
        .fill(bg)
        .stroke(stroke)
        .corner_radius(4)
        .inner_margin(Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .id(id)
                    .frame(Frame::NONE)
                    .hint_text(RichText::new(hint).color(p.muted).font(font.clone()))
                    .font(font)
                    .desired_width(f32::INFINITY),
            )
        })
        .inner
}
