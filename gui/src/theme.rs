// The two themes of the app (web/src/app.css and web/src/lib/theme.ts): "paper" (warm beige,
// brown ink) and dark; plus the editor colors (CodeMirror's paper theme and One Dark).

use eframe::egui::{self, Color32, CornerRadius, Shadow, Stroke, Visuals};

use crate::state::Theme;

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub border: Color32,
    pub hover: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_bg: Color32,
    pub reader_bg: Color32,
    pub code_bg: Color32,
    pub scroll_thumb: Color32,
    pub page_bg: Color32,
    pub heading: Color32,
    pub link: Color32,
    pub warn: Color32,
    pub warn_bg: Color32,
    pub error: Color32,
    pub error_bg: Color32,
    pub editor: EditorColors,
}

/// Colors of the editor (CodeMirror theme + markdown highlight style).
#[derive(Clone, Copy, Debug)]
pub struct EditorColors {
    pub bg: Color32,
    pub text: Color32,
    pub caret: Color32,
    pub selection: Color32,
    pub gutter: Color32,
    pub search_match: Color32,
    pub tooltip_bg: Color32,
    pub tooltip_border: Color32,
    pub tooltip_selected: Color32,
    pub heading: Color32,
    /// # * - ` and the other markup characters.
    pub marks: Color32,
    pub link: Color32,
    pub url: Color32,
    pub link_underline: bool,
    pub quote: Color32,
    pub quote_italic: bool,
    pub monospace: Color32,
    pub meta: Color32,
    pub keyword: Color32,
    pub string: Color32,
    pub number: Color32,
}

pub const PAPER: Palette = Palette {
    bg: hex(0xe9e0cd),
    panel: hex(0xe0d6c1),
    border: hex(0xcfc2a7),
    hover: hex(0xd8ccb3),
    text: hex(0x332e26),
    muted: hex(0x8a8170),
    accent: hex(0x9a5b2e),
    accent_bg: hex(0xd9c6a3),
    reader_bg: hex(0xd6cab2),
    // rgba(107, 79, 58, 0.06) over the page
    code_bg: Color32::from_rgba_premultiplied(6, 5, 3, 15),
    scroll_thumb: hex(0xb9a888),
    page_bg: hex(0xf2eadb),
    heading: hex(0x8b4a2b),
    link: hex(0x4b6a88),
    warn: hex(0x9a6a00),
    warn_bg: hex(0xf3e2b3),
    error: hex(0xb3261e),
    error_bg: hex(0xf2d2cc),
    editor: EditorColors {
        bg: hex(0xf2eadb),
        text: hex(0x332e26),
        caret: hex(0x9a5b2e),
        selection: hex(0xddcba8),
        gutter: hex(0xb3a78c),
        search_match: hex(0xe9d190),
        tooltip_bg: hex(0xe0d6c1),
        tooltip_border: hex(0xcfc2a7),
        tooltip_selected: hex(0xd9c6a3),
        heading: hex(0x8b4a2b),
        marks: hex(0xb8ab90),
        link: hex(0x4b6a88),
        url: hex(0x4b6a88),
        link_underline: false,
        quote: hex(0x6f6553),
        quote_italic: true,
        monospace: hex(0x6b4f3a),
        meta: hex(0xa09680),
        keyword: hex(0x8b4a2b),
        string: hex(0x5f7a3a),
        number: hex(0x9a5b2e),
    },
};

pub const DARK: Palette = Palette {
    bg: hex(0x1e1f22),
    panel: hex(0x232428),
    border: hex(0x33353a),
    hover: hex(0x2e3035),
    text: hex(0xdcdde0),
    muted: hex(0x8b8f98),
    accent: hex(0x8ab4f8),
    accent_bg: hex(0x2a3445),
    reader_bg: hex(0x2b2b2b),
    // rgba(255, 255, 255, 0.04)
    code_bg: Color32::from_rgba_premultiplied(10, 10, 10, 10),
    scroll_thumb: hex(0x4a4d55),
    page_bg: hex(0x282c34),
    heading: hex(0xe06c75),
    link: hex(0x61afef),
    warn: hex(0xf0b429),
    warn_bg: hex(0x4a3b10),
    error: hex(0xff8a8a),
    error_bg: hex(0x4a1717),
    editor: EditorColors {
        bg: hex(0x282c34),
        text: hex(0xabb2bf),
        caret: hex(0x528bff),
        selection: hex(0x3e4451),
        gutter: hex(0x7d8799),
        search_match: hex(0x42557b),
        tooltip_bg: hex(0x353a42),
        tooltip_border: hex(0x353a42),
        tooltip_selected: hex(0x2c313a),
        heading: hex(0xe06c75),
        marks: hex(0x98c379),
        link: hex(0x7d8799),
        url: hex(0x56b6c2),
        link_underline: true,
        quote: hex(0xabb2bf),
        quote_italic: false,
        monospace: hex(0xabb2bf),
        meta: hex(0x7d8799),
        keyword: hex(0xc678dd),
        string: hex(0x98c379),
        number: hex(0xd19a66),
    },
};

pub fn palette(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Light => &PAPER,
        Theme::Dark => &DARK,
    }
}

/// egui's look from the palette: buttons and inputs like app.css (hover background, 1px border,
/// 4px corners), dialogs with 10px corners and a soft shadow.
pub fn visuals(theme: Theme) -> Visuals {
    let p = palette(theme);
    let mut v = if theme == Theme::Dark { Visuals::dark() } else { Visuals::light() };
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.link;
    v.faint_bg_color = p.panel;
    v.extreme_bg_color = p.bg;
    v.text_edit_bg_color = Some(p.hover);
    v.code_bg_color = p.code_bg;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.error;
    v.window_fill = p.panel;
    v.panel_fill = p.bg;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 12], blur: 40, spread: 0, color: Color32::from_black_alpha(64) };
    v.popup_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(51) };
    v.selection.bg_fill = p.accent_bg;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.text_cursor.stroke = Stroke::new(1.5, p.editor.caret);
    v.striped = false;
    v.indent_has_left_vline = false;
    v.handle_shape = egui::style::HandleShape::Circle;
    let r = CornerRadius::same(4);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.bg_fill = p.hover;
        w.weak_bg_fill = p.hover;
        w.bg_stroke = Stroke::new(1.0, p.border);
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.corner_radius = r;
        w.expansion = 0.0;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.muted);
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.noninteractive.bg_fill = p.panel;
    v.widgets.noninteractive.weak_bg_fill = p.panel;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.corner_radius = r;
    v
}

/// The window background behind dialogs: rgba(0, 0, 0, 0.25).
pub const BACKDROP: Color32 = Color32::from_black_alpha(64);
