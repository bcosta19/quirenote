package vault

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
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

func TestMove(t *testing.T) {
	v, dir := newVault(t)
	if p, err := v.Move("a.md", "pasta nova/b.md"); err != nil || p != "pasta nova/b.md" {
		t.Fatalf("Move = %q, %v", p, err)
	}
	if b, err := os.ReadFile(filepath.Join(dir, "pasta nova/b.md")); err != nil || string(b) != "# A" {
		t.Fatalf("conteúdo após mover: %q, %v", b, err)
	}
	// Nunca sobrescreve.
	if _, err := v.Move("livros/b.pdf", "pasta nova/b.md"); !errors.Is(err, ErrExists) {
		t.Fatalf("esperava ErrExists, veio %v", err)
	}
	// Pasta inteira, e não para dentro dela mesma.
	if _, err := v.Move("livros", "livros/sub"); !errors.Is(err, ErrInvalidPath) {
		t.Fatalf("mover pasta para dentro dela mesma: %v", err)
	}
	if _, err := v.Move("livros", "biblioteca"); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "biblioteca/b.pdf")); err != nil {
		t.Fatal(err)
	}
	for _, bad := range [][2]string{{"../x", "y.md"}, {"biblioteca/b.pdf", "../fora.pdf"}, {".obsidian/x.json", "x.json"}, {"biblioteca", ".trash/b"}} {
		if _, err := v.Move(bad[0], bad[1]); err == nil {
			t.Errorf("Move(%q, %q) deveria falhar", bad[0], bad[1])
		}
	}
	if _, err := v.Move("nao-existe.md", "x.md"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("origem inexistente: %v", err)
	}
}

func TestTrash(t *testing.T) {
	v, dir := newVault(t)
	p1, err := v.Trash("a.md")
	if err != nil || p1 != ".trash/a.md" {
		t.Fatalf("Trash = %q, %v", p1, err)
	}
	if _, err := os.Stat(filepath.Join(dir, "a.md")); !os.IsNotExist(err) {
		t.Fatal("nota deveria ter saído do lugar")
	}
	// Mesmo nome de novo: não sobrescreve o que já está na lixeira.
	if err := os.WriteFile(filepath.Join(dir, "a.md"), []byte("segunda"), 0o644); err != nil {
		t.Fatal(err)
	}
	p2, err := v.Trash("a.md")
	if err != nil || p2 == p1 || !strings.HasPrefix(p2, ".trash/a ") || !strings.HasSuffix(p2, ".md") {
		t.Fatalf("segunda ida à lixeira = %q, %v", p2, err)
	}
	if b, _ := os.ReadFile(filepath.Join(dir, p1)); string(b) != "# A" {
		t.Fatalf("primeira versão na lixeira foi alterada: %q", b)
	}
	// Pastas também vão inteiras; a lixeira em si não pode ser enviada.
	if _, err := v.Trash("livros"); err != nil {
		t.Fatal(err)
	}
	if _, err := v.Trash(".trash"); err == nil {
		t.Fatal("a própria lixeira não pode ir para a lixeira")
	}
	if tree, _ := v.Tree(); len(tree) != 0 {
		t.Fatalf("lixeira não deveria aparecer na árvore: %+v", tree)
	}
}
