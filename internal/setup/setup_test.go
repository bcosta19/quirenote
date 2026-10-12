package setup

import (
	"os"
	"path/filepath"
	"testing"
)

func TestResolve(t *testing.T) {
	home := "/home/u"
	ok := map[string]string{"~/Notes": "/home/u/Notes", "/data/notes/": "/data/notes", " ~/a/../b ": "/home/u/b"}
	for in, want := range ok {
		if got, err := Resolve(in, home); err != nil || got != want {
			t.Errorf("Resolve(%q) = %q, %v; want %q", in, got, err, want)
		}
	}
	for _, bad := range []string{"/", "~", "/home", "/home/u/", "notes", "../x", ""} {
		if got, err := Resolve(bad, home); err == nil {
			t.Errorf("Resolve(%q) = %q; should fail", bad, got)
		}
	}
}

func TestPrepare(t *testing.T) {
	home := t.TempDir()
	// New: creates the folder with the welcome note.
	r, err := Prepare(New, "~/Quirenote", home)
	if err != nil || r.Dir != filepath.Join(home, "Quirenote") || r.Obsidian {
		t.Fatalf("New = %+v, %v", r, err)
	}
	if _, err := os.Stat(filepath.Join(r.Dir, "Welcome.md")); err != nil {
		t.Fatal(err)
	}
	// New over a folder with notes: refused, nothing is touched.
	if _, err := Prepare(New, r.Dir, home); err == nil {
		t.Fatal("New over a non-empty folder should fail")
	}
	// Existing: detects an Obsidian vault and leaves no files behind.
	vault := filepath.Join(home, "Vault")
	if err := os.MkdirAll(filepath.Join(vault, ".obsidian"), 0o755); err != nil {
		t.Fatal(err)
	}
	r, err = Prepare(Existing, vault, home)
	if err != nil || !r.Obsidian {
		t.Fatalf("Existing = %+v, %v", r, err)
	}
	if items, _ := os.ReadDir(vault); len(items) != 1 {
		t.Fatalf("Existing left files behind: %v", items)
	}
	if _, err := Prepare(Existing, filepath.Join(home, "missing"), home); err == nil {
		t.Fatal("Existing on a missing folder should fail")
	}
	// Auto: existing folder is used as is, missing one is created.
	if r, err := Auto(vault, home); err != nil || !r.Obsidian {
		t.Fatalf("Auto(existing) = %+v, %v", r, err)
	}
	if r, err := Auto("~/Fresh", home); err != nil || r.Obsidian {
		t.Fatalf("Auto(new) = %+v, %v", r, err)
	}
}
