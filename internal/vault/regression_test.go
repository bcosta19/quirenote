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
