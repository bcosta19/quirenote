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

fn uses_wayland(display: Option<&str>, socket: Option<&str>) -> bool {
    // Match winit's backend selection: Wayland takes precedence over DISPLAY.
    // winit no longer uses the legacy WINIT_UNIX_BACKEND variable.
    display
        .filter(|value| !value.is_empty())
        .or(socket)
        .is_some_and(|value| !value.is_empty())
}

fn native_options(wayland: bool) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Quirenote")
            .with_app_id("quirenote-gui")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    if wayland {
        // EGL can wait for a frame callback during a vsynced buffer swap. A
        // compositor may withhold that callback for windows on hidden workspaces,
        // blocking the UI thread and preventing it from answering Wayland pings.
        // egui still repaints on demand; disabling vsync does not create a render loop.
        options.glow_options.vsync = false;
    }
    options
}

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
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    let wayland_socket = std::env::var("WAYLAND_SOCKET").ok();
    let options = native_options(
        cfg!(target_os = "linux")
            && uses_wayland(wayland_display.as_deref(), wayland_socket.as_deref()),
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_selection_matches_winit_environment_fallbacks() {
        assert!(uses_wayland(Some("wayland-0"), None));
        assert!(uses_wayland(None, Some("7")));
        assert!(uses_wayland(Some(""), Some("7")));
        assert!(!uses_wayland(None, None));
        assert!(!uses_wayland(Some(""), Some("")));
    }

    #[test]
    fn wayland_swaps_do_not_wait_for_hidden_window_frame_callbacks() {
        assert!(!native_options(true).glow_options.vsync);
    }

    #[test]
    fn other_backends_preserve_default_swap_behavior() {
        assert_eq!(
            native_options(false).glow_options.vsync,
            eframe::NativeOptions::default().glow_options.vsync
        );
    }
}
