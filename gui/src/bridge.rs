// The UI runs on one thread and must never wait for the network. Requests and multi-step flows
// (rename → save pending changes → move → offer to relink) run on worker threads; they talk to
// the UI through closures executed at the start of the next frame, like the `await`s of the
// Svelte app.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use eframe::egui;

use crate::app::App;

pub type Job = Box<dyn FnOnce(&mut App) + Send>;

#[derive(Clone)]
pub struct Bridge {
    tx: Sender<Job>,
    ctx: egui::Context,
}

impl Bridge {
    pub fn new(ctx: egui::Context) -> (Bridge, Receiver<Job>) {
        let (tx, rx) = channel();
        (Bridge { tx, ctx }, rx)
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Runs `f` on the UI thread at the next frame.
    pub fn post(&self, f: impl FnOnce(&mut App) + Send + 'static) {
        if self.tx.send(Box::new(f)).is_ok() {
            self.ctx.request_repaint();
        }
    }

    /// Runs `f` on the UI thread and waits for its result (only from worker threads).
    /// None when the app is gone.
    pub fn call<T: Send + 'static>(&self, f: impl FnOnce(&mut App) -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = channel();
        self.post(move |app| {
            let _ = tx.send(f(app));
        });
        rx.recv().ok()
    }

    /// Runs `job` on a worker thread, then `then` with its result on the UI thread.
    pub fn spawn<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&mut App, T) + Send + 'static,
    ) {
        let me = self.clone();
        std::thread::spawn(move || {
            let r = job();
            me.post(move |app| then(app, r));
        });
    }

    /// Runs a multi-step flow on its own thread.
    pub fn flow(&self, f: impl FnOnce(Bridge) + Send + 'static) {
        let me = self.clone();
        std::thread::spawn(move || f(me));
    }

    /// The app's text dialog (PromptDialog): the typed text, or None when cancelled.
    pub fn ask(&self, message: impl Into<String>, value: impl Into<String>, hint: impl Into<String>) -> Option<String> {
        let (tx, rx) = channel();
        let (message, value, hint) = (message.into(), value.into(), hint.into());
        self.post(move |app| app.open_dialog(crate::dialogs::Prompt::ask(message, value, hint, tx)));
        rx.recv().ok().flatten()
    }

    /// The confirmation variant of the dialog (no text field).
    pub fn confirm(&self, message: impl Into<String>, hint: impl Into<String>, ok: impl Into<String>, danger: bool) -> bool {
        let (tx, rx) = channel();
        let (message, hint, ok) = (message.into(), hint.into(), ok.into());
        self.post(move |app| app.open_dialog(crate::dialogs::Prompt::confirm(message, hint, ok, danger, tx)));
        matches!(rx.recv(), Ok(Some(_)))
    }

    pub fn flash(&self, msg: impl Into<String>) {
        let msg = msg.into();
        self.post(move |app| app.flash(msg));
    }

    pub fn sleep(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}
