package vault

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestConcurrentSavesRejectStaleVersions(t *testing.T) {
	v, dir := newVault(t)
	old := time.Now().Add(-time.Hour)
	if err := os.Chtimes(filepath.Join(dir, "a.md"), old, old); err != nil {
		t.Fatal(err)
	}
	base, err := v.Read("a.md")
	if err != nil {
		t.Fatal(err)
	}
	const clients = 16
	start := make(chan struct{})
	results := make(chan error, clients)
	for i := range clients {
		go func() {
			<-start
			_, err := v.Save("a.md", fmt.Sprintf("client %d\n", i)+strings.Repeat("x", 1<<20), base.Mtime)
			results <- err
		}()
	}
	close(start)
	saved := 0
	for range clients {
		err := <-results
		if err == nil {
			saved++
		} else if !errors.Is(err, ErrConflict) {
			t.Fatalf("unexpected save error: %v", err)
		}
	}
	if saved != 1 {
		t.Fatalf("%d clients overwrote the same version; want exactly one successful save", saved)
	}
}

func TestStaleSaveDoesNotResurrectMovedOrTrashedNotes(t *testing.T) {
	for _, operation := range []string{"move", "trash", "move folder"} {
		t.Run(operation, func(t *testing.T) {
			v, dir := newVault(t)
			base, err := v.Create("notes/draft.md", "original")
			if err != nil {
				t.Fatal(err)
			}
			switch operation {
			case "move":
				_, err = v.Move(base.Path, "archive/draft.md")
			case "trash":
				_, err = v.Trash(base.Path)
			case "move folder":
				_, err = v.Move("notes", "archive")
			}
			if err != nil {
				t.Fatal(err)
			}
			if _, err := v.Save(base.Path, "stale autosave", base.Mtime); !os.IsNotExist(err) {
				t.Fatalf("stale Save = %v; want a missing-file error", err)
			}
			if _, err := os.Stat(filepath.Join(dir, base.Path)); !os.IsNotExist(err) {
				t.Fatalf("the stale save recreated the old path: %v", err)
			}
		})
	}
}
