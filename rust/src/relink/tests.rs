//! Relink contract and concurrency regressions; discovery does not require ripgrep.

use std::fs;

use super::*;

#[test]
fn rewrite_cases() {
    // (name, from, to, input, want, unique)
    let cases: &[(&str, &str, &str, &str, &str, bool)] = &[
        (
            "name",
            "Estudos/Lexer.md",
            "Estudos/Analisador léxico.md",
            "Ver [[Lexer]].",
            "Ver [[Analisador léxico]].",
            true,
        ),
        (
            "path",
            "Estudos/Lexer.md",
            "Arquivo/Lexer 2.md",
            "Ver [[Estudos/Lexer]].",
            "Ver [[Arquivo/Lexer 2]].",
            true,
        ),
        (
            "alias and section",
            "Lexer.md",
            "Tokens.md",
            "[[Lexer#Regras|regras]] e ![[lexer#^bloco]]",
            "[[Tokens#Regras|regras]] e ![[Tokens#^bloco]]",
            true,
        ),
        (
            "with .md",
            "a/Lexer.md",
            "a/Novo.md",
            "[[Lexer.md]]",
            "[[Novo.md]]",
            true,
        ),
        (
            "table",
            "Lexer.md",
            "Novo.md",
            "| [[Lexer\\|x]] |",
            "| [[Novo\\|x]] |",
            true,
        ),
        (
            "duplicate name becomes path",
            "a/Lexer.md",
            "b/Comum.md",
            "[[Lexer]]",
            "[[b/Comum]]",
            false,
        ),
        (
            "pdf with extension",
            "Livros/Dragon.pdf",
            "Livros/Dragon Book.pdf",
            "[[Dragon.pdf#page=12]] e [[Livros/Dragon.pdf]]",
            "[[Dragon Book.pdf#page=12]] e [[Livros/Dragon Book.pdf]]",
            true,
        ),
        (
            "leaves similar ones alone",
            "Lexer.md",
            "Novo.md",
            "[[Lexer avançado]] [Lexer] [[Lexer.pdf]] `Lexer`",
            "[[Lexer avançado]] [Lexer] [[Lexer.pdf]] `Lexer`",
            true,
        ),
        (
            "regex characters",
            "C++ (notas).md",
            "Cpp.md",
            "[[C++ (notas)]] e [[C (notas)]]",
            "[[Cpp]] e [[C (notas)]]",
            true,
        ),
    ];
    for (name, from, to, input, want, unique) in cases {
        let (got, _) = rewrite(input, from, to, *unique, true);
        assert_eq!(&got, want, "{name}");
    }
}

#[test]
fn rewrite_ambiguous_keeps_name_links() {
    // Another "Lexer" still exists: [[Lexer]] may point to it, so only the path changes.
    let (got, n) = rewrite(
        "[[Lexer]] e [[a/Lexer#x]]",
        "a/Lexer.md",
        "a/Tokens.md",
        true,
        false,
    );
    assert_eq!(got, "[[Lexer]] e [[a/Tokens#x]]");
    assert_eq!(n, 1);
}

fn new_vault(
    files: &[(String, String)],
) -> Option<(Arc<Vault>, std::path::PathBuf, tempfile::TempDir)> {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fs::canonicalize(tmp.path()).unwrap();
    for (name, body) in files {
        let p = dir.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    Some((Arc::new(Vault::open(&dir).unwrap()), dir, tmp))
}

fn f(files: &[(&str, &str)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[tokio::test]
async fn run_updates_every_referencing_note() {
    // The file was already renamed from Estudos/Lexer.md to Estudos/Novo nome.md.
    let mut files = f(&[
        (
            "Estudos/Novo nome.md",
            "# Lexer\nVer [[Lexer#Regras]] (link para si mesma)\n",
        ),
        ("a.md", "Ver [[Lexer]] e [[Lexer#Regras|regras]].\n"),
        ("b.md", "Por caminho: [[Estudos/Lexer]].\n"),
        ("c.md", "Sem link para ele.\n"),
    ]);
    // Many referencing notes: none may be left behind (the regular search has a limit).
    for i in 0..300 {
        files.push((format!("diario/{i:03}.md"), "hoje: [[Lexer]]\n".into()));
    }
    let Some((v, dir, _t)) = new_vault(&files) else {
        return;
    };
    let read = |n: &str| fs::read_to_string(dir.join(n)).unwrap();

    let plan = preview(&v, "Estudos/Lexer.md", "Estudos/Novo nome.md")
        .await
        .unwrap();
    assert_eq!(plan.files.len(), 303);
    assert!(!plan.ambiguous);
    assert_eq!(
        read("a.md"),
        "Ver [[Lexer]] e [[Lexer#Regras|regras]].\n",
        "preview must not write"
    );

    let res = run(&v, "Estudos/Lexer.md", "Estudos/Novo nome.md")
        .await
        .unwrap();
    assert_eq!((res.updated.len(), res.skipped.len()), (303, 0));
    assert_eq!(
        read("a.md"),
        "Ver [[Novo nome]] e [[Novo nome#Regras|regras]].\n"
    );
    assert_eq!(read("b.md"), "Por caminho: [[Estudos/Novo nome]].\n");
    assert!(read("Estudos/Novo nome.md").contains("[[Novo nome#Regras]]"));
    assert_eq!(read("diario/299.md"), "hoje: [[Novo nome]]\n");
    assert_eq!(read("c.md"), "Sem link para ele.\n");
}

#[tokio::test]
async fn run_ambiguous() {
    // a/Lexer.md became a/Tokens.md, but b/Lexer.md still exists.
    let Some((v, dir, _t)) = new_vault(&f(&[
        ("a/Tokens.md", "x"),
        ("b/Lexer.md", "y"),
        ("nome.md", "[[Lexer]]\n"),
        ("caminho.md", "[[a/Lexer]]\n"),
    ])) else {
        return;
    };
    let plan = preview(&v, "a/Lexer.md", "a/Tokens.md").await.unwrap();
    assert!(plan.ambiguous);
    assert_eq!(plan.files, ["caminho.md"]);
    run(&v, "a/Lexer.md", "a/Tokens.md").await.unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("nome.md")).unwrap(),
        "[[Lexer]]\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("caminho.md")).unwrap(),
        "[[a/Tokens]]\n"
    );
}

#[tokio::test]
async fn run_rejects() {
    let Some((v, dir, _t)) = new_vault(&f(&[
        ("novo.md", "x"),
        ("novo.pdf", "%PDF"),
        ("velho.md", "ainda existe"),
        ("cita.md", "[[velho]]\n"),
        ("C# notas.md", "z"),
    ])) else {
        return;
    };
    type Check = fn(&Error) -> bool;
    let cases: [(&str, &str, Check); 4] = [
        ("x.md", "C# notas.md", |e| {
            matches!(e, Error::Vault(vault::Error::BadName))
        }),
        ("x.md", "novo.pdf", |e| matches!(e, Error::TypeChanged)),
        ("velho.md", "novo.md", |e| matches!(e, Error::NotRenamed)),
        ("sumiu.md", "nao-existe.md", |e| {
            matches!(e, Error::NotRenamed)
        }),
    ];
    for (from, to, want) in cases {
        let err = run(&v, from, to).await.unwrap_err();
        assert!(want(&err), "run({from:?}, {to:?}) = {err}");
    }
    assert_eq!(
        fs::read_to_string(dir.join("cita.md")).unwrap(),
        "[[velho]]\n"
    );
}

#[tokio::test]
async fn run_case_only_rename() {
    // Lexer.md -> lexer.md: not ambiguous (the only "lexer" is the file itself).
    let Some((v, dir, _t)) = new_vault(&f(&[("lexer.md", "x"), ("cita.md", "[[Lexer]]\n")])) else {
        return;
    };
    let res = run(&v, "Lexer.md", "lexer.md").await.unwrap();
    assert!(!res.ambiguous);
    assert_eq!(res.updated.len(), 1);
    assert_eq!(
        fs::read_to_string(dir.join("cita.md")).unwrap(),
        "[[lexer]]\n"
    );
}

/// Relink must discover every visible note itself, even when no search subprocess
/// is available. A search over files being atomically replaced can miss a reference.
#[tokio::test]
async fn run_discovers_references_without_ripgrep() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("notes")).unwrap();
    fs::write(tmp.path().join("New.md"), "target").unwrap();
    fs::write(tmp.path().join("notes/ref.md"), "[[Old]]\n").unwrap();
    fs::write(tmp.path().join(".hidden.md"), "[[Old]]\n").unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("outside.md"), "[[Old]]\n").unwrap();
    std::os::unix::fs::symlink("notes/ref.md", tmp.path().join("link-in.md")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("outside.md"),
        tmp.path().join("link-out.md"),
    )
    .unwrap();
    let v = Arc::new(Vault::open(tmp.path()).unwrap());
    let result = run(&v, "Old.md", "New.md").await.unwrap();
    assert_eq!(result.updated, ["notes/ref.md"]);
    assert!(result.skipped.is_empty());
    assert_eq!(
        fs::read_to_string(outside.path().join("outside.md")).unwrap(),
        "[[Old]]\n"
    );
    assert_eq!(v.read("notes/ref.md").await.unwrap().content, "[[New]]\n");
    assert_eq!(
        fs::read_to_string(tmp.path().join(".hidden.md")).unwrap(),
        "[[Old]]\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_accounts_for_all_references_during_atomic_saves() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("New.md"), "target").unwrap();
    let count = 200;
    for i in 0..count {
        fs::write(tmp.path().join(format!("ref{i}.md")), "[[Old]]\n").unwrap();
    }
    let v = Arc::new(Vault::open(tmp.path()).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let mut editors = Vec::new();
    let started = Arc::new(tokio::sync::Barrier::new(17));
    for worker in 0..16 {
        let (v, stop, started) = (v.clone(), stop.clone(), started.clone());
        editors.push(tokio::spawn(async move {
            let mut accepted = Vec::new();
            let mut n = 0;
            started.wait().await;
            while !stop.load(Ordering::Relaxed) {
                let path = format!("ref{}.md", (worker + n * 16) % count);
                let note = v.read(&path).await.unwrap();
                let token = format!("edit-{worker}-{n}\n");
                match v.save(&path, &(note.content + &token), note.mtime).await {
                    Ok(_) => accepted.push((path, token)),
                    Err(vault::Error::Conflict(_)) => {}
                    Err(e) => panic!("save: {e}"),
                }
                n += 1;
            }
            accepted
        }));
    }
    started.wait().await;
    // Every editor completes work while relink discovers and reads the notes.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let result = run(&v, "Old.md", "New.md").await.unwrap();
    stop.store(true, Ordering::Relaxed);
    let mut accepted_count = 0;
    for editor in editors {
        for (path, token) in editor.await.unwrap() {
            accepted_count += 1;
            assert!(
                v.read(&path).await.unwrap().content.contains(&token),
                "accepted edit lost: {token}"
            );
        }
    }
    assert!(
        accepted_count >= 16,
        "editors did not exercise atomic saves"
    );
    assert_eq!(result.updated.len() + result.skipped.len(), count);
    for i in 0..count {
        let path = format!("ref{i}.md");
        let note = v.read(&path).await.unwrap();
        assert!(
            !note.content.contains("[[Old]]") || result.skipped.contains(&path),
            "stale reference was neither updated nor reported: {path}"
        );
    }
}
