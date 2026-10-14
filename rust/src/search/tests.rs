//! Ports of internal/search/search_test.go (they need `rg` and are skipped without it).

use std::fs;
use std::path::Path;

use super::*;

fn have_rg() -> bool {
    look_path("rg").is_some()
}

fn write(dir: &Path, name: &str, body: impl AsRef<[u8]>) {
    let p = dir.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn pattern_is_literal_and_accent_insensitive() {
    assert_eq!(pattern("a.b"), "[aáàâãäAÁÀÂÃÄ]\\.b");
    assert!(pattern("função").contains("[cçCÇ]"));
    assert_eq!(
        backlink_pattern("Estudos/Lexer.md"),
        r"\[\[(?:Lexer|Estudos/Lexer)(?:\.md)?(?:\\?[#|][^\]]*)?\]\]"
    );
}

#[tokio::test]
async fn search_finds_notes() {
    if !have_rg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "estudos/compiladores.md",
        "# Compiladores\n\nA FUNÇÃO do lexer é gerar tokens.\n",
    );
    write(dir, "outra.md", "nada aqui\nfuncao sem acento\n");
    write(dir, ".obsidian/cache.md", "função escondida\n");
    write(dir, "livro.pdf", "função num pdf\n");
    write(dir, "perigo.md", "$(rm -rf /) ; `x` [a-z]+ .*\n");
    write(dir, ".gitignore", "ignorada.md\n");
    write(dir, "ignorada.md", "função listada no gitignore\n");

    let res = search(dir, "funcao").await.unwrap();
    let got: std::collections::HashMap<_, _> =
        res.iter().map(|r| (r.path.as_str(), r.line)).collect();
    assert_eq!(got.len(), 3, "{res:?}");
    assert_eq!(got["estudos/compiladores.md"], 3);
    assert_eq!(got["outra.md"], 2);
    assert_eq!(got["ignorada.md"], 1);
    for r in &res {
        let hit = r
            .parts
            .iter()
            .find(|p| p.hit)
            .map(|p| p.text.as_str())
            .unwrap_or("");
        assert!(
            ["FUNÇÃO", "funcao", "função"].contains(&hit),
            "{}: {hit:?}",
            r.path
        );
    }

    // Regex and shell metacharacters are searched literally.
    let res = search(dir, "[a-z]+ .*").await.unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].path, "perigo.md");

    // Old Latin-1 note (not UTF-8): rg sends the line in base64; it must not break.
    let mut latin = "x".repeat(100).into_bytes();
    latin.extend_from_slice(b" fun\xe7\xe3o e funcao\n");
    write(dir, "latin1.md", latin);
    let res = search(dir, "funcao").await.unwrap();
    assert!(
        res.iter().any(|r| r.path == "latin1.md"),
        "Latin-1 note not found"
    );

    // An uppercase extension is a note too.
    write(dir, "MAIUSC.MD", "função em arquivo .MD\n");
    assert_eq!(search(dir, "arquivo .MD").await.unwrap().len(), 1);

    // A match longer than the snippet window must not break the search.
    let long = "palavra".repeat(80);
    write(dir, "longa.md", format!("{long}\n"));
    let res = search(dir, &long[..400]).await.unwrap();
    assert_eq!(res.len(), 1);
    assert!(!res[0].parts.is_empty());
    assert!(search(dir, "   ").await.unwrap().is_empty());
}

#[tokio::test]
async fn files_preserves_newlines_in_imported_note_names() {
    if !have_rg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let name = "imported\nreference.md";
    write(tmp.path(), name, "[[Old note]]\n");
    let files = files(tmp.path(), &backlink_pattern("Old note.md"))
        .await
        .unwrap();
    assert_eq!(files, [name]);
}

#[tokio::test]
async fn backlinks_cases() {
    if !have_rg() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "Estudos/Análise léxica.md",
        "# Análise léxica\nVer [[Análise léxica#Tokens]] (auto)\n",
    );
    write(dir, "a.md", "Leia [[Análise léxica]] hoje.\n");
    write(
        dir,
        "b.md",
        "Com apelido: [[Estudos/Análise léxica|o lexer]] e seção [[análise léxica#x]].\n",
    );
    write(dir, "c.md", "Com extensão [[Análise léxica.md]].\n");
    write(
        dir,
        "tabela.md",
        "| col |\n|---|\n| [[Análise léxica\\|apelido]] |\n",
    );
    write(
        dir,
        "falsos.md",
        "[[Análise léxica avançada]] e [[Outra]] e Análise léxica sem link [Análise léxica]\n",
    );
    write(dir, "regex.md", "[[a.b]] não pode casar com [[aXb]]\n");

    let res = backlinks(dir, "Estudos/Análise léxica.md").await.unwrap();
    let got: std::collections::BTreeSet<_> = res.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        got,
        ["a.md", "b.md", "c.md", "tabela.md"].into_iter().collect(),
        "{res:?}"
    );
    // A dot in the name is literal (escaped regex).
    let res = backlinks(dir, "a.b.md").await.unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].path, "regex.md");
    assert!(backlinks(dir, "../x.md").await.unwrap().is_empty());
}
