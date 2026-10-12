package search

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"unicode/utf8"
)

func TestSearch(t *testing.T) {
	if _, err := exec.LookPath("rg"); err != nil {
		t.Skip("ripgrep not installed")
	}
	dir := t.TempDir()
	write := func(name, body string) {
		t.Helper()
		p := filepath.Join(dir, name)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("estudos/compiladores.md", "# Compiladores\n\nA FUNÇÃO do lexer é gerar tokens.\n")
	write("outra.md", "nada aqui\nfuncao sem acento\n")
	write(".obsidian/cache.md", "função escondida\n")
	write("livro.pdf", "função num pdf\n")
	write("perigo.md", "$(rm -rf /) ; `x` [a-z]+ .*\n")
	write(".gitignore", "ignorada.md\n")
	write("ignorada.md", "função listada no gitignore\n")

	res, err := Search(context.Background(), dir, "funcao")
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]int{}
	for _, r := range res {
		got[r.Path] = r.Line
	}
	if len(got) != 3 || got["estudos/compiladores.md"] != 3 || got["outra.md"] != 2 || got["ignorada.md"] != 1 {
		t.Fatalf("unexpected results: %+v", res)
	}
	for _, r := range res {
		hit := ""
		for _, p := range r.Parts {
			if p.Hit {
				hit = p.Text
			}
		}
		if hit != "FUNÇÃO" && hit != "funcao" && hit != "função" {
			t.Errorf("%s: highlighted snippet %q", r.Path, hit)
		}
	}

	// Regex and shell metacharacters are searched literally.
	res, err = Search(context.Background(), dir, "[a-z]+ .*")
	if err != nil || len(res) != 1 || res[0].Path != "perigo.md" {
		t.Fatalf("literal search failed: %+v, %v", res, err)
	}
	// Old Latin-1 note (not UTF-8): rg sends the line in base64; it must not break.
	write("latin1.md", strings.Repeat("x", 100)+" fun\xe7\xe3o e funcao\n")
	res, err = Search(context.Background(), dir, "funcao")
	if err != nil {
		t.Fatal(err)
	}
	found := false
	for _, r := range res {
		if r.Path == "latin1.md" {
			found = true
			for _, p := range r.Parts {
				if !utf8.ValidString(p.Text) {
					t.Errorf("snippet with invalid UTF-8: %q", p.Text)
				}
			}
		}
	}
	if !found {
		t.Fatalf("Latin-1 note not found: %+v", res)
	}

	// An uppercase extension is a note too.
	write("MAIUSC.MD", "função em arquivo .MD\n")
	if res, _ := Search(context.Background(), dir, "arquivo .MD"); len(res) != 1 {
		t.Fatalf(".MD not found: %+v", res)
	}

	// A match longer than the snippet window must not break the search.
	long := strings.Repeat("palavra", 80)
	write("longa.md", long+"\n")
	if res, err := Search(context.Background(), dir, long[:400]); err != nil || len(res) != 1 || len(res[0].Parts) == 0 {
		t.Fatalf("long match: %+v, %v", res, err)
	}
	if res, _ := Search(context.Background(), dir, "   "); len(res) != 0 {
		t.Fatalf("an empty query should return no results: %+v", res)
	}
}

func TestFilesPreservesNewlinesInImportedNoteNames(t *testing.T) {
	if _, err := exec.LookPath("rg"); err != nil {
		t.Skip("ripgrep not installed")
	}
	dir := t.TempDir()
	name := "imported\nreference.md"
	if err := os.WriteFile(filepath.Join(dir, name), []byte("[[Old note]]\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	files, err := Files(context.Background(), dir, BacklinkPattern("Old note.md"))
	if err != nil {
		t.Fatal(err)
	}
	if len(files) != 1 || files[0] != name {
		t.Fatalf("Files = %q; want the intact filename %q", files, name)
	}
}

func TestBacklinks(t *testing.T) {
	if _, err := exec.LookPath("rg"); err != nil {
		t.Skip("ripgrep not installed")
	}
	dir := t.TempDir()
	write := func(name, body string) {
		t.Helper()
		p := filepath.Join(dir, name)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("Estudos/Análise léxica.md", "# Análise léxica\nVer [[Análise léxica#Tokens]] (auto)\n")
	write("a.md", "Leia [[Análise léxica]] hoje.\n")
	write("b.md", "Com apelido: [[Estudos/Análise léxica|o lexer]] e seção [[análise léxica#x]].\n")
	write("c.md", "Com extensão [[Análise léxica.md]].\n")
	write("tabela.md", "| col |\n|---|\n| [[Análise léxica\\|apelido]] |\n")
	write("falsos.md", "[[Análise léxica avançada]] e [[Outra]] e Análise léxica sem link [Análise léxica]\n")
	write("regex.md", "[[a.b]] não pode casar com [[aXb]]\n")

	res, err := Backlinks(context.Background(), dir, "Estudos/Análise léxica.md")
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]bool{}
	for _, r := range res {
		got[r.Path] = true
	}
	if len(got) != 4 || !got["a.md"] || !got["b.md"] || !got["c.md"] || !got["tabela.md"] {
		t.Fatalf("unexpected backlinks: %+v", res)
	}
	// A dot in the name is literal (escaped regex).
	if res, _ := Backlinks(context.Background(), dir, "a.b.md"); len(res) != 1 || res[0].Path != "regex.md" {
		t.Fatalf("name with a dot: %+v", res)
	}
}
