//! Ports of the few Go `path`/`io/fs` helpers whose exact behaviour the HTTP
//! contract depends on (`path.Clean`, `path.Ext`, `fs.ValidPath`, ...).

/// `path.Clean`: purely lexical normalisation.
pub fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, true) => "/".into(),
        (true, false) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

/// `path.Base`.
pub fn base(p: &str) -> &str {
    if p.is_empty() {
        return ".";
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return "/";
    }
    t.rsplit('/').next().unwrap_or(t)
}

/// `path.Dir`.
pub fn dir(p: &str) -> String {
    match p.rfind('/') {
        None => ".".into(),
        Some(i) => clean(&p[..=i]),
    }
}

/// `path.Ext`: the suffix starting at the last dot of the last element.
pub fn ext(p: &str) -> &str {
    let b = p.as_bytes();
    for i in (0..b.len()).rev() {
        if b[i] == b'/' {
            break;
        }
        if b[i] == b'.' {
            return &p[i..];
        }
    }
    ""
}

/// `path.Join`: ignores empty elements, cleans the result.
pub fn join(parts: &[&str]) -> String {
    let non_empty: Vec<&str> = parts.iter().copied().filter(|s| !s.is_empty()).collect();
    if non_empty.is_empty() {
        return String::new();
    }
    clean(&non_empty.join("/"))
}

/// `fs.ValidPath`.
pub fn valid_path(name: &str) -> bool {
    if name == "." {
        return true;
    }
    if name.is_empty() {
        return false;
    }
    name.split('/')
        .all(|e| !(e.is_empty() || e == "." || e == ".."))
}

/// Go's `strings.ToValidUTF8(b, "\u{FFFD}")`: each *run* of invalid bytes becomes one replacement.
pub fn to_valid_utf8(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    let mut in_invalid = false;
    for chunk in b.utf8_chunks() {
        let valid = chunk.valid();
        if !valid.is_empty() {
            out.push_str(valid);
            in_invalid = false;
        }
        if !chunk.invalid().is_empty() && !in_invalid {
            out.push('\u{FFFD}');
            in_invalid = true;
        }
    }
    out
}

/// Go's `string(bytes)` followed by JSON encoding: every invalid byte becomes one U+FFFD
/// (unlike `String::from_utf8_lossy`, which collapses truncated sequences).
pub fn lossy_per_byte(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        out.push_str(chunk.valid());
        for _ in chunk.invalid() {
            out.push('\u{FFFD}');
        }
    }
    out
}

/// `regexp.QuoteMeta`: escapes only `\.+*?()|[]{}^$`.
pub fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_matches_go() {
        for (i, o) in [
            ("", "."),
            ("a//b", "a/b"),
            ("a/./b/", "a/b"),
            ("a/../..", ".."),
            ("/a/../..", "/"),
            ("/../x", "/x"),
            ("../../x", "../../x"),
            ("a/b/../c", "a/c"),
        ] {
            assert_eq!(clean(i), o, "clean({i:?})");
        }
    }

    #[test]
    fn ext_base_dir() {
        assert_eq!(ext("a/b.c/d"), "");
        assert_eq!(ext("a/b.MD"), ".MD");
        assert_eq!(base("a/b/"), "b");
        assert_eq!(dir("a/b/c.md"), "a/b");
        assert_eq!(dir("c.md"), ".");
        assert_eq!(join(&[".", "x"]), "x");
        assert_eq!(join(&["a", "", "/b"]), "a/b");
    }

    #[test]
    fn valid_utf8_runs() {
        assert_eq!(to_valid_utf8(b"fun\xe7\xe3o"), "fun\u{FFFD}o");
        assert_eq!(to_valid_utf8(b"ok"), "ok");
    }
}
