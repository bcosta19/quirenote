//! The user configuration (`~/.config/quirenote/config.toml`), which records the chosen
//! vault (port of internal/config).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    /// Absolute path of the vault; empty = not configured yet.
    pub vault: String,
}

/// `$XDG_CONFIG_HOME/quirenote/config.toml`, like Go's `os.UserConfigDir`.
pub fn path() -> Result<PathBuf, String> {
    let dir = match std::env::var("XDG_CONFIG_HOME") {
        Ok(d) if !d.is_empty() => {
            if !Path::new(&d).is_absolute() {
                return Err("path in $XDG_CONFIG_HOME is relative".into());
            }
            PathBuf::from(d)
        }
        _ => match std::env::var("HOME") {
            Ok(h) if !h.is_empty() => PathBuf::from(h).join(".config"),
            _ => return Err("neither $XDG_CONFIG_HOME nor $HOME are defined".into()),
        },
    };
    Ok(dir.join("quirenote").join("config.toml"))
}

/// `strconv.Quote` for the characters that matter in paths.
pub fn go_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\x0b' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `strconv.Unquote` for double-quoted strings (the only form the app writes) and raw
/// back-quoted strings.
pub fn go_unquote(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() < 2 {
        return None;
    }
    match (b[0], b[b.len() - 1]) {
        (b'`', b'`') => {
            let inner = &s[1..s.len() - 1];
            if inner.contains('`') {
                None
            } else {
                Some(inner.replace('\r', ""))
            }
        }
        (b'"', b'"') => {
            let inner = &s[1..s.len() - 1];
            let mut out = String::new();
            let mut it = inner.chars();
            while let Some(c) = it.next() {
                match c {
                    '"' | '\n' => return None,
                    '\\' => match it.next()? {
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'a' => out.push('\x07'),
                        'b' => out.push('\x08'),
                        'f' => out.push('\x0c'),
                        'v' => out.push('\x0b'),
                        '\\' => out.push('\\'),
                        '"' => out.push('"'),
                        'x' => {
                            let h: String = it.by_ref().take(2).collect();
                            let n = u8::from_str_radix(&h, 16).ok()?;
                            if !n.is_ascii() {
                                return None; // raw bytes are not representable in a String
                            }
                            out.push(n as char);
                        }
                        'u' => {
                            let h: String = it.by_ref().take(4).collect();
                            out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
                        }
                        _ => return None,
                    },
                    c => out.push(c),
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// Reads the config file. A missing file is not an error: it returns an empty `Config`.
/// Only the subset of TOML the app writes is understood: `key = "string"` lines and comments.
pub fn load(path: &Path) -> Result<Config, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(format!("open {}: {e}", path.display())),
    };
    let mut c = Config::default();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            return Err(format!("{}:{n}: expected key = \"value\"", path.display()));
        };
        let Some(s) = go_unquote(val.trim()) else {
            return Err(format!(
                "{}:{n}: the value must be a quoted string",
                path.display()
            ));
        };
        if key.trim() == "vault" {
            c.vault = s;
        }
    }
    Ok(c)
}

/// Writes the config file atomically, creating its directory.
pub fn save(path: &Path, c: &Config) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new().recursive(true).create(dir)?;
    }
    let data = format!(
        "# Quirenote configuration\nvault = {}\n",
        go_quote(&c.vault)
    );
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = fs::File::create(&tmp)?;
    f.write_all(data.as_bytes())?;
    drop(f);
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("config.toml");
        assert_eq!(load(&p).unwrap(), Config::default());
        let want = r#"/home/x/My "Notes""#;
        save(&p, &Config { vault: want.into() }).unwrap();
        assert_eq!(load(&p).unwrap().vault, want);
    }

    #[test]
    fn load_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        for bad in ["vault /x\n", "vault = /x\n"] {
            fs::write(&p, bad).unwrap();
            assert!(load(&p).is_err(), "{bad:?} should fail");
        }
    }
}
