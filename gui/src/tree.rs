// The file tree of the sidebar (Tree.svelte): folders open/closed by hand, the open note and
// document highlighted, their folders revealed when they change, buttons to create inside a
// folder on hover and a context menu on right click.

use std::collections::HashMap;

use eframe::egui::{self, Align2, Id, Pos2, Rect, Sense, Ui, Vec2};

use crate::api::Entry;
use crate::fonts;
use crate::icons;
use crate::kit;
use crate::links::is_md;

pub enum TreeAction {
    Open(String),
    NewNote(String),
    NewFolder(String),
    Context(Entry, Pos2),
}

#[derive(Default)]
pub struct TreeView {
    open: HashMap<String, bool>,
    revealed: String,
}

const ROW_H: f32 = 23.0;

impl TreeView {
    /// Expands the folders on the path of the open note/document only when it changes (including
    /// after rename/move): reloading the tree on every autosave does not reopen a folder that
    /// was collapsed by hand.
    pub fn reveal(&mut self, tree: &[Entry], active: &[&str]) {
        let key = active.join("\n");
        if key == self.revealed || tree.is_empty() {
            return;
        }
        let mut complete = true;
        for path in active {
            let mut level = tree;
            let parts: Vec<&str> = path.split('/').collect();
            for depth in 0..parts.len().saturating_sub(1) {
                let dir = parts[..=depth].join("/");
                match level.iter().find(|e| e.dir && e.path == dir) {
                    Some(e) => {
                        self.open.insert(dir, true);
                        level = e.kids();
                    }
                    None => {
                        // The tree may not have the folder yet (initial load, right after a rename).
                        complete = false;
                        break;
                    }
                }
            }
        }
        if complete {
            self.revealed = key;
        }
    }

    pub fn set_open(&mut self, dir: &str) {
        self.open.insert(dir.to_string(), true);
    }

    pub fn show(&mut self, ui: &mut Ui, entries: &[Entry], active: &[&str], expand_all: bool) -> Option<TreeAction> {
        let mut action = None;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        self.level(ui, entries, active, expand_all, 0, &mut action);
        action
    }

    fn level(&mut self, ui: &mut Ui, entries: &[Entry], active: &[&str], expand_all: bool, depth: usize, action: &mut Option<TreeAction>) {
        let p = kit::palette(ui);
        for e in entries {
            let width = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
            let pad = depth as f32 * 12.0 + 8.0;
            let font = fonts::ui(14.0);
            let row_hovered = ui.rect_contains_pointer(rect);
            if e.dir {
                let is_open = expand_all || self.open.get(&e.path).copied().unwrap_or(false);
                // The two buttons take the right end of the row; the folder button the rest.
                let btn_w = 26.0;
                let item_rect = Rect::from_min_max(rect.min, Pos2::new(rect.max.x - 2.0 * btn_w - 2.0, rect.max.y));
                if resp.hovered() && item_rect.contains(ui.input(|i| i.pointer.hover_pos()).unwrap_or_default()) {
                    ui.painter().rect_filled(item_rect, 4, p.hover);
                }
                let chevron = if is_open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT };
                let mut x = rect.left() + pad;
                ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, chevron, fonts::icons(14.0), p.muted);
                x += 14.0 + 5.0;
                let label_rect = Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new(item_rect.right() - 4.0, rect.bottom()));
                paint_truncated(ui, label_rect, &e.name, font.clone(), p.muted);
                let mut on_button = false;
                if row_hovered {
                    for (i, (ic, tip, is_note)) in [
                        (icons::FILE_PLUS, format!("New note in {}", e.path), true),
                        (icons::FOLDER_PLUS, format!("New subfolder in {}", e.path), false),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let b = Rect::from_min_size(
                            Pos2::new(rect.right() - (2 - i) as f32 * btn_w - 2.0 + 1.0, rect.top() + 2.0),
                            Vec2::new(btn_w - 2.0, ROW_H - 4.0),
                        );
                        let r = ui.interact(b, Id::new(("tree-new", &e.path, i)), Sense::click()).on_hover_text(tip);
                        on_button |= r.hovered();
                        let stroke = if r.hovered() { p.muted } else { p.border };
                        ui.painter().rect(b, 4, p.hover, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
                        ui.painter().text(b.center(), Align2::CENTER_CENTER, ic, fonts::icons(14.0), p.text);
                        if r.clicked() {
                            self.open.insert(e.path.clone(), true);
                            *action = Some(if is_note { TreeAction::NewNote(e.path.clone()) } else { TreeAction::NewFolder(e.path.clone()) });
                        }
                    }
                }
                if resp.clicked() && !on_button {
                    self.open.insert(e.path.clone(), !is_open);
                }
                if resp.secondary_clicked() {
                    let pos = resp.interact_pointer_pos().unwrap_or(rect.left_bottom());
                    *action = Some(TreeAction::Context(e.clone(), pos));
                }
                if is_open {
                    self.level(ui, e.kids(), active, expand_all, depth + 1, action);
                }
            } else {
                let is_active = active.contains(&e.path.as_str());
                if is_active {
                    ui.painter().rect_filled(rect, 4, p.accent_bg);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 4, p.hover);
                }
                let color = if is_active { p.accent } else { p.text };
                let mut x = rect.left() + pad;
                ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, icons::file_icon(&e.name), fonts::icons(14.0), color);
                x += 14.0 + 5.0;
                let name = if is_md(&e.name) && e.name.ends_with(".md") { &e.name[..e.name.len() - 3] } else { e.name.as_str() };
                let label_rect = Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new(rect.right() - 8.0, rect.bottom()));
                paint_truncated(ui, label_rect, name, font, color);
                let resp = resp.on_hover_text(&e.path);
                if resp.clicked() {
                    *action = Some(TreeAction::Open(e.path.clone()));
                }
                if resp.secondary_clicked() {
                    let pos = resp.interact_pointer_pos().unwrap_or(rect.left_bottom());
                    *action = Some(TreeAction::Context(e.clone(), pos));
                }
            }
        }
    }
}

/// Text cut with an ellipsis at the end of the rect (text-overflow: ellipsis).
pub fn paint_truncated(ui: &Ui, rect: Rect, text: &str, font: egui::FontId, color: egui::Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width().max(0.0));
    let galley = ui.painter().layout_job(job);
    let pos = Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(rect.expand(1.0)).galley(pos, galley, color);
}
