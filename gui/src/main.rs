//! quirenote-gui: the Quirenote interface as a native window (egui), talking to the same
//! server API the Svelte app uses in the browser.

#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod api;
mod app;
mod bridge;
mod dialogs;
mod editor;
mod epub;
mod font_settings;
mod fonts;
mod fuzzy;
mod hanging;
mod highlight;
mod icons;
mod kit;
mod links;
mod markdown;
mod pdf;
mod setup;
mod state;
mod theme;
mod tree;
mod undo;
mod vim;

use std::path::PathBuf;

const USAGE: &str =
    "usage: quirenote-gui [--server URL] [--note PATH] [--doc PATH] [--page N] [--state-dir DIR]

  --server URL     the Quirenote server (default: $QUIRENOTE_URL or http://127.0.0.1:8080)
  --note PATH      open this note (vault path), like #note=… in the web app's URL
  --doc PATH       open this PDF/EPUB of the vault, like #doc=…
  --page N         page of the PDF, like #page=…
  --state-dir DIR  where the window keeps its preferences and last session
                   (default: $XDG_STATE_HOME/quirenote-gui)";

fn main() {
    let mut start = app::Start {
        server: std::env::var("QUIRENOTE_URL")
            .or_else(|_| std::env::var("NOTAS_URL"))
            .unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
        state_dir: state::Store::dir_default(),
        note: None,
        doc: None,
        page: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| {
            args.next().unwrap_or_else(|| {
                eprintln!("{name} needs a value\n\n{USAGE}");
                std::process::exit(2)
            })
        };
        match a.as_str() {
            "--server" => start.server = value("--server"),
            "--note" => start.note = Some(value("--note")),
            "--doc" => start.doc = Some(value("--doc")),
            "--page" => start.page = value("--page").parse().ok(),
            "--state-dir" => start.state_dir = PathBuf::from(value("--state-dir")),
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Quirenote")
            .with_app_id("quirenote-gui")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Quirenote",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, start)))),
    );
    if let Err(e) = result {
        eprintln!("quirenote-gui: {e}");
        std::process::exit(1);
    }
}
