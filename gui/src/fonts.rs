// Fonts of the interface, of the notes and of the code (web/src/lib/fonts.ts). The browser finds
// installed fonts by name through CSS; here fontconfig (fc-match / fc-list) does the same, and the
// chosen files are loaded into egui ahead of the default chain of each role.

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;

use eframe::egui::{FontData, FontDefinitions, FontFamily, FontId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Ui,
    Editor,
    Mono,
}

pub const ROLES: [Role; 3] = [Role::Ui, Role::Editor, Role::Mono];

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Ui => "Interface",
            Role::Editor => "Notes",
            Role::Mono => "Code",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Role::Ui => "Sidebar, toolbars, dialogs",
            Role::Editor => "Editor and preview",
            Role::Mono => "Code blocks and shortcuts",
        }
    }
    /// The first font of the default chain, shown while the field is empty.
    pub fn default_name(self) -> &'static str {
        match self {
            Role::Ui => "system font",
            Role::Editor => "EB Garamond",
            Role::Mono => "JetBrains Mono",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Role::Ui => "ui",
            Role::Editor => "editor",
            Role::Mono => "mono",
        }
    }
}

/// Generic families: fontconfig aliases, like the CSS keywords.
const GENERIC: [&str; 13] = [
    "serif",
    "sans-serif",
    "monospace",
    "cursive",
    "fantasy",
    "system-ui",
    "ui-serif",
    "ui-sans-serif",
    "ui-monospace",
    "ui-rounded",
    "math",
    "emoji",
    "fangsong",
];

fn font_char(c: char) -> bool {
    c.is_alphanumeric() || c == ' ' || c == '.' || c == '_' || c == '-'
}

/// The name without the characters it cannot have, keeping spaces and length.
pub fn keep_font_chars(value: &str) -> String {
    value.chars().filter(|&c| font_char(c)).collect()
}

/// What can be part of a font family name, and nothing else (an emptied field means the default).
pub fn clean_font_name(value: &str) -> String {
    let kept = keep_font_chars(value);
    let collapsed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(64).collect::<String>().trim().to_string()
}

pub fn is_generic(name: &str) -> bool {
    GENERIC.contains(&name.to_lowercase().as_str())
}

/// fontconfig's name for a CSS generic family.
fn fc_pattern(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "system-ui" | "ui-sans-serif" | "ui-rounded" => "sans-serif".into(),
        "ui-serif" => "serif".into(),
        "ui-monospace" => "monospace".into(),
        "fangsong" | "math" => "serif".into(),
        other if is_generic(other) => other.into(),
        _ => name.to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub family: String,
    pub file: String,
    pub index: u32,
}

/// The file fontconfig uses for `pattern`, if its family really is the one asked for
/// (fc-match always answers something: a missing font falls back to another one).
pub fn fc_match(name: &str, style: &str) -> Option<Found> {
    let pattern = fc_pattern(name);
    let full = if style.is_empty() { pattern.clone() } else { format!("{pattern}:{style}") };
    let out = Command::new("fc-match").arg("-f").arg("%{family}\n%{file}\n%{index}").arg(&full).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let families = lines.next()?.to_string();
    let file = lines.next()?.to_string();
    let index = lines.next().and_then(|i| i.trim().parse().ok()).unwrap_or(0);
    if file.is_empty() {
        return None;
    }
    let wanted = name.to_lowercase();
    let generic = is_generic(name);
    let matches = generic || families.split(',').any(|f| f.trim().to_lowercase() == wanted);
    matches.then(|| Found {
        family: families.split(',').next().unwrap_or("").to_string(),
        file,
        index,
    })
}

/// Whether this device can draw the family (the settings warn when it cannot).
pub fn available(name: &str) -> bool {
    let clean = clean_font_name(name);
    clean.is_empty() || is_generic(&clean) || clean.eq_ignore_ascii_case("EB Garamond") || fc_match(&clean, "").is_some()
}

/// Font families installed on this device (fc-list), sorted like localeCompare.
pub fn list_installed() -> Vec<String> {
    let Ok(out) = Command::new("fc-list").arg(":").arg("family").output() else { return Vec::new() };
    let mut names = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let first = line.split(',').next().unwrap_or("");
        let name = clean_font_name(first);
        if !name.is_empty() {
            names.insert(name);
        }
    }
    let mut v: Vec<String> = names.into_iter().collect();
    v.sort_by_key(|a| a.to_lowercase());
    v
}

static GARAMOND: &[u8] = include_bytes!("../assets/fonts/EBGaramond-Regular.ttf");
static GARAMOND_BOLD: &[u8] = include_bytes!("../assets/fonts/EBGaramond-Bold.ttf");
static GARAMOND_ITALIC: &[u8] = include_bytes!("../assets/fonts/EBGaramond-Italic.ttf");
static GARAMOND_BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/EBGaramond-BoldItalic.ttf");
static ICONS: &[u8] = include_bytes!("../assets/fonts/lucide-subset.ttf");

pub fn family(name: &str) -> FontFamily {
    FontFamily::Name(name.into())
}

/// Font of a role, with a weight/style variant: "editor", "editor-bold", "ui-bold"…
pub fn font(role: Role, bold: bool, italic: bool, size: f32) -> FontId {
    let suffix = match (bold, italic) {
        (false, false) => "",
        (true, false) => "-bold",
        (false, true) => "-italic",
        (true, true) => "-bolditalic",
    };
    FontId::new(size, family(&format!("{}{}", role.key(), suffix)))
}

pub fn ui(size: f32) -> FontId {
    font(Role::Ui, false, false, size)
}
pub fn ui_bold(size: f32) -> FontId {
    font(Role::Ui, true, false, size)
}
pub fn editor(size: f32) -> FontId {
    font(Role::Editor, false, false, size)
}
pub fn mono(size: f32) -> FontId {
    font(Role::Mono, false, false, size)
}
pub fn icons(size: f32) -> FontId {
    FontId::new(size, family("icons"))
}

/// The files chosen for every role and variant; the fonts are rebuilt only when this changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    files: BTreeMap<String, Found>,
}

const VARIANTS: [(&str, &str); 4] = [("", ""), ("-bold", "weight=bold"), ("-italic", "slant=italic"), ("-bolditalic", "weight=bold:slant=italic")];

/// Which files the current choices map to (the defaults included: the system UI font, and
/// JetBrains Mono when installed).
pub fn resolve(prefs: &crate::state::FontPrefs) -> Resolved {
    let mut files = BTreeMap::new();
    for role in ROLES {
        let custom = clean_font_name(prefs.get(role));
        for (suffix, style) in VARIANTS {
            if role == Role::Mono && !suffix.is_empty() {
                continue;
            }
            if !custom.is_empty() && !custom.eq_ignore_ascii_case("EB Garamond") {
                if let Some(f) = fc_match(&custom, style) {
                    files.insert(format!("custom-{}{}", role.key(), suffix), f);
                }
            }
        }
    }
    for (suffix, style) in [("", ""), ("-bold", "weight=bold")] {
        if let Some(f) = fc_match("sans-serif", style) {
            files.insert(format!("system-ui{suffix}"), f);
        }
    }
    let mono = fc_match("JetBrainsMono Nerd Font", "").or_else(|| fc_match("JetBrains Mono", ""));
    if let Some(f) = mono {
        files.insert("system-mono".into(), f);
    }
    Resolved { files }
}

fn load(found: &Found) -> Option<FontData> {
    let bytes = std::fs::read(&found.file).ok()?;
    let mut data = FontData::from_owned(bytes);
    data.index = found.index;
    Some(data)
}

/// egui font definitions: per role, the custom font first, then the default chain, then
/// egui's own fonts (accents, emoji) and the icon font, like the CSS tails.
pub fn definitions(resolved: &Resolved) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let builtin_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let builtin_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut add = |defs: &mut FontDefinitions, name: &str, data: FontData| {
        defs.font_data.insert(name.to_string(), Arc::new(data));
    };
    add(&mut defs, "garamond", FontData::from_static(GARAMOND));
    add(&mut defs, "garamond-bold", FontData::from_static(GARAMOND_BOLD));
    add(&mut defs, "garamond-italic", FontData::from_static(GARAMOND_ITALIC));
    add(&mut defs, "garamond-bolditalic", FontData::from_static(GARAMOND_BOLD_ITALIC));
    add(&mut defs, "icons", FontData::from_static(ICONS));
    let mut loaded = std::collections::BTreeSet::new();
    for (name, found) in &resolved.files {
        if let Some(data) = load(found) {
            add(&mut defs, name, data);
            loaded.insert(name.clone());
        }
    }
    let has = |n: &str| loaded.contains(n);
    let tail = |chain: Vec<String>| {
        let mut v = chain;
        v.extend(builtin_prop.iter().cloned());
        v.push("icons".into());
        v
    };

    // Interface: custom, system UI font, egui's.
    for (suffix, sys) in [("", "system-ui"), ("-bold", "system-ui-bold"), ("-italic", "system-ui"), ("-bolditalic", "system-ui-bold")] {
        let mut chain = Vec::new();
        let custom = format!("custom-ui{suffix}");
        if has(&custom) {
            chain.push(custom);
        } else if has("custom-ui") {
            chain.push("custom-ui".into());
        }
        if has(sys) {
            chain.push(sys.into());
        } else if has("system-ui") {
            chain.push("system-ui".into());
        }
        defs.families.insert(family(&format!("ui{suffix}")), tail(chain));
    }
    // Notes: custom, EB Garamond.
    for suffix in ["", "-bold", "-italic", "-bolditalic"] {
        let mut chain = Vec::new();
        let custom = format!("custom-editor{suffix}");
        if has(&custom) {
            chain.push(custom);
        } else if has("custom-editor") {
            chain.push("custom-editor".into());
        }
        chain.push(format!("garamond{suffix}"));
        defs.families.insert(family(&format!("editor{suffix}")), tail(chain));
    }
    // Code: custom, JetBrains Mono, egui's Hack.
    let mut chain = Vec::new();
    if has("custom-mono") {
        chain.push("custom-mono".to_string());
    }
    if has("system-mono") {
        chain.push("system-mono".into());
    }
    chain.extend(builtin_mono.iter().cloned());
    chain.extend(builtin_prop.iter().cloned());
    chain.push("icons".into());
    for suffix in ["", "-bold", "-italic", "-bolditalic"] {
        defs.families.insert(family(&format!("mono{suffix}")), chain.clone());
    }
    defs.families.insert(FontFamily::Monospace, chain);
    let ui = defs.families.get(&family("ui")).cloned().unwrap_or_default();
    defs.families.insert(FontFamily::Proportional, ui);
    defs.families.insert(family("icons"), vec!["icons".into()]);
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_names_like_the_web_app() {
        assert_eq!(clean_font_name("  Fira   Code; } "), "Fira Code");
        assert_eq!(clean_font_name("'Inter'"), "Inter");
        assert_eq!(clean_font_name(&"x".repeat(80)).len(), 64);
        assert_eq!(keep_font_chars("a'b c"), "ab c");
    }
}
