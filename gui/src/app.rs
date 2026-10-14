// The app window (App.svelte): sidebar with the file tree, top bar, the reader (PDF/EPUB)
// beside the notes (editor or preview), resizable panels, the dialogs and the keyboard
// shortcuts. Screen state is restored from the last session; switching note/document keeps a
// history for Back/Forward (Alt+←/→).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, Frame, Id, Key, Layout, Margin, Modifiers, Pos2,
    Rect, RichText, Sense, Stroke, Ui, Vec2,
};

use crate::api::{Api, ApiError, Entry, SearchResult, VaultEvent};
use crate::bridge::{Bridge, Job};
use crate::dialogs::{
    self, BacklinksPanel, ContextMenu, MenuAction, MenuItem, Outline, OutlineOutcome, PanelOutcome,
    Prompt, SearchPanel, SwitchOutcome, Switcher,
};
use crate::editor::{self, Editor, Status};
use crate::epub::{Book, EpubReader};
use crate::font_settings::{self, FontSettings};
use crate::fonts;
use crate::icons;
use crate::kit;
use crate::links::{self, file_name, is_epub, is_md, is_pdf};
use crate::markdown::{self, Block, Images, Look, Painter, Resolver};
use crate::pdf::{self, PdfReader};
use crate::setup::{self, Setup};
use crate::state::{self, Prefs, Session, Store, Theme};
use crate::theme;
use crate::tree::{TreeAction, TreeView};

#[derive(Clone, Debug, PartialEq)]
pub struct Reveal {
    pub path: String,
    pub line: usize,
    pub n: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct Nav {
    note: Option<String>,
    doc: Option<String>,
    page: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct LocalDoc {
    name: String,
    path: PathBuf,
    size: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Sidebar,
    Split,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pdf,
    Epub,
}

/// What the reader shows: a vault document or a file of this device.
#[derive(Clone, Debug, PartialEq)]
struct ReaderInfo {
    id: String,
    title: String,
    link: String,
    epub: bool,
}

pub struct Start {
    pub server: String,
    pub state_dir: PathBuf,
    /// Like #note=…&doc=…&page=… in the URL: these win over the last session.
    pub note: Option<String>,
    pub doc: Option<String>,
    pub page: Option<u32>,
}

enum Screen {
    Loading,
    Setup(Setup),
    Main,
}

pub struct App {
    pub api: Api,
    pub bridge: Bridge,
    rx: Receiver<Job>,
    store: Store,
    pub prefs: Prefs,
    screen: Screen,
    start: Option<Start>,
    restore_local: Option<String>,

    pub note: Option<String>,
    pub doc: Option<String>,
    pub page: u32,
    pdf_target: Option<(String, u32)>,
    tree: Vec<Entry>,
    files: Vec<String>,
    visible_tree: Vec<Entry>,
    visible_key: (u64, String),
    filter: String,
    tree_view: TreeView,
    reader_open: bool,
    preview: bool,
    dragging: Option<Drag>,
    font_settings: Option<FontSettings>,
    toast: Option<(String, Instant)>,
    pub editor: Option<Editor>,
    local: Option<LocalDoc>,
    pub pdf: Option<PdfReader>,
    pub epub: Option<EpubReader>,
    reader: Option<ReaderInfo>,
    epub_target: Option<(String, String, u64)>,
    switcher: Option<Switcher>,
    search: Option<SearchPanel>,
    last_search: String,
    outline: Option<Outline>,
    backlinks: Option<Vec<SearchResult>>,
    backlinks_panel: Option<BacklinksPanel>,
    backlinks_due: Option<Instant>,
    backlinks_req: u64,
    pub reveal: Option<Reveal>,
    dialog: Option<Prompt>,
    dialog_queue: VecDeque<Prompt>,
    menu: Option<ContextMenu>,
    preview_cache: Option<(u64, Vec<Block>)>,
    preview_images: Images,
    history: Vec<Nav>,
    hist_pos: usize,
    frame: u64,
    fonts_applied: fonts::Resolved,
    fonts_due: Option<Instant>,
    tree_refresh_due: Option<Instant>,
    tree_version: u64,
    events_started: bool,
    theme_applied: Option<Theme>,
    focus_fonts_button: bool,
    file_dialog_open: bool,
}

const INVALID: &str =
    "Invalid name: do not use \"..\", a leading \".\", empty path segments or # ^ [ ] |";

/// Rejects ., .., hidden names and # ^ [ ] | in the new name (Obsidian rejects them: they would
/// break links). The server rejects them too; here the message is clearer.
fn invalid_path(path: &str) -> bool {
    path.split('/').any(|p| p.is_empty() || p.starts_with('.'))
        || file_name(path).contains(['#', '^', '[', ']', '|'])
}

/// p is the item itself or (for a folder) something inside it.
fn inside(p: Option<&str>, e: &Entry) -> bool {
    p.is_some_and(|p| p == e.path || (e.dir && p.starts_with(&format!("{}/", e.path))))
}

fn remap(p: &str, from: &str, to: &str) -> String {
    format!("{to}{}", &p[from.len()..])
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// The server tree includes images (so the preview can resolve ![[photo.png]]); the sidebar
/// only shows notes and books.
fn only_docs(entries: &[Entry]) -> Vec<Entry> {
    entries
        .iter()
        .filter(|e| e.dir || is_md(&e.name) || is_pdf(&e.name) || is_epub(&e.name))
        .map(|e| {
            if e.dir {
                Entry {
                    children: Some(only_docs(e.kids())),
                    ..e.clone()
                }
            } else {
                e.clone()
            }
        })
        .collect()
}

fn filter_tree(entries: &[Entry], q: &str) -> Vec<Entry> {
    if q.is_empty() {
        return entries.to_vec();
    }
    let mut out = Vec::new();
    for e in entries {
        if e.dir {
            let children = filter_tree(e.kids(), q);
            if !children.is_empty() {
                out.push(Entry {
                    children: Some(children),
                    ..e.clone()
                });
            }
        } else if e.path.to_lowercase().contains(q) {
            out.push(e.clone());
        }
    }
    out
}

/// GNOME's text-scaling-factor, which Chromium applies on top of the monitor scale.
fn text_scaling_factor() -> Option<f32> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "text-scaling-factor"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn open_external(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// Saves anything pending in the editor (before renaming, moving, deleting or leaving the
/// note). Runs on a worker thread; true when nothing is left unsaved.
pub fn flush_editor(b: &Bridge) -> bool {
    loop {
        let Some(st) = b
            .call(|app| app.editor.as_mut().map(Editor::prepare_flush))
            .flatten()
        else {
            return true;
        };
        match st {
            Status::Saving => Bridge::sleep(30),
            Status::Dirty | Status::Error => {
                b.call(|app| {
                    let (bridge, api) = (app.bridge.clone(), app.api.clone());
                    if let Some(ed) = app.editor.as_mut() {
                        ed.save(&bridge, &api);
                    }
                });
                loop {
                    Bridge::sleep(30);
                    let st = b
                        .call(|app| app.editor.as_ref().map(|e| e.status))
                        .flatten();
                    if st != Some(Status::Saving) {
                        break;
                    }
                }
                return b
                    .call(|app| app.editor.as_ref().is_none_or(Editor::flushed))
                    .unwrap_or(true);
            }
            _ => {
                return b
                    .call(|app| app.editor.as_ref().is_none_or(Editor::flushed))
                    .unwrap_or(true);
            }
        }
    }
}

impl Resolver for App {
    fn file_url(&self, target: &str) -> String {
        if let Some(found) = links::resolve(&self.files, target) {
            return self.api.file_url(&found);
        }
        let dir = self
            .note
            .as_deref()
            .map(links::dir_of)
            .filter(|d| !d.is_empty())
            .map(|d| format!("{d}/"))
            .unwrap_or_default();
        self.api.file_url(&if target.contains('/') {
            target.to_string()
        } else {
            format!("{dir}{target}")
        })
    }
    fn exists(&self, target: &str) -> bool {
        links::resolve(&self.files, target).is_some()
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Start) -> Self {
        let ctx = cc.egui_ctx.clone();
        let (bridge, rx) = Bridge::new(ctx.clone());
        let (store, prefs) = Store::open(start.state_dir.clone());
        let api = Api::new(&start.server);
        let from_args = start.note.is_some() || start.doc.is_some();
        let session = if from_args {
            None
        } else {
            prefs.session.clone()
        };
        let note = if from_args {
            start.note.clone()
        } else {
            session.as_ref().and_then(|s| s.note.clone())
        };
        let doc = if from_args {
            start.doc.clone()
        } else {
            session.as_ref().and_then(|s| s.doc.clone())
        };
        let page = if from_args {
            start.page.unwrap_or(1)
        } else {
            session.as_ref().map_or(1, |s| s.page.max(1))
        };
        let pdf_target = match (&start.doc, start.page) {
            (Some(d), Some(p)) => Some((d.clone(), p)),
            _ => None,
        };
        // The device file is not in the arguments: it comes back from the saved session.
        let restore_local = if from_args && start.doc.is_some() {
            None
        } else {
            prefs.session.as_ref().and_then(|s| s.local.clone())
        };
        let mut app = App {
            api,
            bridge,
            rx,
            store,
            screen: Screen::Loading,
            start: Some(start),
            restore_local,
            note: note.clone(),
            doc: doc.clone(),
            page,
            pdf_target,
            tree: Vec::new(),
            files: Vec::new(),
            visible_tree: Vec::new(),
            visible_key: (u64::MAX, String::new()),
            filter: String::new(),
            tree_view: TreeView::default(),
            reader_open: true,
            preview: false,
            dragging: None,
            font_settings: None,
            toast: None,
            editor: None,
            local: None,
            pdf: None,
            epub: None,
            reader: None,
            epub_target: None,
            switcher: None,
            search: None,
            last_search: String::new(),
            outline: None,
            backlinks: None,
            backlinks_panel: None,
            backlinks_due: None,
            backlinks_req: 0,
            reveal: None,
            dialog: None,
            dialog_queue: VecDeque::new(),
            menu: None,
            preview_cache: None,
            preview_images: Images::default(),
            history: vec![Nav { note, doc, page }],
            hist_pos: 0,
            frame: 0,
            fonts_applied: fonts::Resolved::default(),
            fonts_due: None,
            tree_refresh_due: None,
            tree_version: 0,
            events_started: false,
            theme_applied: None,
            focus_fonts_button: false,
            file_dialog_open: false,
            prefs,
        };
        app.apply_fonts(&ctx);
        // Chromium scales its pages by the desktop's text scaling (an accessibility setting);
        // the same factor here keeps the window the size the web app had.
        let zoom = std::env::var("QUIRENOTE_GUI_ZOOM")
            .ok()
            .and_then(|z| z.parse::<f32>().ok())
            .or_else(text_scaling_factor)
            .unwrap_or(1.0);
        if (0.5..=3.0).contains(&zoom) && (zoom - 1.0).abs() > 0.01 {
            ctx.set_zoom_factor(zoom);
        }
        // First run (no vault configured yet): the setup screen instead of the app.
        let api = app.api.clone();
        app.bridge.spawn(
            move || api.setup(),
            |app, res| match res {
                Ok(s) if !s.configured => app.screen = Screen::Setup(Setup::new(s)),
                // A server that cannot tell (older, or down) gets the app, like main.ts does.
                _ => app.enter_main(),
            },
        );
        app
    }

    fn enter_main(&mut self) {
        self.screen = Screen::Main;
        self.refresh_tree();
        self.start_events();
        if let Some(n) = self.note.clone() {
            self.open_editor(&n);
        }
        if let Some(name) = self.restore_local.take()
            && self.doc.is_none()
        {
            let path = PathBuf::from(&name);
            if path.is_file() {
                self.show_local(path);
            } else {
                self.flash(format!(
                    "Could not reopen {}; open it again with the open file button.",
                    file_name(&name)
                ));
            }
        }
        self.load_backlinks(0);
    }

    // ------------------------------------------------------------ small helpers

    pub fn flash(&mut self, msg: impl Into<String>) {
        // A new toast does not disappear early because of the previous one.
        self.toast = Some((msg.into(), Instant::now() + Duration::from_secs(3)));
    }

    pub fn open_dialog(&mut self, p: Prompt) {
        if self.dialog.is_none() {
            self.dialog = Some(p);
        } else {
            self.dialog_queue.push_back(p);
        }
    }

    fn focus_editor(&mut self) {
        if let Some(ed) = self.editor.as_mut() {
            ed.focus();
        }
    }

    fn set_tree(&mut self, tree: Vec<Entry>) {
        self.files = links::all_files(&tree);
        self.tree = tree;
        self.tree_version += 1;
    }

    fn refresh_tree(&mut self) {
        let api = self.api.clone();
        self.bridge.spawn(
            move || api.tree(),
            |app, res| match res {
                Ok(t) => app.set_tree(t),
                Err(e) => app.flash(e.to_string()),
            },
        );
    }

    fn start_events(&mut self) {
        if self.events_started {
            return;
        }
        self.events_started = true;
        let api = self.api.clone();
        let b = self.bridge.clone();
        std::thread::Builder::new()
            .name("events".into())
            .spawn(move || {
                // EventSource reconnects by itself; so does this.
                loop {
                    let b2 = b.clone();
                    let _ = api.events(move |ev| b2.post(move |app| app.on_event(ev)));
                    std::thread::sleep(Duration::from_secs(3));
                }
            })
            .ok();
    }

    fn on_event(&mut self, ev: VaultEvent) {
        if self.note.as_deref() == Some(ev.path.as_str()) {
            let (b, a) = (self.bridge.clone(), self.api.clone());
            if let Some(ed) = self.editor.as_mut() {
                ed.external_change(&b, &a);
            }
        }
        // Another note changed: a link to the open note may have appeared or disappeared.
        if is_md(&ev.path) && self.note.as_deref() != Some(ev.path.as_str()) {
            self.load_backlinks(800);
        }
        if ev.op != "write" {
            self.tree_refresh_due = Some(Instant::now() + Duration::from_millis(300));
        }
    }

    fn load_backlinks(&mut self, delay_ms: u64) {
        self.backlinks_due = Some(Instant::now() + Duration::from_millis(delay_ms));
    }

    fn run_backlinks(&mut self) {
        self.backlinks_req += 1;
        let req = self.backlinks_req;
        let Some(note) = self.note.clone() else {
            self.backlinks = None;
            return;
        };
        let api = self.api.clone();
        let n2 = note.clone();
        self.bridge.spawn(
            move || api.backlinks(&n2),
            move |app, res| {
                // Transient failure (e.g. slow rg): keep this note's previous count.
                if let Ok(r) = res
                    && app.backlinks_req == req
                    && app.note.as_deref() == Some(note.as_str())
                {
                    app.backlinks = Some(r);
                }
            },
        );
    }

    fn apply_fonts(&mut self, ctx: &egui::Context) {
        let resolved = fonts::resolve(&self.prefs.fonts);
        if resolved != self.fonts_applied {
            ctx.set_fonts(fonts::definitions(&resolved));
            self.fonts_applied = resolved;
            if let Some(ed) = self.editor.as_mut() {
                ed.fonts_changed();
            }
            self.preview_cache = None;
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.theme_applied == Some(self.prefs.theme) {
            return;
        }
        self.theme_applied = Some(self.prefs.theme);
        ctx.set_visuals_of(egui::Theme::Light, theme::visuals(Theme::Light));
        ctx.set_visuals_of(egui::Theme::Dark, theme::visuals(Theme::Dark));
        ctx.set_theme(if self.prefs.theme == Theme::Dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        ctx.all_styles_mut(|s| {
            s.spacing.button_padding = Vec2::new(8.0, 3.0);
            s.spacing.item_spacing = Vec2::new(6.0, 6.0);
            s.spacing.interact_size.y = 24.0;
            s.text_styles.insert(egui::TextStyle::Body, fonts::ui(13.0));
            s.text_styles
                .insert(egui::TextStyle::Button, fonts::ui(13.0));
            s.text_styles
                .insert(egui::TextStyle::Small, fonts::ui(11.0));
            s.text_styles
                .insert(egui::TextStyle::Monospace, fonts::mono(13.0));
            s.spacing.scroll = egui::style::ScrollStyle::thin();
        });
        // A wheel notch scrolls about as far as in Chromium.
        ctx.options_mut(|o| o.input_options.line_scroll_speed = 120.0);
    }

    // ------------------------------------------------------------ reader

    fn reader_info(&self) -> Option<ReaderInfo> {
        if let Some(d) = &self.doc {
            return Some(ReaderInfo {
                id: d.clone(),
                title: d.clone(),
                link: d.clone(),
                epub: is_epub(d),
            });
        }
        self.local.as_ref().map(|l| ReaderInfo {
            id: format!("local:{}:{}", l.name, l.size),
            title: l.name.clone(),
            link: l.name.clone(),
            epub: is_epub(&l.name),
        })
    }

    /// Creates the reader when the document changes and fetches its bytes.
    fn sync_reader(&mut self) {
        let info = self.reader_info();
        if info == self.reader {
            return;
        }
        self.reader = info.clone();
        self.pdf = None;
        self.epub = None;
        let Some(info) = info else { return };
        let source: Box<dyn FnOnce() -> Result<Vec<u8>, String> + Send> =
            match (&self.doc, &self.local) {
                (Some(d), _) => {
                    let api = self.api.clone();
                    let d = d.clone();
                    Box::new(move || api.file(&d).map_err(|e| e.to_string()))
                }
                (None, Some(l)) => {
                    let path = l.path.clone();
                    Box::new(move || std::fs::read(&path).map_err(|e| e.to_string()))
                }
                _ => return,
            };
        let id = info.id.clone();
        if info.epub {
            let saved = self.prefs.epub.get(&info.id).cloned();
            let mut r = EpubReader::new(info.id.clone(), info.title.clone(), saved);
            if let Some((tid, cfi, n)) = &self.epub_target
                && *tid == info.id
            {
                r.target(cfi, *n);
            }
            self.epub = Some(r);
            self.bridge.spawn(
                move || source().and_then(Book::open),
                move |app, res| {
                    let Some(r) = app.epub.as_mut().filter(|r| r.id == id) else {
                        return;
                    };
                    match res {
                        Ok(book) => r.opened(book),
                        Err(e) => {
                            r.error = if e.is_empty() {
                                "could not open the EPUB".into()
                            } else {
                                e
                            }
                        }
                    }
                },
            );
        } else {
            // The link page; else the page in the session (same document); else the last one read.
            let start = match &self.pdf_target {
                Some((tid, p)) if *tid == info.id => *p,
                _ if self.page > 1 && self.doc.is_some() => self.page,
                _ => self.prefs.pages.get(&info.id).copied().unwrap_or(1),
            };
            let mut r = PdfReader::new(&self.bridge, info.id.clone(), info.title.clone(), start);
            self.page = start;
            r.page = start;
            self.pdf = Some(r);
            self.bridge.spawn(source, move |app, res| {
                let Some(r) = app.pdf.as_mut().filter(|r| r.id == id) else {
                    return;
                };
                match res {
                    Ok(data) => r.open(data),
                    Err(e) => r.fail(e),
                }
            });
        }
    }

    pub fn pdf_resp(&mut self, r: pdf::Resp) {
        if let Some(p) = self.pdf.as_mut() {
            p.resp(r);
        }
    }

    fn active_kind(&self) -> Option<Kind> {
        self.reader
            .as_ref()
            .map(|r| if r.epub { Kind::Epub } else { Kind::Pdf })
    }

    fn reader_scroll(&mut self, dir: i32, page: bool) {
        match self.active_kind() {
            Some(Kind::Pdf) => {
                if let Some(p) = self.pdf.as_mut() {
                    if page {
                        p.turn_page(dir)
                    } else {
                        p.scroll(dir)
                    }
                }
            }
            Some(Kind::Epub) => {
                if let Some(e) = self.epub.as_mut() {
                    if page {
                        e.turn_page(dir)
                    } else {
                        e.scroll(dir)
                    }
                }
            }
            None => {}
        }
    }

    /// Asks the reader of book id to go to cfi.
    fn go_epub(&mut self, id: String, cfi: String) {
        let n = self.epub_target.as_ref().map_or(0, |t| t.2) + 1;
        if let Some(e) = self.epub.as_mut().filter(|e| e.id == id) {
            e.target(&cfi, n);
        }
        self.epub_target = Some((id, cfi, n));
    }

    /// Switched documents: the previous book's position/request no longer apply.
    fn forget_epub(&mut self) {
        self.epub_target = None;
    }

    fn close_local(&mut self) {
        self.local = None;
    }

    fn close_reader(&mut self) {
        self.close_local();
        self.forget_epub();
        self.doc = None;
    }

    fn show_local(&mut self, path: PathBuf) {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.local = Some(LocalDoc { name, path, size });
        self.doc = None;
        self.page = 1;
        self.forget_epub();
        self.reader_open = true;
    }

    /// Opens a PDF/EPUB from this device (the portal's file chooser); remembered by path.
    fn pick_local(&mut self) {
        if self.file_dialog_open {
            return;
        }
        self.file_dialog_open = true;
        self.bridge.spawn(
            || {
                rfd::FileDialog::new()
                    .add_filter("PDF or EPUB", &["pdf", "epub", "PDF", "EPUB"])
                    .set_title("Open a PDF or EPUB")
                    .pick_file()
            },
            |app, picked| {
                app.file_dialog_open = false;
                if let Some(p) = picked {
                    app.show_local(p);
                }
            },
        );
    }

    // ------------------------------------------------------------ opening notes and documents

    fn open_editor(&mut self, path: &str) {
        let (b, a) = (self.bridge.clone(), self.api.clone());
        match self.editor.as_mut() {
            Some(ed) if ed.path == path => {}
            _ => {
                let mut ed = Editor::new(path.to_string());
                ed.load(&b, &a);
                self.editor = Some(ed);
                self.preview_cache = None;
                self.preview_images.clear();
                // The panel belonged to the previous note; it does not reopen by itself.
                self.backlinks = None;
                self.backlinks_panel = None;
                self.load_backlinks(0);
            }
        }
    }

    fn set_note(&mut self, note: Option<String>) {
        if note == self.note {
            return;
        }
        self.note = note.clone();
        match note {
            Some(n) => self.open_editor(&n),
            None => {
                self.drop_editor();
                self.backlinks = None;
                self.backlinks_panel = None;
                self.outline = None;
            }
        }
    }

    /// The editor goes away (note closed or trashed): pending changes are saved first.
    fn drop_editor(&mut self) {
        if let Some(mut ed) = self.editor.take()
            && ed.status == Status::Dirty
        {
            ed.save(&self.bridge, &self.api);
        }
    }

    fn editor_needs_flush(&self, next: &str) -> bool {
        self.note.as_deref() != Some(next) && self.editor.as_ref().is_some_and(|e| !e.flushed())
    }

    /// openPath: notes in the editor, PDFs/EPUBs in the reader. Leaving a note with pending
    /// changes saves them first; a conflict or error keeps the user on the note.
    pub fn open_path(&mut self, path: String, page: Option<u32>, cfi: Option<String>) {
        if is_md(&path) && self.editor_needs_flush(&path) {
            self.bridge.flow(move |b| {
                if leave_note(&b) {
                    b.post(move |app| app.open_path_now(path, page, cfi));
                }
            });
            return;
        }
        self.open_path_now(path, page, cfi);
    }

    fn open_path_now(&mut self, path: String, page: Option<u32>, cfi: Option<String>) {
        // Opening by any other path cancels the positioning requested by the search.
        self.reveal = None;
        if is_md(&path) || is_pdf(&path) || is_epub(&path) {
            self.prefs.remember(&path);
        }
        if is_md(&path) {
            self.set_note(Some(path));
            self.prefs.notes = true;
        } else if is_pdf(&path) {
            // The link page; without it, the reader goes back to the last page read.
            self.pdf_target = page.map(|p| (path.clone(), p));
            let same = self.doc.as_deref() == Some(path.as_str());
            self.page = page.unwrap_or(if same {
                self.page
            } else {
                self.prefs.pages.get(&path).copied().unwrap_or(1)
            });
            if !same {
                self.forget_epub();
            }
            self.close_local();
            self.doc = Some(path);
            self.reader_open = true;
            if same && let (Some(p), Some(pg)) = (self.pdf.as_mut(), page) {
                p.go_to_page(pg);
            }
        } else if is_epub(&path) {
            self.close_local();
            if self.doc.as_deref() != Some(path.as_str()) {
                self.forget_epub();
            }
            self.doc = Some(path.clone());
            self.reader_open = true;
            if let Some(c) = cfi {
                self.sync_reader();
                self.go_epub(path, c);
            }
        } else {
            self.flash("Format not supported by the reader");
        }
    }

    fn open_at_line(&mut self, path: String, line: usize) {
        self.preview = false;
        let n = self.reveal.as_ref().map_or(0, |r| r.n) + 1;
        if self.note.as_deref() == Some(path.as_str()) || !self.editor_needs_flush(&path) {
            self.open_path_now(path.clone(), None, None);
            self.reveal = Some(Reveal {
                path: path.clone(),
                line,
                n,
            });
            if let Some(ed) = self.editor.as_mut().filter(|e| e.path == path) {
                ed.reveal(line, n);
            }
            return;
        }
        self.bridge.flow(move |b| {
            if leave_note(&b) {
                b.post(move |app| app.open_at_line(path, line));
            }
        });
    }

    pub fn open_link(&mut self, target: String, page: Option<u32>, cfi: Option<String>) {
        if let Some(path) = links::resolve(&self.files, &target) {
            return self.open_path(path, page, cfi);
        }
        // Link to the open local PDF/EPUB (stored with the file name only).
        let name = file_name(&target).to_string();
        if let Some(l) = &self.local
            && l.name == name
        {
            if let Some(p) = page
                && let Some(r) = self.pdf.as_mut()
            {
                r.go_to_page(p);
            }
            if let (Some(c), Some(info)) = (cfi, self.reader.clone()) {
                self.go_epub(info.id, c);
            }
            return;
        }
        // Link to a note that does not exist yet: offer to create it (like Obsidian), in the
        // current note's folder when the link has no path. PDFs, EPUBs and images do not
        // become notes; names with dots (e.g. [[2024.10.12]]) do.
        if !is_pdf(&name) && !is_epub(&name) && !links::IMAGE.is_match(&name) {
            let dir = if !target.contains('/') {
                self.note
                    .as_deref()
                    .map(links::dir_of)
                    .filter(|d| !d.is_empty())
                    .map(|d| format!("{d}/"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let suggestion = format!("{}.md", links::strip_md(&format!("{dir}{target}")));
            let shown = links::strip_md(&name).to_string();
            self.bridge.flow(move |b| {
                let chosen = b.ask(
                    format!("Create the note “{shown}”?"),
                    suggestion,
                    "The link does not point to any note yet. Adjust the path if you like.",
                );
                if let Some(c) = chosen.filter(|c| !c.trim().is_empty()) {
                    create_and_open(&b, c);
                }
            });
            return;
        }
        let local_hint = if is_pdf(&target) || is_epub(&target) {
            " (if it is a local file, open it with the open file button)"
        } else {
            ""
        };
        self.flash(format!("Not found: {target}{local_hint}"));
    }

    fn insert_page_link(&mut self) {
        let Some(info) = self.reader.clone() else {
            return;
        };
        let ctx = self.bridge.ctx().clone();
        let text = if info.epub {
            // Only this book's position.
            match self.epub.as_ref().and_then(|e| e.location.clone()) {
                Some(loc) if loc.id == info.id && !loc.cfi.is_empty() => {
                    links::cfi_link(&info.link, &loc.cfi)
                }
                _ => return,
            }
        } else {
            links::page_link(&info.link, self.page)
        };
        if let Some(ed) = self.editor.as_mut() {
            ed.insert(&ctx, &format!("{text} "));
        }
    }

    fn toggle_preview(&mut self) {
        if self.note.is_none() {
            return;
        }
        self.preview = !self.preview;
        if !self.preview {
            self.focus_editor();
        }
    }

    /// Never leaves both panels collapsed at the same time.
    fn toggle_notes(&mut self) {
        self.prefs.notes = !self.prefs.notes;
        if !self.prefs.notes && self.reader.is_some() {
            self.reader_open = true;
        }
    }

    fn toggle_reader(&mut self) {
        self.reader_open = !self.reader_open;
        if !self.reader_open {
            self.prefs.notes = true;
        }
    }

    // ------------------------------------------------------------ history (Back/Forward)

    fn record_history(&mut self) {
        let cur = Nav {
            note: self.note.clone(),
            doc: self.doc.clone(),
            page: self.page,
        };
        let Some(last) = self.history.get(self.hist_pos).cloned() else {
            self.history.push(cur);
            self.hist_pos = self.history.len() - 1;
            return;
        };
        if last.note == cur.note && last.doc == cur.doc {
            // Changing only the PDF page updates the current entry.
            self.history[self.hist_pos].page = cur.page;
        } else {
            self.history.truncate(self.hist_pos + 1);
            self.history.push(cur);
            self.hist_pos = self.history.len() - 1;
        }
    }

    fn go_history(&mut self, delta: i32) {
        let target = self.hist_pos as i64 + delta as i64;
        if target < 0 || target as usize >= self.history.len() {
            return;
        }
        let nav = self.history[target as usize].clone();
        let apply = move |app: &mut App| {
            app.hist_pos = target as usize;
            app.reveal = None;
            app.set_note(nav.note.clone());
            if nav.doc != app.doc {
                app.forget_epub();
            }
            if let Some(d) = &nav.doc {
                app.close_local();
                app.page = nav.page;
                app.pdf_target = Some((d.clone(), nav.page));
                app.reader_open = true;
                if app.doc.as_deref() == Some(d.as_str())
                    && let Some(p) = app.pdf.as_mut()
                {
                    p.go_to_page(nav.page);
                }
            }
            app.doc = nav.doc.clone();
        };
        let next_note = self.history[target as usize].note.clone();
        if next_note
            .as_deref()
            .is_some_and(|n| self.editor_needs_flush(n))
            || (next_note.is_none() && self.editor.as_ref().is_some_and(|e| !e.flushed()))
        {
            self.bridge.flow(move |b| {
                if leave_note(&b) {
                    b.post(apply);
                }
            });
        } else {
            apply(self);
        }
    }

    // ------------------------------------------------------------ the frame

    fn drain(&mut self) {
        while let Ok(job) = self.rx.try_recv() {
            job(self);
        }
    }

    fn timers(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut wake = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        if let Some(t) = self.backlinks_due {
            if now >= t {
                self.backlinks_due = None;
                self.run_backlinks();
            } else {
                wake(t);
            }
        }
        if let Some(t) = self.tree_refresh_due {
            if now >= t {
                self.tree_refresh_due = None;
                self.refresh_tree();
            } else {
                wake(t);
            }
        }
        if let Some((_, t)) = &self.toast {
            if now >= *t {
                self.toast = None;
            } else {
                wake(*t);
            }
        }
        if let Some(t) = self.fonts_due {
            if now >= t {
                self.fonts_due = None;
                self.apply_fonts(ctx);
            } else {
                wake(t);
            }
        }
        if let Some(t) = next {
            ctx.request_repaint_after(t.saturating_duration_since(now));
        }
    }

    /// Shortcuts in the capture phase: they arrive before the editor, whose vim mode uses
    /// Ctrl+E (scroll) and Ctrl+O (jump list back).
    fn capture_keys(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.font_settings.is_some() {
            return;
        }
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        if self.note.is_some() && dialogs::take(ctx, cs, Key::O) {
            self.switcher = None;
            self.search = None;
            self.outline = if self.outline.is_some() {
                None
            } else {
                Some(Outline::new())
            };
            if self.outline.is_none() {
                self.focus_editor();
            }
            return;
        }
        if dialogs::take(ctx, cs, Key::F) {
            self.switcher = None;
            // Closing through the panel keeps the term and returns focus to the editor.
            if let Some(s) = self.search.take() {
                self.last_search = s.query;
                self.focus_editor();
            } else {
                self.search = Some(SearchPanel::new(self.last_search.clone()));
            }
            return;
        }
        // With the switcher open, Ctrl+P moves up the list (handled there) and only Ctrl+O closes it.
        let o = dialogs::take(ctx, Modifiers::COMMAND, Key::O);
        let p = self.switcher.is_none() && dialogs::take(ctx, Modifiers::COMMAND, Key::P);
        if o || p {
            self.switcher = if self.switcher.is_some() {
                None
            } else {
                Some(Switcher::new())
            };
            if self.switcher.is_none() {
                self.focus_editor();
            }
            return;
        }
        if self.note.is_some() && dialogs::take(ctx, Modifiers::COMMAND, Key::E) {
            self.toggle_preview();
        }
    }

    fn bubble_keys(&mut self, ctx: &egui::Context) {
        // A Tab can take the focus out of a dialog: with one open the app must not act behind it.
        if self.dialog.is_some()
            || self.font_settings.is_some()
            || self.search.is_some()
            || self.outline.is_some()
            || self.backlinks_panel.is_some()
        {
            return;
        }
        if self.reader.is_some() && self.reader_open {
            let alt = Modifiers::ALT;
            let alt_shift = Modifiers::ALT | Modifiers::SHIFT;
            if dialogs::take(ctx, alt_shift, Key::ArrowDown) {
                self.reader_scroll(1, true);
            }
            if dialogs::take(ctx, alt_shift, Key::ArrowUp) {
                self.reader_scroll(-1, true);
            }
            if dialogs::take(ctx, alt, Key::ArrowDown) {
                self.reader_scroll(1, false);
            }
            if dialogs::take(ctx, alt, Key::ArrowUp) {
                self.reader_scroll(-1, false);
            }
        }
        if self.reader.is_some() && dialogs::take(ctx, Modifiers::ALT, Key::Backslash) {
            self.toggle_reader();
        }
        if dialogs::take(ctx, Modifiers::COMMAND, Key::Backslash) {
            self.toggle_notes();
        }
        if dialogs::take(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::Backslash)
            || dialogs::take(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::Pipe)
            || dialogs::take(ctx, Modifiers::COMMAND, Key::Pipe)
        {
            self.prefs.sidebar = !self.prefs.sidebar;
        }
        if dialogs::take(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::L) {
            self.insert_page_link();
        }
        // Browser Back/Forward: Alt+←/→ and the mouse's side buttons.
        let back = dialogs::take(ctx, Modifiers::ALT, Key::ArrowLeft)
            || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1));
        let fwd = dialogs::take(ctx, Modifiers::ALT, Key::ArrowRight)
            || ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2));
        if back {
            self.go_history(-1);
        }
        if fwd {
            self.go_history(1);
        }
    }

    fn save_session(&mut self) {
        let local = self
            .local
            .as_ref()
            .map(|l| l.path.to_string_lossy().into_owned());
        let s = Session {
            note: self.note.clone(),
            doc: self.doc.clone(),
            page: self.page,
            local,
        };
        if self.prefs.session.as_ref() != Some(&s) {
            self.prefs.session = Some(s);
        }
        self.store.save(&self.prefs);
    }

    // ------------------------------------------------------------ drawing

    fn ui_main(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let p = theme::palette(self.prefs.theme);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, p.bg);
        // Narrow window: reader above the notes (the CSS media query at 800px).
        let narrow = full.width() <= 800.0;

        let mut sidebar_rect = Rect::NOTHING;
        if self.prefs.sidebar {
            let w = self.prefs.sidebar_w;
            sidebar_rect = Rect::from_min_size(full.min, Vec2::new(w, full.height()));
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(sidebar_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            child.set_clip_rect(sidebar_rect);
            self.sidebar(&mut child, p);
            let r = Rect::from_min_size(
                Pos2::new(sidebar_rect.right(), full.top()),
                Vec2::new(5.0, full.height()),
            );
            self.resizer(ui, r, Drag::Sidebar, full, p);
        }
        let left = if self.prefs.sidebar {
            sidebar_rect.right() + 5.0
        } else {
            full.left()
        };
        let ws = Rect::from_min_max(Pos2::new(left, full.top()), full.max);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(ws)
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(ws);
        self.topbar(&mut child, p);
        let y = child.cursor().top();
        child
            .painter()
            .hline(ws.x_range(), y, Stroke::new(1.0, p.border));
        let panes = Rect::from_min_max(Pos2::new(ws.left(), y + 1.0), ws.max);
        self.panes(ui, panes, p, narrow);
        self.toast_ui(&ctx, p);
    }

    fn sidebar(&mut self, ui: &mut Ui, p: &theme::Palette) {
        ui.painter().rect_filled(ui.max_rect(), 0.0, p.panel);
        Frame::new().inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let w = ui.available_width() - 3.0 * 36.0;
                ui.allocate_ui(Vec2::new(w, 26.0), |ui| {
                    kit::field(ui, &mut self.filter, "Filter…", fonts::ui(13.0), p.hover)
                        .on_hover_text("Filter the tree by name · Ctrl+O quick switcher · Ctrl+Shift+F full-text search");
                });
                if kit::button(ui, Some(icons::FILE_PLUS), "", "New note").clicked() {
                    self.new_note(None);
                }
                if kit::button(ui, Some(icons::FOLDER_PLUS), "", "New folder").clicked() {
                    self.new_folder(None);
                }
                if kit::button(ui, Some(icons::FOLDER_OPEN), "", "Open a PDF or EPUB from this device").clicked() {
                    self.pick_local();
                }
            });
        });
        let q = self.filter.trim().to_lowercase();
        if self.visible_key != (self.tree_version, q.clone()) {
            self.visible_tree = filter_tree(&only_docs(&self.tree), &q);
            self.visible_key = (self.tree_version, q.clone());
        }
        let active: Vec<String> = [self.note.clone(), self.doc.clone()]
            .into_iter()
            .flatten()
            .collect();
        let active_refs: Vec<&str> = active.iter().map(String::as_str).collect();
        self.tree_view.reveal(&self.tree, &active_refs);
        let tree = std::mem::take(&mut self.visible_tree);
        let action = egui::ScrollArea::vertical()
            .id_salt("tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin {
                        left: 4,
                        right: 4,
                        top: 0,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        self.tree_view.show(ui, &tree, &active_refs, !q.is_empty())
                    })
                    .inner
            })
            .inner;
        self.visible_tree = tree;
        match action {
            Some(TreeAction::Open(path)) => self.open_path(path, None, None),
            Some(TreeAction::NewNote(dir)) => self.new_note(Some(dir)),
            Some(TreeAction::NewFolder(dir)) => self.new_folder(Some(dir)),
            Some(TreeAction::Context(e, pos)) => self.open_menu(e, pos),
            None => {}
        }
    }

    fn open_menu(&mut self, e: Entry, pos: Pos2) {
        let mut items = if e.dir {
            vec![
                MenuItem {
                    label: "New note here".into(),
                    danger: false,
                    action: MenuAction::NewNoteIn(e.path.clone()),
                },
                MenuItem {
                    label: "New subfolder".into(),
                    danger: false,
                    action: MenuAction::NewFolderIn(e.path.clone()),
                },
            ]
        } else {
            vec![MenuItem {
                label: "Open".into(),
                danger: false,
                action: MenuAction::Open(e.path.clone()),
            }]
        };
        items.push(MenuItem {
            label: "Rename / move…".into(),
            danger: false,
            action: MenuAction::Rename(e.clone()),
        });
        items.push(MenuItem {
            label: "Move to trash".into(),
            danger: true,
            action: MenuAction::Trash(e),
        });
        self.menu = Some(ContextMenu::new(pos, items, self.frame));
    }

    fn resizer(&mut self, ui: &mut Ui, r: Rect, kind: Drag, panes: Rect, p: &theme::Palette) {
        let id = Id::new(("resizer", kind as u8));
        let resp = ui
            .interact(r, id, Sense::click_and_drag())
            .on_hover_text("Drag to resize · double-click to reset");
        let active = resp.hovered() || self.dragging == Some(kind);
        ui.painter()
            .rect_filled(r, 0.0, if active { p.accent } else { p.border });
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn);
        }
        if resp.drag_started() {
            self.dragging = Some(kind);
        }
        if resp.dragged()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            match kind {
                Drag::Sidebar => self.prefs.sidebar_w = pos.x.clamp(160.0, 520.0),
                Drag::Split => {
                    self.prefs.notes_pct =
                        ((panes.right() - pos.x) / panes.width() * 100.0).clamp(20.0, 80.0)
                }
            }
        }
        if resp.drag_stopped() {
            self.dragging = None;
            self.prefs.sidebar_w = self.prefs.sidebar_w.round();
            self.prefs.notes_pct = self.prefs.notes_pct.round();
        }
        if resp.double_clicked() {
            match kind {
                Drag::Sidebar => self.prefs.sidebar_w = state::SIDEBAR_W,
                Drag::Split => self.prefs.notes_pct = state::NOTES_PCT,
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn topbar(&mut self, ui: &mut Ui, p: &theme::Palette) {
        Frame::new()
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if kit::button(ui, Some(icons::PANEL_LEFT), "", "Files (Ctrl+Shift+\\)")
                        .clicked()
                    {
                        self.prefs.sidebar = !self.prefs.sidebar;
                    }
                    if kit::button(
                        ui,
                        Some(icons::SEARCH),
                        "",
                        "Search note contents (Ctrl+Shift+F)",
                    )
                    .clicked()
                    {
                        self.search = Some(SearchPanel::new(self.last_search.clone()));
                    }
                    // Right side first, so the crumb takes what is left.
                    let crumb = self.note.clone().unwrap_or_default();
                    let crumb_id = ui.next_auto_id();
                    let crumb_pos = ui.cursor().min;
                    let right = |ui: &mut Ui, me: &mut Self| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (icon, label) = if me.prefs.notes {
                                (icons::PANEL_RIGHT_CLOSE, "Collapse notes")
                            } else {
                                (icons::PANEL_RIGHT_OPEN, "Notes")
                            };
                            if kit::button_ex(
                                ui,
                                Some(icon),
                                label,
                                "Notes (Ctrl+\\)",
                                false,
                                me.prefs.notes,
                            )
                            .clicked()
                            {
                                me.toggle_notes();
                            }
                            let fonts_btn = kit::button(
                                ui,
                                Some(icons::TYPE),
                                "",
                                "Fonts: interface, notes and code",
                            );
                            if me.focus_fonts_button {
                                fonts_btn.request_focus();
                                me.focus_fonts_button = false;
                            }
                            if fonts_btn.clicked() {
                                me.open_fonts();
                            }
                            let (ti, tt) = if me.prefs.theme == Theme::Light {
                                (icons::MOON, "Dark theme")
                            } else {
                                (icons::SUN, "Light theme (paper)")
                            };
                            if kit::button(ui, Some(ti), "", tt).clicked() {
                                me.prefs.theme = if me.prefs.theme == Theme::Light {
                                    Theme::Dark
                                } else {
                                    Theme::Light
                                };
                            }
                            if me.reader.is_some() {
                                if kit::button(ui, Some(icons::X), "", "Close reader").clicked() {
                                    me.close_reader();
                                }
                                let (ic, lb, after) = if me.reader_open {
                                    (icons::PANEL_LEFT_CLOSE, "Collapse reader", false)
                                } else {
                                    (icons::PANEL_LEFT_OPEN, "Reader", true)
                                };
                                if kit::button_ex(
                                    ui,
                                    Some(ic),
                                    lb,
                                    "Collapse/show the reader without closing it (Alt+\\)",
                                    false,
                                    after,
                                )
                                .clicked()
                                {
                                    me.toggle_reader();
                                }
                            }
                            if !me.prefs.sidebar
                                && kit::button(
                                    ui,
                                    Some(icons::FOLDER_OPEN),
                                    "",
                                    "Open a PDF or EPUB from this device",
                                )
                                .clicked()
                            {
                                me.pick_local();
                            }
                            if let (Some(info), true, true) =
                                (me.reader.clone(), me.note.is_some(), me.prefs.notes)
                            {
                                let label = if info.epub {
                                    match me.epub.as_ref().and_then(|e| e.location.as_ref()) {
                                        Some(l) if l.id == info.id => {
                                            format!("{}%", (l.fraction * 100.0).round() as i32)
                                        }
                                        _ => "…".into(),
                                    }
                                } else {
                                    format!("p.{}", me.page)
                                };
                                if kit::button(
                                    ui,
                                    Some(icons::LINK),
                                    &label,
                                    "Insert a link to the current position (Ctrl+Shift+L)",
                                )
                                .clicked()
                                {
                                    me.insert_page_link();
                                }
                            }
                            if me.note.is_some() {
                                let (ic, lb) = if me.preview {
                                    (icons::PENCIL, "Edit")
                                } else {
                                    (icons::EYE, "Preview")
                                };
                                if kit::button_ex(
                                    ui,
                                    Some(ic),
                                    lb,
                                    "Toggle preview / editing (Ctrl+E)",
                                    me.preview,
                                    false,
                                )
                                .clicked()
                                {
                                    me.toggle_preview();
                                }
                            }
                            if let (Some(_), Some(b)) = (&me.note, &me.backlinks) {
                                let unique: std::collections::HashSet<&str> =
                                    b.iter().map(|r| r.path.as_str()).collect();
                                let r = ui
                                    .add_enabled_ui(!b.is_empty(), |ui| {
                                        kit::button(
                                            ui,
                                            Some(icons::CORNER_UP_LEFT),
                                            &unique.len().to_string(),
                                            "Notes linking here (backlinks)",
                                        )
                                    })
                                    .inner;
                                if r.clicked() {
                                    me.backlinks_panel = Some(BacklinksPanel::new());
                                }
                            }
                            // The crumb: what is left between the left buttons and these.
                            let w = (ui.available_width() - 4.0).max(0.0);
                            let (r, _) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::hover());
                            crate::tree::paint_truncated(ui, r, &crumb, fonts::ui(13.0), p.muted);
                        });
                    };
                    let _ = (crumb_id, crumb_pos);
                    right(ui, self);
                });
            });
    }

    fn panes(&mut self, ui: &mut Ui, area: Rect, p: &theme::Palette, narrow: bool) {
        self.sync_reader();
        let has_reader = self.reader.is_some();
        let show_reader = has_reader && self.reader_open;
        let notes = self.prefs.notes;
        let split = show_reader && notes;
        let (reader_rect, notes_rect) = if split {
            if narrow {
                let h = area.height() / 2.0;
                (
                    Rect::from_min_size(area.min, Vec2::new(area.width(), h)),
                    Rect::from_min_max(Pos2::new(area.left(), area.top() + h), area.max),
                )
            } else {
                let nw = (area.width() - 5.0) * self.prefs.notes_pct / 100.0;
                let rw = area.width() - 5.0 - nw;
                let rr = Rect::from_min_size(area.min, Vec2::new(rw, area.height()));
                let sr = Rect::from_min_size(
                    Pos2::new(rr.right(), area.top()),
                    Vec2::new(5.0, area.height()),
                );
                self.resizer(ui, sr, Drag::Split, area, p);
                (
                    rr,
                    Rect::from_min_max(Pos2::new(sr.right(), area.top()), area.max),
                )
            }
        } else if show_reader {
            (area, Rect::NOTHING)
        } else {
            (Rect::NOTHING, area)
        };
        if show_reader && reader_rect.width() > 0.0 {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(reader_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            child.set_clip_rect(reader_rect);
            self.reader_ui(&mut child, p);
        }
        if notes && notes_rect.width() > 0.0 {
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(notes_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            child.set_clip_rect(notes_rect);
            self.notes_ui(&mut child, p);
        }
    }

    fn reader_ui(&mut self, ui: &mut Ui, p: &theme::Palette) {
        match self.active_kind() {
            Some(Kind::Pdf) => {
                if let Some(r) = self.pdf.as_mut()
                    && let Some(page) = r.ui(ui, p)
                {
                    self.page = page;
                    self.prefs.pages.insert(r.id.clone(), page);
                }
            }
            Some(Kind::Epub) => {
                let theme = self.prefs.theme;
                if let Some(r) = self.epub.as_mut() {
                    if let Some(url) = r.ui(ui, theme, p) {
                        // External links in the book: only http(s)/mailto.
                        if url.starts_with("http:")
                            || url.starts_with("https:")
                            || url.starts_with("mailto:")
                        {
                            open_external(&url);
                        }
                    }
                    if let Some(loc) = &r.location
                        && self.prefs.epub.get(&loc.id) != Some(&loc.cfi)
                    {
                        self.prefs.epub.insert(loc.id.clone(), loc.cfi.clone());
                    }
                    let wanted = r.wanted_images();
                    for (key, bytes) in wanted {
                        let id = r.id.clone();
                        self.bridge.spawn(
                            move || markdown::decode(&bytes),
                            move |app, img| {
                                let ctx = app.bridge.ctx().clone();
                                if let Some(r) = app.epub.as_mut().filter(|r| r.id == id) {
                                    r.image_loaded(&ctx, key, img);
                                }
                            },
                        );
                    }
                }
            }
            None => {}
        }
    }

    fn notes_ui(&mut self, ui: &mut Ui, p: &theme::Palette) {
        if self.note.is_none() {
            self.empty_state(ui, p);
            return;
        }
        let (b, a) = (self.bridge.clone(), self.api.clone());
        if self.preview {
            self.preview_ui(ui, p);
            return;
        }
        let theme = self.prefs.theme;
        let files = std::mem::take(&mut self.files);
        let out = match self.editor.as_mut() {
            Some(ed) => ed.ui(ui, theme, &files, &b, &a),
            None => editor::Out::default(),
        };
        self.files = files;
        if let Some(l) = out.link {
            self.open_link(l.target, l.page, l.cfi);
        }
        if !out.images.is_empty() {
            self.paste_images(out.images);
        }
    }

    fn preview_ui(&mut self, ui: &mut Ui, p: &theme::Palette) {
        let text = self
            .editor
            .as_ref()
            .map(|e| e.text.clone())
            .unwrap_or_default();
        let key = hash_str(&text) ^ self.tree_version.rotate_left(17);
        if self.preview_cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let blocks = markdown::parse(&text, self);
            self.preview_cache = Some((key, blocks));
        }
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, p.page_bg);
        let look = Look {
            size: editor::SIZE,
            text: p.text,
            muted: p.muted,
            heading: p.heading,
            link: p.link,
            border: p.border,
            code_bg: p.code_bg,
            accent: p.accent,
            line_height: 1.6,
            colored_headings: true,
        };
        let blocks = self
            .preview_cache
            .as_ref()
            .map(|(_, b)| b.clone())
            .unwrap_or_default();
        let mut images = std::mem::take(&mut self.preview_images);
        let mut click = None;
        egui::ScrollArea::vertical()
            .id_salt(("preview", self.note.clone()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let avail = ui.available_width();
                let col = (avail - 56.0).clamp(100.0, 44.0 * editor::SIZE);
                let left = ((avail - col) / 2.0).max(28.0);
                ui.add_space(24.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(left);
                    ui.vertical(|ui| {
                        ui.set_width(col);
                        let mut painter = Painter {
                            look: &look,
                            images: &mut images,
                            click: None,
                            hovered_link: false,
                        };
                        painter.blocks(ui, &blocks, col, 0);
                        click = painter.click;
                    });
                });
                ui.add_space(ui.ctx().content_rect().height() * 0.4);
            });
        // Images the preview wants: fetched and decoded off the UI thread.
        for key in std::mem::take(&mut images.wanted) {
            let api = self.api.clone();
            let k2 = key.clone();
            self.bridge.spawn(
                move || {
                    let bytes = if k2.starts_with("data:") {
                        markdown::data_uri(&k2)
                    } else {
                        api.url_bytes(&k2).ok()
                    };
                    bytes.and_then(|b| markdown::decode(&b))
                },
                move |app, img| {
                    let ctx = app.bridge.ctx().clone();
                    app.preview_images.insert(&ctx, key, img);
                },
            );
        }
        self.preview_images = images;
        match click {
            Some(markdown::Click::Task(line)) => {
                let ctx = self.bridge.ctx().clone();
                // The preview is redrawn from the text; the editor is what changes the note.
                let ok = self
                    .editor
                    .as_mut()
                    .is_some_and(|e| e.toggle_task(&ctx, line));
                if !ok {
                    self.flash(
                        "Could not toggle this task from the preview; edit it in the editor.",
                    );
                }
            }
            Some(markdown::Click::Link(markdown::Target::Wiki {
                target, page, cfi, ..
            })) => self.open_link(target, page, cfi),
            Some(markdown::Click::Link(markdown::Target::Href(href))) => {
                let lower = href.to_lowercase();
                if lower.starts_with("http:")
                    || lower.starts_with("https:")
                    || lower.starts_with("mailto:")
                {
                    open_external(&href);
                } else if !href.is_empty() && !href.starts_with('#') {
                    // Markdown link to another vault note/file: [text](folder/note.md)
                    let decoded = percent_encoding::percent_decode_str(&href)
                        .decode_utf8_lossy()
                        .into_owned();
                    self.open_link(decoded, None, None);
                }
            }
            None => {}
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, p: &theme::Palette) {
        Frame::new().inner_margin(Margin::same(40)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 10.0);
            let line = |ui: &mut Ui, parts: &[(&str, bool)], color: Color32| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 8.0);
                    for (t, is_kbd) in parts {
                        if *is_kbd {
                            kit::kbd(ui, t);
                        } else {
                            ui.label(RichText::new(*t).font(fonts::ui(15.0)).color(color));
                        }
                    }
                });
            };
            line(
                ui,
                &[
                    ("Open a note from the sidebar or create one with ", false),
                    ("+", true),
                    (". To read a PDF or EPUB from this device (outside the vault), use the open folder button next to ", false),
                    ("+", true),
                    (".", false),
                ],
                p.text,
            );
            let m = p.muted;
            line(
                ui,
                &[
                    ("Ctrl+O", true),
                    (" quick switcher · ", false),
                    ("Ctrl+Shift+F", true),
                    (" full-text search · ", false),
                    ("Ctrl+Shift+O", true),
                    (" outline · ", false),
                    ("Ctrl+E", true),
                    (" preview ·  ", false),
                    ("Ctrl+\\", true),
                    (" collapse notes · ", false),
                    ("Alt+\\", true),
                    (" collapse PDF · ", false),
                    ("Ctrl+Shift+\\", true),
                    (" sidebar · ", false),
                    ("Ctrl+Shift+L", true),
                    (" insert page link · ", false),
                    ("Alt+↑/↓", true),
                    (" scroll the PDF · ", false),
                    ("Alt+Shift+↑/↓", true),
                    (" change page · ", false),
                    ("Ctrl+click", true),
                    (" or ", false),
                    ("Ctrl+Enter", true),
                    (" follows a ", false),
                    ("[[link]]", true),
                    (" · ", false),
                    (":w", true),
                    (" save", false),
                ],
                m,
            );
        });
    }

    fn toast_ui(&mut self, ctx: &egui::Context, p: &theme::Palette) {
        let Some((msg, _)) = &self.toast else { return };
        let screen = ctx.content_rect();
        let max_w = 560.0f32.min(screen.width() * 0.9);
        egui::Area::new(Id::new("toast"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -20.0))
            .interactable(false)
            .show(ctx, |ui| {
                let r = Frame::new()
                    .fill(p.panel)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(8)
                    .inner_margin(Margin {
                        left: 18,
                        right: 16,
                        top: 10,
                        bottom: 10,
                    })
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(51),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(max_w - 34.0);
                        ui.label(RichText::new(msg).font(fonts::ui(13.0)));
                    })
                    .response;
                let bar = Rect::from_min_size(r.rect.min, Vec2::new(3.0, r.rect.height()));
                ui.painter().rect_filled(
                    bar,
                    egui::CornerRadius {
                        nw: 8,
                        sw: 8,
                        ne: 0,
                        se: 0,
                    },
                    p.accent,
                );
            });
    }

    fn open_fonts(&mut self) {
        let mut fs = FontSettings::new();
        let _ = &mut fs;
        self.font_settings = Some(fs);
        self.bridge
            .spawn(font_settings::list_with_files, |app, (names, files)| {
                if let Some(fs) = app.font_settings.as_mut() {
                    fs.listed(names, files);
                }
            });
    }

    #[allow(clippy::too_many_lines)]
    fn overlays(&mut self, ctx: &egui::Context) {
        // Dialog (PromptDialog): answers go back to the flow that asked.
        if let Some(d) = self.dialog.as_mut() {
            if d.show(ctx) {
                self.dialog = self.dialog_queue.pop_front();
                if self.dialog.is_none() {
                    self.focus_editor();
                }
            }
            return;
        }
        if self.font_settings.is_some() {
            let before = self.prefs.fonts.clone();
            let mut prefs = self.prefs.fonts.clone();
            let out = self.font_settings.as_mut().map(|f| f.show(ctx, &mut prefs));
            self.prefs.fonts = prefs;
            if self.prefs.fonts != before {
                // Applied while the dialog is open, once typing pauses.
                self.fonts_due = Some(Instant::now() + Duration::from_millis(250));
            }
            if matches!(out, Some(font_settings::Outcome::Close)) {
                self.font_settings = None;
                self.apply_fonts(ctx);
                // Without a note open there is no editor to go back to.
                if self.editor.is_some() {
                    self.focus_editor();
                } else {
                    self.focus_fonts_button = true;
                }
            }
            return;
        }
        if let Some(m) = self.menu.as_mut() {
            match m.show(ctx, self.frame) {
                Ok(None) => {}
                Ok(Some(action)) => {
                    self.menu = None;
                    match action {
                        MenuAction::NewNoteIn(d) => self.new_note(Some(d)),
                        MenuAction::NewFolderIn(d) => self.new_folder(Some(d)),
                        MenuAction::Open(p) => self.open_path(p, None, None),
                        MenuAction::Rename(e) => self.rename_entry(e),
                        MenuAction::Trash(e) => self.trash_entry(e),
                    }
                }
                Err(()) => self.menu = None,
            }
        }
        if let (Some(panel), Some(note), Some(results)) = (
            self.backlinks_panel.as_mut(),
            self.note.clone(),
            self.backlinks.clone(),
        ) {
            match panel.show(ctx, &note, &results) {
                PanelOutcome::None => {}
                PanelOutcome::Close => {
                    self.backlinks_panel = None;
                    self.focus_editor();
                }
                PanelOutcome::Open(path, line) => {
                    self.backlinks_panel = None;
                    self.open_at_line(path, line);
                }
            }
        }
        if let Some(o) = self.outline.as_mut() {
            let text = self
                .editor
                .as_ref()
                .map(|e| e.text.clone())
                .unwrap_or_default();
            let hs = markdown::headings(&text);
            match o.show(ctx, &hs) {
                OutlineOutcome::None => {}
                OutlineOutcome::Close => {
                    self.outline = None;
                    self.focus_editor();
                }
                OutlineOutcome::Jump(line) => {
                    self.outline = None;
                    if let Some(n) = self.note.clone() {
                        self.open_at_line(n, line);
                    }
                }
            }
        }
        if let Some(s) = self.search.as_mut() {
            let (out, run) = s.show(ctx);
            if let Some(q) = run {
                let generation = s.generation;
                let api = self.api.clone();
                self.bridge.spawn(
                    move || api.search(&q).map_err(|e| e.to_string()),
                    move |app, res| {
                        if let Some(s) = app.search.as_mut() {
                            s.finish(generation, res);
                        }
                    },
                );
            }
            match out {
                PanelOutcome::None => {}
                PanelOutcome::Close => {
                    if let Some(s) = self.search.take() {
                        self.last_search = s.query;
                    }
                    self.focus_editor();
                }
                PanelOutcome::Open(path, line) => {
                    if let Some(s) = self.search.take() {
                        self.last_search = s.query;
                    }
                    self.open_at_line(path, line);
                }
            }
        }
        if let Some(sw) = self.switcher.as_mut() {
            let files: Vec<String> = self
                .files
                .iter()
                .filter(|p| is_md(p) || is_pdf(p) || is_epub(p))
                .cloned()
                .collect();
            match sw.show(ctx, &files, &self.prefs.recent) {
                SwitchOutcome::None => {}
                SwitchOutcome::Close => {
                    self.switcher = None;
                    self.focus_editor();
                }
                SwitchOutcome::Open(p) => {
                    self.switcher = None;
                    self.focus_editor();
                    self.open_path(p, None, None);
                }
                SwitchOutcome::Create(p) => {
                    self.switcher = None;
                    self.bridge.flow(move |b| create_and_open(&b, p));
                }
            }
        }
    }

    // ------------------------------------------------------------ flows

    /// dir: subfolder chosen in the tree; without it, asks for the full path.
    fn new_note(&mut self, dir: Option<String>) {
        // With a PDF open, suggests a reading note linked to it.
        let reader_base = self.reader.as_ref().map(|r| {
            let n = file_name(&r.link).to_string();
            let lower = n.to_lowercase();
            if lower.ends_with(".pdf") || lower.ends_with(".epub") {
                n[..n.rfind('.').unwrap_or(n.len())].to_string()
            } else {
                n
            }
        });
        let base = reader_base.clone().unwrap_or_else(|| "New note".into());
        self.bridge.flow(move |b| {
            let path = match dir {
                Some(dir) => {
                    let Some(name) = b.ask(format!("New note in \"{dir}\":"), base, "") else {
                        return;
                    };
                    let name = name.trim().trim_start_matches('/').to_string();
                    if name.is_empty() {
                        return;
                    }
                    format!("{dir}/{name}")
                }
                None => {
                    let suggestion = if reader_base.is_some() {
                        format!("Book notes/{base}.md")
                    } else {
                        format!("Random notes/{base}.md")
                    };
                    let Some(p) = b.ask(
                        "New note:",
                        suggestion,
                        "Path relative to the vault; missing folders are created.",
                    ) else {
                        return;
                    };
                    p.trim().to_string()
                }
            };
            if !path.is_empty() {
                create_and_open(&b, path);
            }
        });
    }

    /// parent: folder chosen in the tree; without it, creates from the vault root.
    fn new_folder(&mut self, parent: Option<String>) {
        let api = self.api.clone();
        self.bridge.flow(move |b| {
            let (msg, hint) = match &parent {
                Some(p) => (format!("New subfolder in \"{p}\":"), String::new()),
                None => (
                    "New folder:".to_string(),
                    "Path relative to the vault.".to_string(),
                ),
            };
            let Some(name) = b.ask(msg, "", hint) else {
                return;
            };
            let name = name.trim().trim_matches('/').to_string();
            if name.is_empty() {
                return;
            }
            if invalid_path(&name) {
                return b.flash(INVALID);
            }
            let full = match &parent {
                Some(p) => format!("{p}/{name}"),
                None => name,
            };
            match api.create_dir(&full) {
                Ok(path) => {
                    let tree = api.tree();
                    b.post(move |app| {
                        if let Ok(t) = tree {
                            app.set_tree(t);
                        }
                        app.flash(format!("Folder created: {path}"));
                    });
                }
                Err(e) => b.flash(e.to_string()),
            }
        });
    }

    fn rename_entry(&mut self, e: Entry) {
        let api = self.api.clone();
        self.bridge.flow(move |b| {
            let msg = if e.dir {
                "Rename / move folder:"
            } else {
                "Rename / move:"
            };
            let Some(to) = b.ask(
                msg,
                e.path.clone(),
                "Change the name or the path; missing folders are created.",
            ) else {
                return;
            };
            let mut to = to.trim().trim_matches('/').to_string();
            if to.is_empty() || to == e.path {
                return;
            }
            // No extension in the new name: keep the original one (a note stays .md, a PDF stays .pdf).
            let ext = if e.dir {
                String::new()
            } else {
                e.name
                    .rfind('.')
                    .map(|i| e.name[i..].to_string())
                    .unwrap_or_default()
            };
            let last = file_name(&to).to_string();
            let has_ext = last.rfind('.').is_some_and(|i| i + 1 < last.len());
            if !ext.is_empty() && !has_ext {
                to.push_str(&ext);
            }
            if invalid_path(&to) {
                return b.flash(INVALID);
            }
            let target = to.clone();
            let mut moved = None;
            with_settled(&b, &e, || match api.move_item(&e.path, &target) {
                Ok(path) => {
                    let (e2, p2) = (e.clone(), path.clone());
                    b.call(move |app| {
                        if inside(app.note.as_deref(), &e2) {
                            let n = remap(app.note.as_deref().unwrap_or(""), &e2.path, &p2);
                            app.note = Some(n.clone());
                            if let Some(ed) = app.editor.as_mut() {
                                ed.path = n;
                            }
                        }
                        if inside(app.doc.as_deref(), &e2) {
                            let d = remap(app.doc.as_deref().unwrap_or(""), &e2.path, &p2);
                            app.doc = Some(d);
                        }
                        app.prefs.recent = app
                            .prefs
                            .recent
                            .iter()
                            .map(|r| {
                                if inside(Some(r), &e2) {
                                    remap(r, &e2.path, &p2)
                                } else {
                                    r.clone()
                                }
                            })
                            .collect();
                        app.flash(format!("Moved to {p2}"));
                    });
                    if !e.dir {
                        moved = Some(path);
                    }
                }
                Err(ApiError::Exists(_)) => {
                    b.flash(format!("Something already exists at {target}"))
                }
                Err(err) => b.flash(err.to_string()),
            });
            refresh_tree_sync(&b, &api);
            if let Some(m) = moved {
                offer_relink(&b, &api, &e.path, &m);
            }
        });
    }

    fn trash_entry(&mut self, e: Entry) {
        let api = self.api.clone();
        self.bridge.flow(move |b| {
            let shown = links::strip_md(&e.name).to_string();
            let ok = b.confirm(
                format!("Move “{shown}” to the trash?"),
                format!(
                    "{} to .trash in the vault (the Obsidian trash); you can restore it from there.",
                    if e.dir { "The whole folder goes" } else { "It goes" }
                ),
                "Move to trash",
                true,
            );
            if !ok {
                return;
            }
            with_settled(&b, &e, || match api.trash(&e.path) {
                Ok(_) => {
                    let e2 = e.clone();
                    let shown = shown.clone();
                    b.call(move |app| {
                        if inside(app.note.as_deref(), &e2) {
                            // Nothing to save: the note is gone.
                            app.editor = None;
                            app.note = None;
                            app.backlinks = None;
                        }
                        if inside(app.doc.as_deref(), &e2) {
                            app.close_reader();
                        }
                        app.prefs.recent.retain(|r| !inside(Some(r), &e2));
                        app.flash(format!("“{shown}” moved to the trash"));
                    });
                }
                Err(err) => b.flash(err.to_string()),
            });
            refresh_tree_sync(&b, &api);
        });
    }

    fn paste_images(&mut self, images: Vec<(Vec<u8>, usize)>) {
        let Some(note) = self.note.clone() else {
            return;
        };
        if self.editor.as_ref().is_some_and(Editor::readonly) {
            return;
        }
        let api = self.api.clone();
        self.bridge.flow(move |b| {
            let mut pos: Option<usize> = None;
            for (i, (bytes, at)) in images.into_iter().enumerate() {
                let before = b
                    .call(|app| app.editor.as_ref().map(|e| e.text.clone()))
                    .flatten();
                let path = match api.upload_image(&note, &bytes) {
                    Ok(p) => p,
                    Err(e) => return b.flash(e.to_string()),
                };
                // The preview resolves ![[image]] through the tree.
                refresh_tree_sync(&b, &api);
                let note2 = note.clone();
                let start = pos.unwrap_or(at);
                let end = b
                    .call(move |app| {
                        let files = app.files.clone();
                        let ctx = app.bridge.ctx().clone();
                        // Ignore if the user switched notes while the image was uploading.
                        let ed = app.editor.as_mut().filter(|e| e.path == note2)?;
                        // If the text changed during the upload, the image goes where the cursor is now.
                        let at = if Some(&ed.text) == before.as_ref() {
                            start
                        } else {
                            ed.cursor()
                        };
                        let insert = format!(
                            "{}![[{}]]",
                            if i > 0 { "\n" } else { "" },
                            links::link_text(&path, &files)
                        );
                        let end = ed.insert_at(&ctx, at, &insert);
                        ed.focus();
                        Some(end)
                    })
                    .flatten();
                match end {
                    Some(e) => pos = Some(e),
                    None => return,
                }
            }
        });
    }
}

/// canLeaveNote: locks the editor, saves pending changes, unlocks.
fn leave_note(b: &Bridge) -> bool {
    b.call(|app| {
        if let Some(e) = app.editor.as_mut() {
            e.set_locked(true);
        }
    });
    let ok = flush_editor(b);
    b.call(|app| {
        if let Some(e) = app.editor.as_mut() {
            e.set_locked(false);
        }
    });
    if !ok {
        b.flash("Resolve the conflict or save error before leaving this note.");
    }
    ok
}

fn refresh_tree_sync(b: &Bridge, api: &Api) {
    match api.tree() {
        Ok(t) => b.post(move |app| app.set_tree(t)),
        Err(e) => b.flash(e.to_string()),
    }
}

/// Runs an operation on item e. If the open note is affected, it locks the editor, saves
/// pending changes first (otherwise autosave would recreate the old file) and unlocks at the end.
fn with_settled(b: &Bridge, e: &Entry, op: impl FnOnce()) {
    let e2 = e.clone();
    let affected = b
        .call(move |app| {
            let hit = inside(app.note.as_deref(), &e2);
            if hit && let Some(ed) = app.editor.as_mut() {
                ed.set_locked(true);
            }
            hit
        })
        .unwrap_or(false);
    if affected && !flush_editor(b) {
        b.flash("Resolve the conflict or error in the open note before moving or deleting.");
    } else {
        op();
    }
    if affected {
        b.call(|app| {
            if let Some(ed) = app.editor.as_mut() {
                ed.set_locked(false);
            }
        });
    }
}

/// Creates the note (with a title and, if a PDF is open, the source frontmatter) and opens
/// it. If it already exists, it just opens it: the server never overwrites an existing note.
fn create_and_open(b: &Bridge, path: String) {
    let mut path = path.trim().trim_start_matches('/').to_string();
    if !path.ends_with(".md") {
        path.push_str(".md");
    }
    if invalid_path(&path) {
        return b.flash(INVALID);
    }
    let p2 = path.clone();
    let Some((exists, reader_link, api)) = b.call(move |app| {
        (
            app.files.contains(&p2),
            app.reader.as_ref().map(|r| r.link.clone()),
            app.api.clone(),
        )
    }) else {
        return;
    };
    if exists {
        b.post(move |app| app.open_path(path, None, None));
        return;
    }
    let title = links::strip_md(file_name(&path)).to_string();
    let content = match reader_link {
        Some(l) => format!("---\nsource: \"[[{l}]]\"\n---\n\n# {title}\n\n"),
        None => format!("# {title}\n\n"),
    };
    match api.create(&path, &content) {
        Ok(_) | Err(ApiError::Exists(_)) => {}
        // Already exists (Neovim/Obsidian created it before the tree updated): open the existing one.
        Err(e) => return b.flash(e.to_string()),
    }
    refresh_tree_sync(b, &api);
    b.post(move |app| app.open_path(path, None, None));
}

/// After renaming a file, offers to update the [[links]] of the notes that referenced it.
fn offer_relink(b: &Bridge, api: &Api, from: &str, to: &str) {
    // A note became another type (or vice versa): the link form changes; left to the user.
    if is_md(from) != is_md(to) {
        return;
    }
    // No ripgrep or the search failed: the rename already worked, just do not offer.
    let Ok(plan) = api.preview_relink(from, to) else {
        return;
    };
    let ambiguous_note = if plan.ambiguous {
        " Another file with the old name still exists, so name-only links stay as they are."
    } else {
        ""
    };
    if plan.files.is_empty() {
        if plan.ambiguous {
            b.flash(format!(
                "Name-only links were not changed: another “{}” still exists.",
                file_name(from)
            ));
        }
        return;
    }
    let n = plan.files.len();
    let shown = plan
        .files
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = if n > 5 {
        format!(" and {} more", n - 5)
    } else {
        String::new()
    };
    let ok = b.confirm(
        format!("Update the links in {n} {}?", if n == 1 { "note" } else { "notes" }),
        format!(
            "{shown}{more} still {} to the old name (links inside code blocks change too).{ambiguous_note}",
            if n == 1 { "points" } else { "point" }
        ),
        "Update links",
        false,
    );
    if !ok {
        return;
    }
    // The open note may be one of the referencing notes: lock the editor and save pending
    // changes first (the server skips notes changed during the operation instead of overwriting).
    let files = plan.files.clone();
    let affected = b
        .call(move |app| {
            let hit = app.note.as_ref().is_some_and(|n| files.contains(n));
            if hit && let Some(ed) = app.editor.as_mut() {
                ed.set_locked(true);
            }
            hit
        })
        .unwrap_or(false);
    let unlock = |b: &Bridge| {
        b.call(|app| {
            if let Some(ed) = app.editor.as_mut() {
                ed.set_locked(false);
            }
        });
    };
    if affected && !flush_editor(b) {
        unlock(b);
        return b
            .flash("Resolve the conflict or error in the open note before updating the links.");
    }
    match api.relink(from, to) {
        Ok(done) => {
            let mut msg = format!(
                "{} {}",
                done.updated.len(),
                if done.updated.len() == 1 {
                    "note updated"
                } else {
                    "notes updated"
                }
            );
            if !done.skipped.is_empty() {
                msg.push_str(&format!(
                    "; skipped (changed meanwhile): {}",
                    done.skipped.join(", ")
                ));
            }
            b.flash(msg);
        }
        Err(e) => b.flash(e.to_string()),
    }
    unlock(b);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame += 1;
        self.drain();
        self.apply_theme(&ctx);
        if let Some(s) = self.start.take() {
            let _ = s;
        }
        match &mut self.screen {
            Screen::Loading => {
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, theme::palette(self.prefs.theme).bg);
                return;
            }
            Screen::Setup(setup) => {
                match setup.ui(ui) {
                    setup::Action::Submit(mode, path) => {
                        let api = self.api.clone();
                        self.bridge.spawn(
                            move || api.run_setup(&mode, &path).map_err(|e| e.to_string()),
                            |app, res| {
                                if let Screen::Setup(s) = &mut app.screen {
                                    s.finished(res);
                                }
                            },
                        );
                    }
                    setup::Action::None => {}
                }
                if let Screen::Setup(s) = &self.screen
                    && let Some(t) = s.enter_at
                {
                    if Instant::now() >= t {
                        self.enter_main();
                    } else {
                        ctx.request_repaint_after(t - Instant::now());
                    }
                }
                return;
            }
            Screen::Main => {}
        }
        self.timers(&ctx);
        self.capture_keys(&ctx);
        self.bubble_keys(&ctx);
        let (b, a) = (self.bridge.clone(), self.api.clone());
        if let Some(ed) = self.editor.as_mut() {
            ed.tick(&b, &a);
        }
        self.ui_main(ui);
        self.overlays(&ctx);
        if self.dragging.is_some() {
            ctx.set_cursor_icon(CursorIcon::ResizeColumn);
        }
        self.record_history();
        self.save_session();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Like beforeunload: pending changes are written before the window goes away.
        if let Some(ed) = self.editor.as_mut()
            && matches!(ed.status, Status::Dirty)
            && let (Some(mtime), true) = (ed.base_mtime(), ed.has_base())
        {
            let _ = self.api.save(&ed.path, &ed.text, &mtime);
        }
        self.store.save(&self.prefs);
    }
}
