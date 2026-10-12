package relink

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"notas/internal/vault"
)

func TestRewrite(t *testing.T) {
	cases := []struct {
		name, from, to, in, want string
		unique                   bool
	}{
		{"nome", "Estudos/Lexer.md", "Estudos/Analisador léxico.md", "Ver [[Lexer]].", "Ver [[Analisador léxico]].", true},
		{"caminho", "Estudos/Lexer.md", "Arquivo/Lexer 2.md", "Ver [[Estudos/Lexer]].", "Ver [[Arquivo/Lexer 2]].", true},
		{"apelido e seção", "Lexer.md", "Tokens.md", "[[Lexer#Regras|regras]] e ![[lexer#^bloco]]", "[[Tokens#Regras|regras]] e ![[Tokens#^bloco]]", true},
		{"com .md", "a/Lexer.md", "a/Novo.md", "[[Lexer.md]]", "[[Novo.md]]", true},
		{"tabela", "Lexer.md", "Novo.md", "| [[Lexer\\|x]] |", "| [[Novo\\|x]] |", true},
		{"nome repetido vira caminho", "a/Lexer.md", "b/Comum.md", "[[Lexer]]", "[[b/Comum]]", false},
		{"pdf com extensão", "Livros/Dragon.pdf", "Livros/Dragon Book.pdf", "[[Dragon.pdf#page=12]] e [[Livros/Dragon.pdf]]", "[[Dragon Book.pdf#page=12]] e [[Livros/Dragon Book.pdf]]", true},
		{"não mexe em parecidos", "Lexer.md", "Novo.md", "[[Lexer avançado]] [Lexer] [[Lexer.pdf]] `Lexer`", "[[Lexer avançado]] [Lexer] [[Lexer.pdf]] `Lexer`", true},
		{"caracteres de regex", "C++ (notas).md", "Cpp.md", "[[C++ (notas)]] e [[C (notas)]]", "[[Cpp]] e [[C (notas)]]", true},
	}
	for _, c := range cases {
		got, _ := Rewrite(c.in, c.from, c.to, c.unique, true)
		if got != c.want {
			t.Errorf("%s:\n got %q\nwant %q", c.name, got, c.want)
		}
	}
}

func TestRewriteAmbiguousKeepsNameLinks(t *testing.T) {
	// Ainda existe outro "Lexer": [[Lexer]] pode ser dele, então só o caminho muda.
	got, n := Rewrite("[[Lexer]] e [[a/Lexer#x]]", "a/Lexer.md", "a/Tokens.md", true, false)
	if got != "[[Lexer]] e [[a/Tokens#x]]" || n != 1 {
		t.Fatalf("got %q (%d)", got, n)
	}
}

func newVault(t *testing.T, files map[string]string) (*vault.Vault, string) {
	t.Helper()
	if _, err := exec.LookPath("rg"); err != nil {
		t.Skip("ripgrep não instalado")
	}
	dir := t.TempDir()
	for name, body := range files {
		p := filepath.Join(dir, name)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = v.Close() })
	return v, dir
}

func read(t *testing.T, dir, name string) string {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(dir, name))
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestRun(t *testing.T) {
	// O arquivo já foi renomeado de Estudos/Lexer.md para Estudos/Novo nome.md.
	files := map[string]string{
		"Estudos/Novo nome.md": "# Lexer\nVer [[Lexer#Regras]] (link para si mesma)\n",
		"a.md":                 "Ver [[Lexer]] e [[Lexer#Regras|regras]].\n",
		"b.md":                 "Por caminho: [[Estudos/Lexer]].\n",
		"c.md":                 "Sem link para ele.\n",
	}
	// Muitas citantes: nenhuma pode ficar para trás (a busca comum tem limite).
	for i := 0; i < 300; i++ {
		files[fmt.Sprintf("diario/%03d.md", i)] = "hoje: [[Lexer]]\n"
	}
	v, dir := newVault(t, files)

	plan, err := Preview(context.Background(), v, "Estudos/Lexer.md", "Estudos/Novo nome.md")
	if err != nil || len(plan.Files) != 303 || plan.Ambiguous {
		t.Fatalf("Preview = %d arquivos, ambíguo=%v, %v", len(plan.Files), plan.Ambiguous, err)
	}
	if read(t, dir, "a.md") != files["a.md"] {
		t.Fatal("Preview não pode gravar nada")
	}
	res, err := Run(context.Background(), v, "Estudos/Lexer.md", "Estudos/Novo nome.md")
	if err != nil || len(res.Updated) != 303 || len(res.Skipped) != 0 {
		t.Fatalf("Run = %d atualizadas, %d puladas, %v", len(res.Updated), len(res.Skipped), err)
	}
	if got := read(t, dir, "a.md"); got != "Ver [[Novo nome]] e [[Novo nome#Regras|regras]].\n" {
		t.Fatalf("a.md = %q", got)
	}
	if got := read(t, dir, "b.md"); got != "Por caminho: [[Estudos/Novo nome]].\n" {
		t.Fatalf("b.md = %q", got)
	}
	if got := read(t, dir, "Estudos/Novo nome.md"); !strings.Contains(got, "[[Novo nome#Regras]]") {
		t.Fatalf("links para si mesma não atualizados: %q", got)
	}
	if got := read(t, dir, "diario/299.md"); got != "hoje: [[Novo nome]]\n" {
		t.Fatalf("última citante = %q", got)
	}
	if read(t, dir, "c.md") != files["c.md"] {
		t.Fatal("nota sem link foi alterada")
	}
}

func TestRunAmbiguous(t *testing.T) {
	// a/Lexer.md virou a/Tokens.md, mas b/Lexer.md continua existindo.
	v, dir := newVault(t, map[string]string{
		"a/Tokens.md": "x", "b/Lexer.md": "y",
		"nome.md":    "[[Lexer]]\n",
		"caminho.md": "[[a/Lexer]]\n",
	})
	plan, err := Preview(context.Background(), v, "a/Lexer.md", "a/Tokens.md")
	if err != nil || !plan.Ambiguous || len(plan.Files) != 1 || plan.Files[0] != "caminho.md" {
		t.Fatalf("Preview = %+v, %v", plan, err)
	}
	if _, err := Run(context.Background(), v, "a/Lexer.md", "a/Tokens.md"); err != nil {
		t.Fatal(err)
	}
	if read(t, dir, "nome.md") != "[[Lexer]]\n" || read(t, dir, "caminho.md") != "[[a/Tokens]]\n" {
		t.Fatalf("nome.md=%q caminho.md=%q", read(t, dir, "nome.md"), read(t, dir, "caminho.md"))
	}
}

func TestRunRejects(t *testing.T) {
	v, dir := newVault(t, map[string]string{"novo.md": "x", "novo.pdf": "%PDF", "velho.md": "ainda existe", "cita.md": "[[velho]]\n", "C# notas.md": "z"})
	cases := []struct {
		from, to string
		want     error
	}{
		{"x.md", "C# notas.md", vault.ErrBadName},    // quebraria os links
		{"x.md", "novo.pdf", ErrTypeChanged},         // nota virou PDF
		{"velho.md", "novo.md", ErrNotRenamed},       // origem ainda existe
		{"sumiu.md", "nao-existe.md", ErrNotRenamed}, // destino não existe
	}
	for _, c := range cases {
		if _, err := Run(context.Background(), v, c.from, c.to); !errors.Is(err, c.want) {
			t.Errorf("Run(%q, %q) = %v, esperava %v", c.from, c.to, err, c.want)
		}
	}
	if read(t, dir, "cita.md") != "[[velho]]\n" {
		t.Fatal("pedido recusado não pode alterar notas")
	}
}

func TestRunCaseOnlyRename(t *testing.T) {
	// Lexer.md → lexer.md: não é ambíguo (o único "lexer" é o próprio arquivo).
	v, dir := newVault(t, map[string]string{"lexer.md": "x", "cita.md": "[[Lexer]]\n"})
	res, err := Run(context.Background(), v, "Lexer.md", "lexer.md")
	if err != nil || res.Ambiguous || len(res.Updated) != 1 || read(t, dir, "cita.md") != "[[lexer]]\n" {
		t.Fatalf("Run = %+v, %v, cita=%q", res, err, read(t, dir, "cita.md"))
	}
}
