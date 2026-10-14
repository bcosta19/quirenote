// First-run screen (Setup.svelte): start a new vault or use an existing folder of notes.

use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, TextEdit, Ui, Vec2};

use crate::api::{SetupDone, SetupStatus};
use crate::fonts;
use crate::icons;
use crate::kit;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    New,
    Existing,
}

pub struct Setup {
    status: SetupStatus,
    mode: Mode,
    new_path: String,
    existing_path: String,
    pub error: String,
    pub busy: bool,
    pub done: Option<SetupDone>,
    /// When to enter the app after a successful setup.
    pub enter_at: Option<Instant>,
    focus_existing: bool,
    /// The card's height at the last frame, to center it.
    card_h: f32,
}

pub enum Action {
    None,
    Submit(String, String),
}

impl Setup {
    pub fn new(status: SetupStatus) -> Self {
        let suggested = status.suggested.clone().unwrap_or_else(|| "~/Quirenote".into());
        Setup {
            status,
            mode: Mode::New,
            new_path: suggested,
            existing_path: String::new(),
            error: String::new(),
            busy: false,
            done: None,
            enter_at: None,
            focus_existing: false,
            card_h: 420.0,
        }
    }

    pub fn finished(&mut self, res: Result<SetupDone, String>) {
        match res {
            Ok(d) => {
                // An Obsidian vault gets a moment to read the confirmation; otherwise go straight in.
                self.enter_at = Some(Instant::now() + if d.obsidian { Duration::from_millis(1500) } else { Duration::ZERO });
                self.done = Some(d);
            }
            Err(e) => {
                self.error = e;
                self.busy = false;
            }
        }
    }

    fn option(&mut self, ui: &mut Ui, mode: Mode, icon: char, title: &str, desc: &str) {
        let p = kit::palette(ui);
        let selected = self.mode == mode;
        let r = Frame::new()
            .fill(if selected { p.hover } else { egui::Color32::TRANSPARENT })
            .stroke(Stroke::new(1.0, if selected { p.accent } else { p.border }))
            .corner_radius(8)
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    // The radio button.
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    let c = rect.center() + Vec2::new(0.0, 3.0);
                    ui.painter().circle_stroke(c, 6.5, Stroke::new(1.5, if selected { p.accent } else { p.muted }));
                    if selected {
                        ui.painter().circle_filled(c, 3.5, p.accent);
                    }
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(kit::icon(icon, 16.0));
                            ui.label(RichText::new(title).font(fonts::ui_bold(14.0)));
                        });
                        ui.label(RichText::new(desc).font(fonts::ui(13.0)).color(p.muted));
                        if selected {
                            ui.add_space(2.0);
                            let (text, hint, id) = match mode {
                                Mode::New => (&mut self.new_path, "", "setup-new"),
                                Mode::Existing => (&mut self.existing_path, "~/Documents/My vault", "setup-existing"),
                            };
                            let id = egui::Id::new(id);
                            let focused = ui.memory(|m| m.has_focus(id));
                            let r = Frame::new()
                                .fill(p.bg)
                                .stroke(Stroke::new(1.0, if focused { p.accent } else { p.border }))
                                .corner_radius(4)
                                .inner_margin(Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    ui.add(
                                        TextEdit::singleline(text)
                                            .id(id)
                                            .frame(Frame::NONE)
                                            .font(fonts::mono(13.0))
                                            .hint_text(RichText::new(hint).font(fonts::mono(13.0)).color(p.muted))
                                            .desired_width(f32::INFINITY),
                                    )
                                })
                                .inner;
                            if mode == Mode::Existing && self.focus_existing {
                                r.request_focus();
                                self.focus_existing = false;
                            }
                        }
                    });
                });
            })
            .response;
        if ui.interact(r.rect, egui::Id::new(("setup-option", title)), Sense::click()).clicked() && self.mode != mode {
            self.mode = mode;
            self.focus_existing = mode == Mode::Existing;
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Action {
        let p = kit::palette(ui);
        let mut action = Action::None;
        let submit_key = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, p.bg);
        let w = 560.0f32.min(rect.width() - 48.0);
        let top = ((rect.height() - self.card_h) / 2.0).max(24.0);
        let left = ((rect.width() - w) / 2.0).max(24.0);
        let card_rect = Rect::from_min_size(rect.min + Vec2::new(left, top), Vec2::new(w, rect.height()));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(card_rect).layout(Layout::top_down(Align::Min)));
        {
            let ui = &mut child;
            let r = kit::dialog_frame(p, 28).corner_radius(12).show(ui, |ui| {
                ui.set_width(w - 56.0);
                ui.spacing_mut().item_spacing.y = 12.0;
                ui.label(RichText::new("Welcome to Quirenote").font(fonts::font(fonts::Role::Editor, true, false, 30.0)).color(p.heading));
                ui.label(
                    RichText::new("Your notes are plain markdown files in a folder on this machine. Choose where they live.")
                        .font(fonts::ui(14.0))
                        .color(p.muted),
                );
                self.option(ui, Mode::New, icons::FOLDER_PLUS, "Start fresh", "Creates a new folder with a welcome note.");
                self.option(
                    ui,
                    Mode::Existing,
                    icons::FOLDER_OPEN,
                    "Use an existing folder",
                    "An Obsidian vault works as is: nothing is copied, and your notes keep working in Obsidian, Neovim or any editor.",
                );
                if !self.error.is_empty() {
                    ui.label(RichText::new(&self.error).font(fonts::ui(13.0)).color(p.error));
                }
                if let Some(d) = &self.done {
                    let msg = format!("{}Opening {}…", if d.obsidian { "Obsidian vault detected. " } else { "" }, d.dir);
                    ui.label(RichText::new(msg).font(fonts::ui(13.0)).color(p.accent));
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Paths are on the machine running the server; ~ is its home folder.").font(fonts::ui(12.0)).color(p.muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.mode == Mode::New { "Create vault" } else { "Open folder" };
                        let enabled = !self.busy && self.status.allowed;
                        let b = egui::Button::new(kit::text(label).color(p.panel)).fill(p.accent).stroke(Stroke::new(1.0, p.accent)).corner_radius(4).min_size(Vec2::new(0.0, 30.0));
                        if ui.add_enabled(enabled, b).clicked() || (submit_key && enabled) {
                            let path = if self.mode == Mode::New { &self.new_path } else { &self.existing_path }.trim().to_string();
                            if path.is_empty() {
                                self.error = "Type the folder path.".into();
                            } else {
                                self.busy = true;
                                self.error.clear();
                                let mode = if self.mode == Mode::New { "new" } else { "existing" };
                                action = Action::Submit(mode.into(), path);
                            }
                        }
                    });
                });
                if !self.status.allowed {
                    ui.label(
                        RichText::new("Choosing the vault from the browser is disabled. Run quirenote init <folder>.")
                            .font(fonts::ui(13.0))
                            .color(p.error),
                    );
                }
            });
            if (r.response.rect.height() - self.card_h).abs() > 0.5 {
                self.card_h = r.response.rect.height();
                ui.ctx().request_repaint();
            }
        }
        action
    }
}
