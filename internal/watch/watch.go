// Package watch observa o vault e repassa as mudanças para os navegadores
// conectados (via SSE no pacote api).
package watch

import (
	"context"
	"io/fs"
	"log/slog"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"

	"quirenote/internal/vault"
)

type Event struct {
	Path string `json:"path"` // relativo ao vault
	Op   string `json:"op"`   // write | create | remove | rename
}

// Broker distribui eventos para os assinantes.
type Broker struct {
	mu   sync.Mutex
	subs map[chan Event]struct{}
}

func NewBroker() *Broker { return &Broker{subs: map[chan Event]struct{}{}} }

func (b *Broker) Subscribe() (<-chan Event, func()) {
	ch := make(chan Event, 32)
	b.mu.Lock()
	b.subs[ch] = struct{}{}
	b.mu.Unlock()
	return ch, func() {
		b.mu.Lock()
		delete(b.subs, ch)
		b.mu.Unlock()
	}
}

func (b *Broker) publish(e Event) {
	b.mu.Lock()
	defer b.mu.Unlock()
	for ch := range b.subs {
		select {
		case ch <- e:
		default: // cliente lento: descarta em vez de travar o watcher
		}
	}
}

// Run observa o diretório recursivamente até ctx ser cancelado.
// Eventos repetidos do mesmo arquivo são agrupados (debounce), já que
// editores costumam gerar várias escritas por salvamento.
func Run(ctx context.Context, dir string, b *Broker) error {
	w, err := fsnotify.NewWatcher()
	if err != nil {
		return err
	}
	defer func() { _ = w.Close() }()

	addTree := func(root string) {
		_ = filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
			if err != nil || !d.IsDir() {
				return nil
			}
			if p != dir && vault.Hidden(d.Name()) {
				return filepath.SkipDir
			}
			if err := w.Add(p); err != nil {
				slog.Warn("watch: add", "dir", p, "err", err)
			}
			return nil
		})
	}
	addTree(dir)

	const debounce = 150 * time.Millisecond
	pending := map[string]Event{}
	timer := time.NewTimer(time.Hour)
	timer.Stop()

	for {
		select {
		case <-ctx.Done():
			return nil
		case err := <-w.Errors:
			slog.Warn("watch", "err", err)
		case ev := <-w.Events:
			rel, err := filepath.Rel(dir, ev.Name)
			if err != nil {
				continue
			}
			rel = filepath.ToSlash(rel)
			if vault.Hidden(rel) {
				continue
			}
			if ev.Has(fsnotify.Create) {
				if info, err := os.Stat(ev.Name); err == nil && info.IsDir() {
					addTree(ev.Name)
				}
			}
			pending[rel] = Event{Path: rel, Op: opName(ev.Op)}
			timer.Reset(debounce)
		case <-timer.C:
			for _, e := range pending {
				b.publish(e)
			}
			clear(pending)
		}
	}
}

func opName(op fsnotify.Op) string {
	switch {
	case op.Has(fsnotify.Remove):
		return "remove"
	case op.Has(fsnotify.Rename):
		return "rename"
	case op.Has(fsnotify.Create):
		return "create"
	default:
		return "write"
	}
}
