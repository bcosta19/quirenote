// What the browser keeps in localStorage/IndexedDB, kept here in one JSON file per device:
// panel sizes, theme, fonts, recent files, the last session and reading positions.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const SIDEBAR_W: f32 = 260.0;
pub const NOTES_PCT: f32 = 42.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FontPrefs {
    #[serde(default)]
    pub ui: String,
    #[serde(default)]
    pub editor: String,
    #[serde(default)]
    pub mono: String,
}

impl FontPrefs {
    pub fn get(&self, role: crate::fonts::Role) -> &str {
        match role {
            crate::fonts::Role::Ui => &self.ui,
            crate::fonts::Role::Editor => &self.editor,
            crate::fonts::Role::Mono => &self.mono,
        }
    }
    pub fn get_mut(&mut self, role: crate::fonts::Role) -> &mut String {
        match role {
            crate::fonts::Role::Ui => &mut self.ui,
            crate::fonts::Role::Editor => &mut self.editor,
            crate::fonts::Role::Mono => &mut self.mono,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub note: Option<String>,
    /// Vault PDF/EPUB.
    pub doc: Option<String>,
    pub page: u32,
    /// Path of a PDF/EPUB opened from this device (outside the vault).
    pub local: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub sidebar: bool,
    pub notes: bool,
    pub sidebar_w: f32,
    pub notes_pct: f32,
    pub theme: Theme,
    pub fonts: FontPrefs,
    pub recent: Vec<String>,
    pub session: Option<Session>,
    /// Last page read per PDF id.
    pub pages: BTreeMap<String, u32>,
    /// Last position (CFI) per EPUB id.
    pub epub: BTreeMap<String, String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            sidebar: true,
            notes: true,
            sidebar_w: SIDEBAR_W,
            notes_pct: NOTES_PCT,
            theme: Theme::Light,
            fonts: FontPrefs::default(),
            recent: Vec::new(),
            session: None,
            pages: BTreeMap::new(),
            epub: BTreeMap::new(),
        }
    }
}

pub struct Store {
    path: PathBuf,
    saved: String,
}

impl Store {
    pub fn dir_default() -> PathBuf {
        if let Some(d) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
            return PathBuf::from(d).join("quirenote-gui");
        }
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join(".local/state/quirenote-gui")
    }

    /// Reads the saved preferences; a missing or corrupted file must not keep the app from starting.
    pub fn open(dir: PathBuf) -> (Store, Prefs) {
        let path = dir.join("state.json");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut prefs: Prefs = serde_json::from_str(&text).unwrap_or_default();
        prefs.sanitize();
        (Store { path, saved: text }, prefs)
    }

    /// Writes only when something changed (called every frame is cheap enough: one serialization).
    pub fn save(&mut self, prefs: &Prefs) {
        let Ok(text) = serde_json::to_string_pretty(prefs) else { return };
        if text == self.saved {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, &self.path).is_ok() {
            self.saved = text;
        }
    }
}

impl Prefs {
    fn sanitize(&mut self) {
        if !self.sidebar_w.is_finite() || self.sidebar_w <= 0.0 {
            self.sidebar_w = SIDEBAR_W;
        }
        if !self.notes_pct.is_finite() || self.notes_pct <= 0.0 {
            self.notes_pct = NOTES_PCT;
        }
        self.sidebar_w = self.sidebar_w.clamp(160.0, 520.0);
        self.notes_pct = self.notes_pct.clamp(20.0, 80.0);
        for role in crate::fonts::ROLES {
            let clean = crate::fonts::clean_font_name(self.fonts.get(role));
            *self.fonts.get_mut(role) = clean;
        }
        self.recent.truncate(30);
        if let Some(s) = &mut self.session {
            if s.page == 0 {
                s.page = 1;
            }
        }
    }

    pub fn remember(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(30);
    }
}
