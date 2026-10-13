//! Ports of internal/vault/vault_test.go and regression_test.go.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::*;

fn new_vault() -> (Arc<Vault>, PathBuf, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fs::canonicalize(tmp.path()).unwrap();
    fs::create_dir(dir.join("livros")).unwrap();
    fs::create_dir(dir.join(".obsidian")).unwrap();
    for (name, body) in [
        ("a.md", "# A"),
        ("livros/b.pdf", "%PDF"),
        ("livros/img.png", "png"),
        (".obsidian/x.json", "{}"),
    ] {
        fs::write(dir.join(name), body).unwrap();
    }
    (Arc::new(Vault::open(&dir).unwrap()), dir, tmp)
}

fn set_mtime(p: &Path, t: SystemTime) {
    let f = fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(t).unwrap();
}

#[test]
fn tree_hides_dotfiles_and_lists_known_extensions() {
    let (v, _, _t) = new_vault();
    let tree = v.tree_sync().unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].path, "livros");
    assert_eq!(tree[1].path, "a.md");
    let kids: Vec<_> = tree[0].children.iter().map(|k| k.path.as_str()).collect();
    assert_eq!(kids, ["livros/b.pdf", "livros/img.png"]);
}

#[test]
fn rejects_paths_outside_vault() {
    let (v, _, _t) = new_vault();
    for p in [
        "../etc/passwd",
        "/etc/passwd",
        "livros/../../x.md",
        ".obsidian/x.json",
        "",
    ] {
        assert!(v.read_sync(p).is_err(), "read({p:?}) should fail");
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    for p in ["../etc/passwd", "livros/../../x.md", ".obsidian/x.json", ""] {
        assert!(
            rt.block_on(v.save(p, "x", 0)).is_err(),
            "save({p:?}) should fail"
        );
    }
}

#[test]
fn rejects_symlink_escape() {
    let (v, dir, _t) = new_vault();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.md");
    fs::write(&secret, "segredo").unwrap();
    std::os::unix::fs::symlink(&secret, dir.join("link.md")).unwrap();
    let err = v.read_sync("link.md").unwrap_err();
    assert!(err.is_escape(), "{err}");
}

#[tokio::test]
async fn save_detects_conflict() {
    let (v, dir, _t) = new_vault();
    let note = v.read("a.md").await.unwrap();

    // Another editor (e.g. Neovim) changes the file.
    fs::write(dir.join("a.md"), "# Neovim").unwrap();
    set_mtime(
        &dir.join("a.md"),
        SystemTime::now() + Duration::from_secs(1),
    );

    let Err(Error::Conflict(current)) = v.save("a.md", "# Web", note.mtime).await else {
        panic!("want Conflict");
    };
    assert_eq!(current.content, "# Neovim");

    // With the current mtime, it saves normally and leaves no temporary files.
    let saved = v.save("a.md", "# Web", current.mtime).await.unwrap();
    assert_eq!(saved.content, "# Web");
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "leftover temporary files: {leftovers:?}"
    );
}

#[tokio::test]
async fn save_creates_dirs_and_only_markdown() {
    let (v, dir, _t) = new_vault();
    v.save("novos/c.md", "oi", 0).await.unwrap();
    assert!(dir.join("novos/c.md").exists());
    assert!(matches!(
        v.save("livros/b.pdf", "x", 0).await,
        Err(Error::NotNote)
    ));
}

#[tokio::test]
async fn mkdir_cases() {
    let (v, dir, _t) = new_vault();
    assert_eq!(
        v.mkdir("Estudos/Compiladores").await.unwrap(),
        "Estudos/Compiladores"
    );
    assert!(dir.join("Estudos/Compiladores").is_dir());
    v.mkdir("Estudos/Compiladores").await.unwrap();
    for bad in ["../fora", "a/../../fora", ".oculta", "a/.obsidian", ""] {
        assert!(v.mkdir(bad).await.is_err(), "mkdir({bad:?}) should fail");
    }
    // A leading slash is relative to the vault, never the system root.
    assert_eq!(v.mkdir("/etc/x").await.unwrap(), "etc/x");
    assert!(dir.join("etc/x").exists());
}

#[tokio::test]
async fn create_never_overwrites() {
    let (v, dir, _t) = new_vault();
    let n = v.create("novas/ideia.md", "# Ideia\n").await.unwrap();
    assert_eq!(n.content, "# Ideia\n");
    assert!(matches!(
        v.create("a.md", "apagaria tudo").await,
        Err(Error::Exists)
    ));
    assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "# A");
    assert!(matches!(
        v.create("livros/b.pdf", "x").await,
        Err(Error::NotNote)
    ));
    assert!(v.create("../fora.md", "x").await.is_err());
}

#[tokio::test]
async fn move_cases() {
    let (v, dir, _t) = new_vault();
    assert_eq!(
        v.move_to("a.md", "pasta nova/b.md").await.unwrap(),
        "pasta nova/b.md"
    );
    assert_eq!(
        fs::read_to_string(dir.join("pasta nova/b.md")).unwrap(),
        "# A"
    );
    // Never overwrites.
    assert!(matches!(
        v.move_to("livros/b.pdf", "pasta nova/b.md").await,
        Err(Error::Exists)
    ));
    // Whole folder, and not into itself.
    assert!(matches!(
        v.move_to("livros", "livros/sub").await,
        Err(Error::InvalidPath)
    ));
    v.move_to("livros", "biblioteca").await.unwrap();
    assert!(dir.join("biblioteca/b.pdf").exists());
    for (from, to) in [
        ("../x", "y.md"),
        ("biblioteca/b.pdf", "../fora.pdf"),
        (".obsidian/x.json", "x.json"),
        ("biblioteca", ".trash/b"),
    ] {
        assert!(
            v.move_to(from, to).await.is_err(),
            "move({from:?}, {to:?}) should fail"
        );
    }
    let err = v.move_to("nao-existe.md", "x.md").await.unwrap_err();
    assert!(err.is_not_exist(), "{err}");
}

#[tokio::test]
async fn trash_cases() {
    let (v, dir, _t) = new_vault();
    let p1 = v.trash("a.md").await.unwrap();
    assert_eq!(p1, ".trash/a.md");
    assert!(!dir.join("a.md").exists());
    // Same name again: does not overwrite what is already in the trash.
    fs::write(dir.join("a.md"), "segunda").unwrap();
    let p2 = v.trash("a.md").await.unwrap();
    assert!(
        p2 != p1 && p2.starts_with(".trash/a ") && p2.ends_with(".md"),
        "{p2}"
    );
    assert_eq!(fs::read_to_string(dir.join(&p1)).unwrap(), "# A");
    // Folders go whole too; the trash itself cannot be trashed.
    v.trash("livros").await.unwrap();
    assert!(v.trash(".trash").await.is_err());
    assert!(v.tree().await.unwrap().is_empty());
}

const TINY_PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0, 0,
    0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
];

#[tokio::test]
async fn save_image_cases() {
    let (v, dir, _t) = new_vault();
    // No configuration: vault root, Obsidian naming, never overwrites.
    let p1 = v
        .save_image("livros/nota.md", TINY_PNG.to_vec())
        .await
        .unwrap();
    assert!(
        !p1.contains('/') && p1.starts_with("Pasted image ") && p1.ends_with(".png"),
        "{p1}"
    );
    let p2 = v
        .save_image("livros/nota.md", TINY_PNG.to_vec())
        .await
        .unwrap();
    assert_ne!(p1, p2);
    // Content that is not an image (or SVG, which may contain scripts) is rejected.
    for bad in [&b"<svg onload=alert(1)></svg>"[..], b"# nota", b""] {
        assert!(matches!(
            v.save_image("a.md", bad.to_vec()).await,
            Err(Error::NotImage)
        ));
    }
    // "./" = the note's folder; "./anexos" = a subfolder of it; "Anexos" = a fixed folder.
    for (cfg, want) in [
        ("./", "livros"),
        ("./anexos", "livros/anexos"),
        ("Anexos", "Anexos"),
        ("/", "."),
    ] {
        fs::write(
            dir.join(".obsidian/app.json"),
            format!(r#"{{"attachmentFolderPath":"{cfg}"}}"#),
        )
        .unwrap();
        let p = v
            .save_image("livros/nota.md", TINY_PNG.to_vec())
            .await
            .unwrap();
        assert_eq!(golike::dir(&p), want, "config {cfg:?}: {p}");
    }
    // A setting pointing outside the vault does not escape.
    fs::write(
        dir.join(".obsidian/app.json"),
        r#"{"attachmentFolderPath":"../fora"}"#,
    )
    .unwrap();
    assert!(
        v.save_image("livros/nota.md", TINY_PNG.to_vec())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bad_names() {
    let (v, dir, _t) = new_vault();
    for bad in ["C# notas.md", "x^y.md", "a [b].md", "a|b.md"] {
        assert!(
            matches!(v.create(bad, "x").await, Err(Error::BadName)),
            "{bad}"
        );
        assert!(
            matches!(v.move_to("a.md", bad).await, Err(Error::BadName)),
            "{bad}"
        );
    }
    assert!(matches!(v.mkdir("Pasta #1").await, Err(Error::BadName)));
    // An old folder with # (created outside the app) stays usable: only the new name is checked.
    fs::create_dir(dir.join("C#")).unwrap();
    v.create("C#/nota.md", "x").await.unwrap();
    assert!(v.exists("a.md").await && !v.exists("nao.md").await && !v.exists("../x").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_saves_reject_stale_versions() {
    let (v, dir, _t) = new_vault();
    set_mtime(
        &dir.join("a.md"),
        SystemTime::now() - Duration::from_secs(3600),
    );
    let base = v.read("a.md").await.unwrap();
    const CLIENTS: usize = 16;
    let mut tasks = Vec::new();
    for i in 0..CLIENTS {
        let v = Arc::clone(&v);
        let mtime = base.mtime;
        tasks.push(tokio::spawn(async move {
            v.save(
                "a.md",
                &format!("client {i}\n{}", "x".repeat(1 << 20)),
                mtime,
            )
            .await
        }));
    }
    let mut saved = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(_) => saved += 1,
            Err(Error::Conflict(_)) => {}
            Err(e) => panic!("unexpected save error: {e}"),
        }
    }
    assert_eq!(
        saved, 1,
        "want exactly one successful save of the same version"
    );
}

#[tokio::test]
async fn saved_bmp_attachments_appear_in_the_tree() {
    let (v, dir, _t) = new_vault();
    fs::write(
        dir.join(".obsidian/app.json"),
        r#"{"attachmentFolderPath":"Attachments"}"#,
    )
    .unwrap();
    let mut bmp = vec![b'B', b'M'];
    bmp.extend([0u8; 52]);
    let p = v.save_image("a.md", bmp).await.unwrap();
    let tree = v.tree().await.unwrap();
    assert!(
        tree.iter().any(|f| f.children.iter().any(|c| c.path == p)),
        "{p} missing from {tree:?}"
    );
}

#[tokio::test]
async fn trash_keeps_every_version_with_the_same_name() {
    let (v, dir, _t) = new_vault();
    let mut versions = std::collections::HashMap::new();
    for i in 0..100 {
        let content = format!("version {i}");
        v.create(&format!("batch/{i}/note.md"), &content)
            .await
            .unwrap();
        let p = v.trash(&format!("batch/{i}/note.md")).await.unwrap();
        assert!(
            versions.insert(p.clone(), content).is_none(),
            "trash destination {p} was reused"
        );
    }
    for (p, want) in versions {
        assert_eq!(fs::read_to_string(dir.join(&p)).unwrap(), want, "{p}");
    }
}

#[tokio::test]
async fn save_preserves_existing_permissions() {
    for mode in [0o600u32, 0o640, 0o660] {
        for force in [false, true] {
            let (v, dir, _t) = new_vault();
            let p = dir.join("a.md");
            fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
            let note = v.read("a.md").await.unwrap();
            let mtime = if force { 0 } else { note.mtime };
            v.save(&note.path, "edited", mtime).await.unwrap();
            let got = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(got, mode, "force={force}");
        }
    }
}

#[tokio::test]
async fn stale_save_does_not_resurrect_moved_or_trashed_notes() {
    for op in ["move", "trash", "move folder"] {
        let (v, dir, _t) = new_vault();
        let base = v.create("notes/draft.md", "original").await.unwrap();
        match op {
            "move" => drop(v.move_to(&base.path, "archive/draft.md").await.unwrap()),
            "trash" => drop(v.trash(&base.path).await.unwrap()),
            _ => drop(v.move_to("notes", "archive").await.unwrap()),
        }
        let err = v
            .save(&base.path, "stale autosave", base.mtime)
            .await
            .unwrap_err();
        assert!(err.is_not_exist(), "{op}: {err}");
        assert!(
            !dir.join(&base.path).exists(),
            "{op}: recreated the old path"
        );
    }
}

#[tokio::test]
async fn read_returns_per_byte_replacement_for_invalid_utf8() {
    let (v, dir, _t) = new_vault();
    fs::write(dir.join("latin1.md"), b"fun\xe7\xe3o").unwrap();
    let n = v.read("latin1.md").await.unwrap();
    assert_eq!(n.content, "fun\u{FFFD}\u{FFFD}o");
}
