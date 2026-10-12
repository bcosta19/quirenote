// Package setup prepares the vault on first run: a new empty folder or an existing
// folder of markdown notes (an Obsidian vault works as is, nothing is copied).
package setup

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

type Mode string

const (
	New      Mode = "new"      // create a new folder with a welcome note
	Existing Mode = "existing" // use a folder that already has notes
)

type Result struct {
	Dir      string `json:"dir"`
	Obsidian bool   `json:"obsidian"` // the folder is an Obsidian vault (.obsidian/ exists)
}

// DefaultPath is the folder suggested for a fresh start.
const DefaultPath = "~/Quirenote"

// Resolve expands "~", requires an absolute path and refuses folders that would expose
// too much: the filesystem root, the home folder itself and its parents.
func Resolve(path, home string) (string, error) {
	path = strings.TrimSpace(path)
	if path == "~" || strings.HasPrefix(path, "~/") {
		path = filepath.Join(home, strings.TrimPrefix(path, "~"))
	}
	if !filepath.IsAbs(path) {
		return "", errors.New("use an absolute path (or one starting with ~/)")
	}
	dir, err := canonical(filepath.Clean(path))
	if err != nil {
		return "", err
	}
	home, err = canonical(filepath.Clean(home))
	if err != nil {
		return "", err
	}
	if dir == "/" || dir == home || strings.HasPrefix(home, dir+"/") {
		return "", fmt.Errorf("%s is too broad for a vault; pick a folder inside it", dir)
	}
	return dir, nil
}

func canonical(dir string) (string, error) {
	resolved, err := filepath.EvalSymlinks(dir)
	if err == nil || !errors.Is(err, fs.ErrNotExist) {
		return resolved, err
	}
	parent := filepath.Dir(dir)
	if parent == dir {
		return "", err
	}
	resolved, err = canonical(parent)
	if err != nil {
		return "", err
	}
	return filepath.Join(resolved, filepath.Base(dir)), nil
}

// Prepare validates (and for New, creates) the vault folder. It does not save the config.
func Prepare(mode Mode, path, home string) (Result, error) {
	dir, err := Resolve(path, home)
	if err != nil {
		return Result{}, err
	}
	switch mode {
	case New:
		return Result{Dir: dir}, create(dir)
	case Existing:
		info, err := os.Stat(dir)
		if err != nil {
			return Result{}, fmt.Errorf("folder not found: %s", dir)
		}
		if !info.IsDir() {
			return Result{}, fmt.Errorf("not a folder: %s", dir)
		}
		if err := writable(dir); err != nil {
			return Result{}, err
		}
		obsidian := false
		if o, err := os.Stat(filepath.Join(dir, ".obsidian")); err == nil {
			obsidian = o.IsDir()
		}
		return Result{Dir: dir, Obsidian: obsidian}, nil
	default:
		return Result{}, fmt.Errorf("unknown mode %q", mode)
	}
}

// Auto picks the mode for the CLI: an existing folder is used as is, a missing one is created.
func Auto(path, home string) (Result, error) {
	dir, err := Resolve(path, home)
	if err != nil {
		return Result{}, err
	}
	if _, err := os.Stat(dir); err == nil {
		return Prepare(Existing, dir, home)
	}
	return Prepare(New, dir, home)
}

// create makes the folder (it may exist, but must be empty) and adds the welcome note.
func create(dir string) error {
	if items, err := os.ReadDir(dir); err == nil && len(items) > 0 {
		return fmt.Errorf("%s is not empty; to keep its notes, choose \"use an existing folder\"", dir)
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, "Welcome.md"), []byte(welcome), 0o644)
}

func writable(dir string) error {
	f, err := os.CreateTemp(dir, ".quirenote-check-*")
	if err != nil {
		return fmt.Errorf("cannot write to %s: %w", dir, err)
	}
	_ = f.Close()
	return os.Remove(f.Name())
}

const welcome = `# Welcome to Quirenote

Your notes are plain markdown files in this folder: you can also edit them in Neovim,
Obsidian or any text editor, and Quirenote picks up the changes.

## Getting started

- Create a note with the **new note** button in the sidebar, or press ` + "`Ctrl+O`" + `,
  type a name and press ` + "`Shift+Enter`" + `.
- Link notes with ` + "`[[Note name]]`" + `; ` + "`Ctrl+click`" + ` follows a link.
- Open a PDF or EPUB from the sidebar to read it next to your notes, and press
  ` + "`Ctrl+Shift+L`" + ` to insert a link to the current page.
- ` + "`Ctrl+E`" + ` toggles the preview; ` + "`Ctrl+Shift+F`" + ` searches all notes.

- [ ] Create your first note
- [ ] Open a PDF and link to a page
`
