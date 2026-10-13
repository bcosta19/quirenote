package vault

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestReadKeepsContentAndMtimeTogetherDuringReplacement(t *testing.T) {
	v, dir := newVault(t)
	const base = int64(1_700_000_000_000_000_000)
	writeVersion := func(version int) error {
		mtime := base + int64(version)*int64(time.Second)
		tmp := filepath.Join(dir, ".replacement.tmp")
		if err := os.WriteFile(tmp, []byte(strconv.FormatInt(mtime, 10)), 0o644); err != nil {
			return err
		}
		stamp := time.Unix(0, mtime)
		if err := os.Chtimes(tmp, stamp, stamp); err != nil {
			return err
		}
		return os.Rename(tmp, filepath.Join(dir, "a.md"))
	}
	if err := writeVersion(0); err != nil {
		t.Fatal(err)
	}

	const readers = 8
	start := make(chan struct{})
	results := make(chan error, readers+1)
	go func() {
		<-start
		for version := 1; version <= 300; version++ {
			if err := writeVersion(version); err != nil {
				results <- err
				return
			}
		}
		results <- nil
	}()
	for range readers {
		go func() {
			<-start
			for range 300 {
				note, err := v.Read("a.md")
				if err != nil {
					results <- err
					return
				}
				want, err := strconv.ParseInt(note.Content, 10, 64)
				if err != nil || note.Mtime != want {
					results <- fmt.Errorf("content version %q paired with mtime %d: %v", note.Content, note.Mtime, err)
					return
				}
			}
			results <- nil
		}()
	}
	close(start)
	for range readers + 1 {
		if err := <-results; err != nil {
			t.Error(err)
		}
	}
}

func TestCreateRacingMoveNeverOverwrites(t *testing.T) {
	for _, external := range []bool{false, true} {
		t.Run(fmt.Sprintf("external=%v", external), func(t *testing.T) {
			if external && runtime.GOOS != "linux" {
				t.Skip("atomic no-replace rename is currently implemented on Linux")
			}
			v, dir := newVault(t)
			for round := range 150 {
				from := fmt.Sprintf("source-%d.md", round)
				to := fmt.Sprintf("dest/%d/note.md", round)
				if _, err := v.Create(from, "moved"); err != nil {
					t.Fatal(err)
				}
				start := make(chan struct{})
				var createErr, moveErr error
				var workers sync.WaitGroup
				workers.Add(2)
				go func() {
					defer workers.Done()
					<-start
					if !external {
						_, createErr = v.Create(to, "created")
						return
					}
					p := filepath.Join(dir, to)
					if createErr = os.MkdirAll(filepath.Dir(p), 0o755); createErr != nil {
						return
					}
					var f *os.File
					f, createErr = os.OpenFile(p, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
					if createErr == nil {
						_, createErr = f.WriteString("created")
						createErr = errors.Join(createErr, f.Close())
					}
				}()
				go func() {
					defer workers.Done()
					<-start
					_, moveErr = v.Move(from, to)
				}()
				close(start)
				workers.Wait()
				want := "moved"
				if createErr == nil {
					want = "created"
					if !errors.Is(moveErr, ErrExists) {
						t.Fatalf("round %d: create succeeded, move = %v; want ErrExists", round, moveErr)
					}
					source, err := v.Read(from)
					if err != nil || source.Content != "moved" {
						t.Fatalf("source lost: %+v, %v", source, err)
					}
				} else if moveErr != nil || !(errors.Is(createErr, ErrExists) || errors.Is(createErr, os.ErrExist)) {
					t.Fatalf("round %d: create = %v, move = %v; want one winner", round, createErr, moveErr)
				}
				dest, err := v.Read(to)
				if err != nil || dest.Content != want {
					t.Fatalf("destination = %+v, %v; want %q", dest, err, want)
				}
			}
		})
	}
}

func TestTrashKeepsAConcurrentExternalDestination(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("atomic no-replace rename is currently implemented on Linux")
	}
	v, dir := newVault(t)
	if err := os.Mkdir(filepath.Join(dir, TrashDir), 0o755); err != nil {
		t.Fatal(err)
	}
	for round := range 150 {
		name := fmt.Sprintf("note-%d.md", round)
		if _, err := v.Create(name, "trashed"); err != nil {
			t.Fatal(err)
		}
		start := make(chan struct{})
		var workers sync.WaitGroup
		var externalErr, trashErr error
		var dest string
		workers.Add(2)
		go func() {
			defer workers.Done()
			<-start
			f, err := os.OpenFile(filepath.Join(dir, TrashDir, name), os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
			externalErr = err
			if err == nil {
				_, externalErr = f.WriteString("external")
				externalErr = errors.Join(externalErr, f.Close())
			}
		}()
		go func() {
			defer workers.Done()
			<-start
			dest, trashErr = v.Trash(name)
		}()
		close(start)
		workers.Wait()
		if trashErr != nil {
			t.Fatal(trashErr)
		}
		data, err := os.ReadFile(filepath.Join(dir, dest))
		if err != nil || string(data) != "trashed" {
			t.Fatalf("trash content = %q, %v; want trashed", data, err)
		}
		if externalErr == nil {
			data, err := os.ReadFile(filepath.Join(dir, TrashDir, name))
			if err != nil || string(data) != "external" {
				t.Fatalf("external trash entry overwritten: %q, %v", data, err)
			}
		} else if !errors.Is(externalErr, os.ErrExist) {
			t.Fatal(externalErr)
		}
	}
}

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

func TestSavedBMPAttachmentsAppearInTheTree(t *testing.T) {
	v, dir := newVault(t)
	if err := os.WriteFile(filepath.Join(dir, ".obsidian/app.json"), []byte(`{"attachmentFolderPath":"Attachments"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	bmp := append([]byte{'B', 'M'}, make([]byte, 52)...)
	p, err := v.SaveImage("a.md", bmp)
	if err != nil {
		t.Fatal(err)
	}
	tree, err := v.Tree()
	if err != nil {
		t.Fatal(err)
	}
	for _, folder := range tree {
		for _, file := range folder.Children {
			if file.Path == p {
				return
			}
		}
	}
	t.Fatalf("saved BMP attachment %q is missing from the link-resolution tree: %+v", p, tree)
}

func TestTrashKeepsEveryVersionWithTheSameName(t *testing.T) {
	v, dir := newVault(t)
	versions := map[string]string{}
	for i := range 100 {
		content := fmt.Sprintf("version %d", i)
		if _, err := v.Create(fmt.Sprintf("batch/%d/note.md", i), content); err != nil {
			t.Fatal(err)
		}
		p, err := v.Trash(fmt.Sprintf("batch/%d/note.md", i))
		if err != nil {
			t.Fatal(err)
		}
		if _, exists := versions[p]; exists {
			t.Fatalf("trash destination %q was reused and overwrote an earlier version", p)
		}
		versions[p] = content
	}
	for p, want := range versions {
		data, err := os.ReadFile(filepath.Join(dir, p))
		if err != nil || string(data) != want {
			t.Fatalf("trash version %s = %q, %v; want %q", p, data, err, want)
		}
	}
}

func TestSavePreservesExistingPermissions(t *testing.T) {
	for _, mode := range []os.FileMode{0o600, 0o640, 0o660} {
		for _, force := range []bool{false, true} {
			t.Run(fmt.Sprintf("%o/force=%v", mode, force), func(t *testing.T) {
				v, dir := newVault(t)
				p := filepath.Join(dir, "a.md")
				if err := os.Chmod(p, mode); err != nil {
					t.Fatal(err)
				}
				note, err := v.Read("a.md")
				if err != nil {
					t.Fatal(err)
				}
				if force {
					note.Mtime = 0
				}
				if _, err := v.Save(note.Path, "edited", note.Mtime); err != nil {
					t.Fatal(err)
				}
				info, err := os.Stat(p)
				if err != nil {
					t.Fatal(err)
				}
				if info.Mode().Perm() != mode {
					t.Fatalf("saved permissions = %o; want %o", info.Mode().Perm(), mode)
				}
			})
		}
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
