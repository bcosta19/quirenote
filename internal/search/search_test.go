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
		t.Skip("ripgrep não instalado")
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
		t.Fatalf("resultados inesperados: %+v", res)
	}
	for _, r := range res {
		hit := ""
		for _, p := range r.Parts {
			if p.Hit {
				hit = p.Text
			}
		}
		if hit != "FUNÇÃO" && hit != "funcao" && hit != "função" {
			t.Errorf("%s: trecho marcado %q", r.Path, hit)
		}
	}

	// Metacaracteres de regex e de shell são buscados literalmente.
	res, err = Search(context.Background(), dir, "[a-z]+ .*")
	if err != nil || len(res) != 1 || res[0].Path != "perigo.md" {
		t.Fatalf("busca literal falhou: %+v, %v", res, err)
	}
	// Nota antiga em Latin-1 (não UTF-8): o rg manda a linha em base64; não pode quebrar.
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
					t.Errorf("trecho com UTF-8 inválido: %q", p.Text)
				}
			}
		}
	}
	if !found {
		t.Fatalf("nota Latin-1 não encontrada: %+v", res)
	}

	// Extensão em maiúsculas também é nota.
	write("MAIUSC.MD", "função em arquivo .MD\n")
	if res, _ := Search(context.Background(), dir, "arquivo .MD"); len(res) != 1 {
		t.Fatalf(".MD não encontrado: %+v", res)
	}

	// Ocorrência maior que a janela do trecho não pode quebrar a busca.
	long := strings.Repeat("palavra", 80)
	write("longa.md", long+"\n")
	if res, err := Search(context.Background(), dir, long[:400]); err != nil || len(res) != 1 || len(res[0].Parts) == 0 {
		t.Fatalf("ocorrência longa: %+v, %v", res, err)
	}
	if res, _ := Search(context.Background(), dir, "   "); len(res) != 0 {
		t.Fatalf("consulta vazia deveria dar zero resultados: %+v", res)
	}
}
