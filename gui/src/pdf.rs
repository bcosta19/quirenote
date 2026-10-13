// The PDF reader (PdfReader.svelte, pdf.js): continuous vertical pages fitted to the width,
// zoom, the page number field, the last page read per document, and text selection to copy.
// Poppler renders the pages on a thread of its own; only the pages near the view keep a
// texture.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, ColorImage, Frame, Id, Key, Margin, Pos2, Rect, RichText, ScrollArea, Sense,
    Stroke, TextureHandle, TextureOptions, Ui, Vec2,
};

use crate::bridge::Bridge;
use crate::fonts;
use crate::icons;
use crate::kit;
use crate::theme::Palette;

enum Req {
    Open(u64, Vec<u8>),
    Render(u64, usize, f32),
    Select(u64, usize, [f64; 4]),
}

pub enum Resp {
    Opened(u64, Vec<Vec2>),
    Failed(u64, String),
    Rendered(u64, usize, f32, ColorImage),
    Selected(u64, usize, Vec<Rect>, String),
}

/// The poppler thread: owns the document (not Send), renders on request. Stale requests (a
/// page rendered at an old zoom) are dropped by the UI when they come back.
fn worker(rx: Receiver<Req>, bridge: Bridge) {
    let mut doc: Option<(u64, poppler::Document)> = None;
    while let Ok(first) = rx.recv() {
        // Keep only the newest render request per page.
        let mut batch = vec![first];
        while let Ok(r) = rx.try_recv() {
            batch.push(r);
        }
        let mut seen = HashSet::new();
        let mut todo = Vec::new();
        for r in batch.into_iter().rev() {
            match &r {
                Req::Render(g, p, _) => {
                    if seen.insert((*g, *p)) {
                        todo.push(r);
                    }
                }
                _ => todo.push(r),
            }
        }
        todo.reverse();
        // Opening comes first in its batch.
        todo.sort_by_key(|r| !matches!(r, Req::Open(..)));
        for r in todo {
            match r {
                Req::Open(g, data) => {
                    let bytes = glib::Bytes::from_owned(data);
                    match poppler::Document::from_bytes(&bytes, None) {
                        Ok(d) => {
                            let sizes = (0..d.n_pages())
                                .filter_map(|i| d.page(i))
                                .map(|p| {
                                    let (w, h) = p.size();
                                    Vec2::new(w as f32, h as f32)
                                })
                                .collect();
                            doc = Some((g, d));
                            bridge.post(move |app| app.pdf_resp(Resp::Opened(g, sizes)));
                        }
                        Err(e) => {
                            doc = None;
                            let msg = e.to_string();
                            bridge.post(move |app| app.pdf_resp(Resp::Failed(g, msg)));
                        }
                    }
                }
                Req::Render(g, page, scale) => {
                    let Some((dg, d)) = &doc else { continue };
                    if *dg != g {
                        continue;
                    }
                    if let Some(img) = render(d, page, scale) {
                        bridge.post(move |app| app.pdf_resp(Resp::Rendered(g, page, scale, img)));
                    }
                }
                Req::Select(g, page, r) => {
                    let Some((dg, d)) = &doc else { continue };
                    if *dg != g {
                        continue;
                    }
                    let Some(p) = d.page(page as i32) else { continue };
                    let mut rect = poppler::Rectangle::new();
                    rect.set_x1(r[0]);
                    rect.set_y1(r[1]);
                    rect.set_x2(r[2]);
                    rect.set_y2(r[3]);
                    let text = p.selected_text(poppler::SelectionStyle::Glyph, &mut rect).map(|s| s.to_string()).unwrap_or_default();
                    let mut rects = Vec::new();
                    if let Some(region) = p.selected_region(1.0, poppler::SelectionStyle::Glyph, &mut rect) {
                        for i in 0..region.num_rectangles() {
                            let ri = region.rectangle(i);
                            rects.push(Rect::from_min_size(
                                Pos2::new(ri.x() as f32, ri.y() as f32),
                                Vec2::new(ri.width() as f32, ri.height() as f32),
                            ));
                        }
                    }
                    bridge.post(move |app| app.pdf_resp(Resp::Selected(g, page, rects, text)));
                }
            }
        }
    }
}

fn render(d: &poppler::Document, page: usize, scale: f32) -> Option<ColorImage> {
    let p = d.page(page as i32)?;
    let (w, h) = p.size();
    let (pw, ph) = ((w * scale as f64).ceil() as i32, (h * scale as f64).ceil() as i32);
    if pw <= 0 || ph <= 0 || pw > 16_384 || ph > 16_384 {
        return None;
    }
    let mut surface = cairo::ImageSurface::create(cairo::Format::Rgb24, pw, ph).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok()?;
        cr.scale(scale as f64, scale as f64);
        p.render(&cr);
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().ok()?;
    let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph as usize {
        let row = &data[y * stride..y * stride + pw as usize * 4];
        for px in row.chunks_exact(4) {
            // Cairo's RGB24 is 0xXXRRGGBB in native (little-endian) order: B G R X.
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    Some(ColorImage::from_rgba_unmultiplied([pw as usize, ph as usize], &rgba))
}

#[derive(Clone, Copy, PartialEq)]
enum Zoom {
    /// pdf.js "page-width".
    Width,
    /// CSS pixels per PDF point.
    Scale(f32),
}

struct Selection {
    page: usize,
    a: Pos2,
    b: Pos2,
    rects: Vec<Rect>,
    text: String,
}

pub struct PdfReader {
    pub id: String,
    pub title: String,
    tx: Sender<Req>,
    generation: u64,
    sizes: Vec<Vec2>,
    pub total: usize,
    /// The page shown (1 = first).
    pub page: u32,
    zoom: Zoom,
    /// Pixels per point used for the textures (zoom × screen scale).
    textures: HashMap<usize, (f32, TextureHandle)>,
    requested: HashSet<(usize, u32)>,
    pub error: String,
    /// Scroll so this page is at the top at the next frame.
    goto: Option<(u32, f32)>,
    scroll_by: f32,
    page_input: String,
    editing_page: bool,
    selection: Option<Selection>,
    last_width: f32,
    resize_due: Option<Instant>,
    fit_scale: f32,
    applied_scale: f32,
    start_page: u32,
    loaded: bool,
    ctx: egui::Context,
    /// How far into the current page the view starts (0..1), kept across zoom changes.
    page_fraction: f32,
}

const GAP: f32 = 10.0;
const SCROLL_STEP: f32 = 60.0;
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl PdfReader {
    pub fn new(bridge: &Bridge, id: String, title: String, start_page: u32) -> Self {
        let (tx, rx) = channel();
        let b = bridge.clone();
        std::thread::Builder::new().name("pdf".into()).spawn(move || worker(rx, b)).ok();
        PdfReader {
            id,
            title,
            tx,
            generation: 0,
            sizes: Vec::new(),
            total: 0,
            page: start_page.max(1),
            zoom: Zoom::Width,
            textures: HashMap::new(),
            requested: HashSet::new(),
            error: String::new(),
            goto: None,
            scroll_by: 0.0,
            page_input: start_page.max(1).to_string(),
            editing_page: false,
            selection: None,
            last_width: 0.0,
            resize_due: None,
            fit_scale: 1.0,
            applied_scale: 1.0,
            start_page: start_page.max(1),
            loaded: false,
            ctx: bridge.ctx().clone(),
            page_fraction: 0.0,
        }
    }

    /// The document bytes arrived (vault download or local file).
    pub fn open(&mut self, data: Vec<u8>) {
        self.generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.textures.clear();
        self.requested.clear();
        self.sizes.clear();
        self.total = 0;
        self.error.clear();
        self.loaded = false;
        let _ = self.tx.send(Req::Open(self.generation, data));
    }

    pub fn fail(&mut self, msg: String) {
        self.error = msg;
    }

    pub fn resp(&mut self, r: Resp) {
        match r {
            Resp::Opened(g, sizes) if g == self.generation => {
                self.total = sizes.len();
                self.sizes = sizes;
                self.loaded = true;
                // Page requested by the link, otherwise the last one read.
                let p = self.start_page.clamp(1, self.total.max(1) as u32);
                self.page = p;
                self.page_input = p.to_string();
                self.goto = Some((p, 0.0));
            }
            Resp::Failed(g, e) if g == self.generation => self.error = e,
            Resp::Rendered(g, page, scale, img) if g == self.generation => {
                self.requested.remove(&(page, scale.to_bits()));
                if (scale - self.applied_scale).abs() < 1e-3 || !self.textures.contains_key(&page) {
                    // One texture per page, replaced when the zoom changes.
                    let tex = self.ctx.load_texture(format!("pdf-{}-{page}", self.generation), img, TextureOptions::LINEAR);
                    self.textures.insert(page, (scale, tex));
                }
            }
            Resp::Selected(g, page, rects, text) if g == self.generation => {
                if let Some(s) = &mut self.selection {
                    if s.page == page {
                        s.rects = rects;
                        s.text = text;
                    }
                }
            }
            _ => {}
        }
    }

    /// A [[book.pdf#page=N]] link clicked with the PDF already open, or Alt+Shift+↑/↓.
    pub fn go_to_page(&mut self, page: u32) {
        if self.total == 0 {
            self.start_page = page.max(1);
            return;
        }
        let p = page.clamp(1, self.total as u32);
        self.page = p;
        self.page_input = p.to_string();
        self.goto = Some((p, 0.0));
    }

    /// Scrolls the PDF (called by the shortcuts while the editor has focus).
    pub fn scroll(&mut self, dir: i32) {
        self.scroll_by += dir as f32 * SCROLL_STEP;
    }

    pub fn turn_page(&mut self, dir: i32) {
        if self.total > 0 {
            let p = (self.page as i64 + dir as i64).clamp(1, self.total as i64) as u32;
            self.go_to_page(p);
        }
    }

    fn scale(&self) -> f32 {
        match self.zoom {
            Zoom::Width => self.fit_scale,
            Zoom::Scale(s) => s,
        }
    }

    fn zoom_by(&mut self, factor: f32) {
        let s = (self.scale() * factor).clamp(0.1, 10.0);
        self.zoom = Zoom::Scale(s);
        self.keep_page();
    }

    fn keep_page(&mut self) {
        self.goto = Some((self.page, self.page_fraction));
    }

    /// Draws the reader. Returns the page shown, when it changed.
    #[allow(clippy::too_many_lines)]
    pub fn ui(&mut self, ui: &mut Ui, p: &Palette) -> Option<u32> {
        let before = self.page;
        // Toolbar: title, page field, total, zoom out/in, fit to width.
        Frame::new().inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
                let right = 3.0 * 32.0 + 60.0 + 70.0;
                let w = (ui.available_width() - right).max(40.0);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::hover());
                crate::tree::paint_truncated(ui, r, crate::links::file_name(&self.title), fonts::ui(13.0), p.text);
                resp.on_hover_text(&self.title);
                let id = Id::new(("pdf-page", &self.id));
                let field = kit_page_field(ui, id, &mut self.page_input, p);
                if field.gained_focus() {
                    self.editing_page = true;
                }
                if field.lost_focus() {
                    self.editing_page = false;
                    if let Ok(n) = self.page_input.trim().parse::<u32>() {
                        if n >= 1 && (n as usize) <= self.total {
                            self.go_to_page(n);
                        }
                    }
                    self.page_input = self.page.to_string();
                }
                let total = if self.total > 0 { self.total.to_string() } else { "…".into() };
                ui.label(RichText::new(format!("/ {total}")).font(fonts::ui(13.0)).color(p.muted));
                if kit::button(ui, Some(icons::MINUS), "", "Zoom out").clicked() {
                    self.zoom_by(1.0 / 1.15);
                }
                if kit::button(ui, Some(icons::PLUS), "", "Zoom in").clicked() {
                    self.zoom_by(1.15);
                }
                if kit::button(ui, Some(icons::MOVE_HORIZONTAL), "", "Fit to width").clicked() {
                    self.zoom = Zoom::Width;
                    self.keep_page();
                }
            });
        });
        let y = ui.cursor().top();
        ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.border));
        if !self.error.is_empty() {
            Frame::new().inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.label(RichText::new(format!("Could not open the PDF: {}", self.error)).color(p.error).font(fonts::ui(13.0)));
            });
        }
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, p.reader_bg);
        if area.width() < 1.0 {
            return None;
        }
        // Refits to the width when the panel is resized, debounced so the PDF is not
        // re-rendered on every pixel while the divider is dragged.
        let width = area.width();
        if (width - self.last_width).abs() > 0.5 {
            if self.last_width == 0.0 {
                self.fit_scale = ((width - 40.0) / self.max_page_width()).max(0.1);
            } else {
                self.resize_due = Some(Instant::now() + Duration::from_millis(120));
            }
            self.last_width = width;
        }
        if let Some(due) = self.resize_due {
            if Instant::now() >= due {
                self.resize_due = None;
                self.fit_scale = ((width - 40.0) / self.max_page_width()).max(0.1);
                if self.zoom == Zoom::Width {
                    self.keep_page();
                }
            } else {
                ui.ctx().request_repaint_after(due - Instant::now());
            }
        }
        if self.loaded && self.fit_scale <= 0.0 {
            self.fit_scale = 1.0;
        }
        let scale = self.scale();
        let ppp = ui.ctx().pixels_per_point();
        let tex_scale = scale * ppp;
        if (tex_scale - self.applied_scale).abs() > 1e-3 {
            self.applied_scale = tex_scale;
            self.requested.clear();
        }

        // Page positions at this zoom.
        let mut tops = Vec::with_capacity(self.sizes.len());
        let mut y = GAP;
        for s in &self.sizes {
            tops.push(y);
            y += s.y * scale + GAP;
        }
        let content_h = y;
        let content_w = self.sizes.iter().map(|s| s.x * scale).fold(0.0, f32::max) + 2.0 * GAP;

        let mut sa = ScrollArea::both().id_salt(("pdf", &self.id)).auto_shrink([false, false]);
        if let Some((pg, frac)) = self.goto.take() {
            if let Some(&top) = tops.get(pg as usize - 1) {
                let h = self.sizes.get(pg as usize - 1).map_or(0.0, |s| s.y * scale);
                sa = sa.vertical_scroll_offset((top - GAP + frac * h).max(0.0));
            } else if !self.loaded {
                self.goto = Some((pg, frac));
            }
        }
        let scroll_by = std::mem::take(&mut self.scroll_by);
        let mut visible: Vec<(usize, f32)> = Vec::new();
        let mut copy_text: Option<String> = None;
        let out = sa.show_viewport(ui, |ui, viewport| {
            ui.set_min_size(Vec2::new(content_w.max(viewport.width()), content_h));
            let origin = ui.min_rect().min;
            let x_center = origin.x + content_w.max(viewport.width()) / 2.0;
            for (i, s) in self.sizes.iter().enumerate() {
                let top = tops[i];
                let size = *s * scale;
                let r = Rect::from_min_size(Pos2::new(x_center - size.x / 2.0, origin.y + top), size);
                let vis_top = top.max(viewport.top());
                let vis_bot = (top + size.y).min(viewport.bottom());
                let near = top + size.y > viewport.top() - viewport.height() && top < viewport.bottom() + viewport.height();
                if vis_bot > vis_top {
                    visible.push((i, vis_bot - vis_top));
                }
                if !near {
                    continue;
                }
                // pdf.js draws a soft shadow border around each page.
                ui.painter().rect_filled(r.expand(1.0), 0.0, Color32::from_black_alpha(40));
                ui.painter().rect_filled(r, 0.0, Color32::WHITE);
                match self.textures.get(&i) {
                    Some((_, tex)) => {
                        egui::Image::new((tex.id(), size)).paint_at(ui, r);
                    }
                    None => {}
                }
                let have = self.textures.get(&i).is_some_and(|(sc, _)| (sc - tex_scale).abs() < 1e-3);
                if !have && self.requested.insert((i, tex_scale.to_bits())) {
                    let _ = self.tx.send(Req::Render(self.generation, i, tex_scale));
                }
                // Text selection: drag over a page, Ctrl+C copies.
                let resp = ui.interact(r, Id::new(("pdf-page-area", &self.id, i)), Sense::click_and_drag());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                }
                let to_page = |p: Pos2| ((p - r.min) / scale).to_pos2();
                if resp.drag_started() {
                    if let Some(pp) = resp.interact_pointer_pos() {
                        let a = to_page(pp);
                        self.selection = Some(Selection { page: i, a, b: a, rects: Vec::new(), text: String::new() });
                    }
                }
                if resp.dragged() {
                    if let (Some(pp), Some(sel)) = (resp.interact_pointer_pos(), &mut self.selection) {
                        if sel.page == i {
                            sel.b = to_page(pp);
                            let _ = self.tx.send(Req::Select(self.generation, i, [sel.a.x as f64, sel.a.y as f64, sel.b.x as f64, sel.b.y as f64]));
                        }
                    }
                }
                if resp.clicked() {
                    self.selection = None;
                }
                if let Some(sel) = &self.selection {
                    if sel.page == i {
                        for sr in &sel.rects {
                            let rr = Rect::from_min_max(r.min + sr.min.to_vec2() * scale, r.min + sr.max.to_vec2() * scale);
                            ui.painter().rect_filled(rr, 0.0, Color32::from_rgba_unmultiplied(0, 100, 255, 70));
                        }
                    }
                }
            }
            if scroll_by != 0.0 {
                ui.scroll_with_delta(Vec2::new(0.0, -scroll_by));
            }
            if ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
                if let Some(sel) = &self.selection {
                    if !sel.text.is_empty() {
                        copy_text = Some(sel.text.clone());
                    }
                }
            }
        });
        if let Some(t) = copy_text {
            ui.ctx().copy_text(t);
        }
        let offset = out.state.offset.y;
        // The current page: the most visible one (ties: the first).
        if let Some(&(best, _)) = visible.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(b.0.cmp(&a.0))) {
            let pg = best as u32 + 1;
            if pg != self.page && self.goto.is_none() {
                self.page = pg;
                if !self.editing_page {
                    self.page_input = pg.to_string();
                }
            }
        }
        if let (Some(&top), Some(s)) = (tops.get(self.page as usize - 1), self.sizes.get(self.page as usize - 1)) {
            self.page_fraction = ((offset - (top - GAP)) / (s.y * scale)).clamp(0.0, 1.0);
        }
        // Textures far from the view are dropped (pdf.js keeps a small page cache too).
        let keep: HashSet<usize> = visible.iter().flat_map(|(i, _)| i.saturating_sub(1)..=i + 1).collect();
        self.textures.retain(|i, _| keep.contains(i));
        (self.page != before).then_some(self.page)
    }

    fn max_page_width(&self) -> f32 {
        self.sizes.iter().map(|s| s.x).fold(0.0, f32::max).max(1.0)
    }
}

fn kit_page_field(ui: &mut Ui, id: Id, text: &mut String, p: &Palette) -> egui::Response {
    let focused = ui.memory(|m| m.has_focus(id));
    Frame::new()
        .fill(p.hover)
        .stroke(Stroke::new(1.0, if focused { p.accent } else { p.border }))
        .corner_radius(4)
        .inner_margin(Margin::symmetric(6, 3))
        .show(ui, |ui| {
            let r = ui.add(egui::TextEdit::singleline(text).id(id).frame(Frame::NONE).font(fonts::ui(13.0)).desired_width(40.0));
            if r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                r.surrender_focus();
            }
            r
        })
        .inner
}
