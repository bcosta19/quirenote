package vault

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func newVault(t *testing.T) (*Vault, string) {
	t.Helper()
	dir := t.TempDir()
	if err := errors.Join(os.Mkdir(filepath.Join(dir, "livros"), 0o755), os.Mkdir(filepath.Join(dir, ".obsidian"), 0o755)); err != nil {
		t.Fatal(err)
	}
	for name, body := range map[string]string{
		"a.md":             "# A",
		"livros/b.pdf":     "%PDF",
		"livros/img.png":   "png",
		".obsidian/x.json": "{}",
	} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	v, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = v.Close() })
	return v, dir
}

func TestTreeHidesDotfilesAndListsKnownExtensions(t *testing.T) {
	v, _ := newVault(t)
	tree, err := v.Tree()
	if err != nil {
		t.Fatal(err)
	}
	if len(tree) != 2 || tree[0].Path != "livros" || tree[1].Path != "a.md" {
		t.Fatalf("árvore inesperada: %+v", tree)
	}
	if kids := tree[0].Children; len(kids) != 2 || kids[0].Path != "livros/b.pdf" || kids[1].Path != "livros/img.png" {
		t.Fatalf("filhos inesperados: %+v", kids)
	}
}

func TestRejectsPathsOutsideVault(t *testing.T) {
	v, _ := newVault(t)
	for _, p := range []string{"../etc/passwd", "/etc/passwd", "livros/../../x.md", ".obsidian/x.json", ""} {
		if _, err := v.Read(p); err == nil {
			t.Errorf("Read(%q) deveria falhar", p)
		}
		if _, err := v.Save(p, "x", 0); err == nil {
			t.Errorf("Save(%q) deveria falhar", p)
		}
	}
}

func TestRejectsSymlinkEscape(t *testing.T) {
	v, dir := newVault(t)
	outside := filepath.Join(t.TempDir(), "secret.md")
	if err := os.WriteFile(outside, []byte("segredo"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(dir, "link.md")); err != nil {
		t.Fatal(err)
	}
	if _, err := v.Read("link.md"); err == nil {
		t.Fatal("symlink para fora do vault deveria falhar")
	}
}

func TestSaveDetectsConflict(t *testing.T) {
	v, dir := newVault(t)
	note, err := v.Read("a.md")
	if err != nil {
		t.Fatal(err)
	}

	// Outro editor (ex.: Neovim) altera o arquivo.
	later := time.Now().Add(time.Second)
	if err := os.WriteFile(filepath.Join(dir, "a.md"), []byte("# Neovim"), 0o644); err != nil {
		t.Fatal(err)
	}
	_ = os.Chtimes(filepath.Join(dir, "a.md"), later, later)

	current, err := v.Save("a.md", "# Web", note.Mtime)
	if !errors.Is(err, ErrConflict) {
		t.Fatalf("esperava ErrConflict, veio %v", err)
	}
	if current.Content != "# Neovim" {
		t.Fatalf("conflito deveria devolver a versão do disco, veio %q", current.Content)
	}

	// Com o mtime atual, salva normalmente e sem deixar temporários.
	saved, err := v.Save("a.md", "# Web", current.Mtime)
	if err != nil {
		t.Fatal(err)
	}
	if saved.Content != "# Web" {
		t.Fatalf("conteúdo salvo errado: %q", saved.Content)
	}
	files, _ := filepath.Glob(filepath.Join(dir, ".*.tmp"))
	if len(files) != 0 {
		t.Fatalf("temporários sobraram: %v", files)
	}
}

func TestSaveCreatesDirsAndOnlyMarkdown(t *testing.T) {
	v, dir := newVault(t)
	if _, err := v.Save("novos/c.md", "oi", 0); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "novos/c.md")); err != nil {
		t.Fatal(err)
	}
	if _, err := v.Save("livros/b.pdf", "x", 0); !errors.Is(err, ErrNotNote) {
		t.Fatalf("esperava ErrNotNote, veio %v", err)
	}
}

func TestMkdir(t *testing.T) {
	v, dir := newVault(t)
	p, err := v.Mkdir("Estudos/Compiladores")
	if err != nil || p != "Estudos/Compiladores" {
		t.Fatalf("Mkdir = %q, %v", p, err)
	}
	if info, err := os.Stat(filepath.Join(dir, "Estudos/Compiladores")); err != nil || !info.IsDir() {
		t.Fatalf("pasta não criada: %v", err)
	}
	if _, err := v.Mkdir("Estudos/Compiladores"); err != nil {
		t.Fatalf("pasta existente não deveria falhar: %v", err)
	}
	for _, bad := range []string{"../fora", "a/../../fora", ".oculta", "a/.obsidian", ""} {
		if _, err := v.Mkdir(bad); err == nil {
			t.Errorf("Mkdir(%q) deveria falhar", bad)
		}
	}
	// Barra inicial é relativa ao vault (o caminho vem da URL), nunca à raiz do sistema.
	if p, err := v.Mkdir("/etc/x"); err != nil || p != "etc/x" {
		t.Fatalf("Mkdir(/etc/x) = %q, %v; esperava etc/x dentro do vault", p, err)
	}
	if _, err := os.Stat(filepath.Join(dir, "etc/x")); err != nil {
		t.Fatal(err)
	}
}

func TestCreateNeverOverwrites(t *testing.T) {
	v, dir := newVault(t)
	n, err := v.Create("novas/ideia.md", "# Ideia\n")
	if err != nil || n.Content != "# Ideia\n" {
		t.Fatalf("Create = %+v, %v", n, err)
	}
	if _, err := v.Create("a.md", "apagaria tudo"); !errors.Is(err, ErrExists) {
		t.Fatalf("esperava ErrExists, veio %v", err)
	}
	if b, _ := os.ReadFile(filepath.Join(dir, "a.md")); string(b) != "# A" {
		t.Fatalf("nota existente foi alterada: %q", b)
	}
	if _, err := v.Create("livros/b.pdf", "x"); !errors.Is(err, ErrNotNote) {
		t.Fatalf("esperava ErrNotNote, veio %v", err)
	}
	if _, err := v.Create("../fora.md", "x"); err == nil {
		t.Fatal("caminho fora do vault deveria falhar")
	}
}
