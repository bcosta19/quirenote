// Package vault gives safe access to the files of the notes vault.
//
// Every access goes through an os.Root, so paths coming from the browser
// cannot escape the vault directory (../, symlinks, absolute paths).
package vault

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"path"
	"sort"
	"strings"
	"sync"
	"time"
)

var (
	ErrConflict    = errors.New("file was modified on disk")
	ErrInvalidPath = errors.New("invalid path")
	ErrNotNote     = errors.New("only .md files can be edited")
	ErrExists      = errors.New("a file with this name already exists")
	ErrBadName     = errors.New("the name cannot contain # ^ [ ] | (Obsidian rejects them; they would break links)")
)

// Extensions listed in the tree. Images are included so the preview can resolve ![[photo.png]]
// by name, like Obsidian; the frontend sidebar only shows notes and books.
var visibleExt = map[string]bool{
	".md": true, ".pdf": true, ".epub": true,
	".png": true, ".jpg": true, ".jpeg": true, ".gif": true, ".webp": true, ".svg": true, ".avif": true,
}

type Vault struct {
	dir  string
	root *os.Root
	mu   sync.Mutex
}

type Entry struct {
	Name     string  `json:"name"`
	Path     string  `json:"path"`
	Dir      bool    `json:"dir"`
	Children []Entry `json:"children,omitempty"`
}

type Note struct {
	Path    string `json:"path"`
	Content string `json:"content"`
	Mtime   int64  `json:"mtime,string"` // UnixNano; a string in JSON (it exceeds 2^53, the exact limit of a JS Number)
}

func Open(dir string) (*Vault, error) {
	root, err := os.OpenRoot(dir)
	if err != nil {
		return nil, err
	}
	return &Vault{dir: dir, root: root}, nil
}

func (v *Vault) Dir() string  { return v.dir }
func (v *Vault) Close() error { return v.root.Close() }

// Hidden reports whether the (relative) path goes through a hidden file/directory,
// such as .obsidian, .stversions or the temporary write files.
func Hidden(p string) bool {
	for _, part := range strings.Split(p, "/") {
		if strings.HasPrefix(part, ".") && part != "." {
			return true
		}
	}
	return false
}

// ValidName reports whether the new name (last path element) can be referenced in a
// [[link]]: Obsidian rejects # ^ [ ] | in names because they mean something in links.
// Only the new name is checked: existing folders with these characters remain usable.
func ValidName(p string) bool {
	return !strings.ContainsAny(path.Base(p), "#^[]|\r\n")
}

// Exists reports whether the path exists in the vault.
func (v *Vault) Exists(p string) bool {
	p, err := clean(p)
	if err != nil {
		return false
	}
	_, err = v.root.Lstat(p)
	return err == nil
}

func clean(p string) (string, error) {
	p = path.Clean(strings.TrimPrefix(p, "/"))
	if p == "." || !fs.ValidPath(p) || Hidden(p) {
		return "", ErrInvalidPath
	}
	return p, nil
}

// Tree returns the visible directories and files, directories first.
func (v *Vault) Tree() ([]Entry, error) {
	return v.readDir(".")
}

func (v *Vault) readDir(dir string) ([]Entry, error) {
	items, err := fs.ReadDir(v.root.FS(), dir)
	if err != nil {
		return nil, err
	}
	entries := []Entry{}
	for _, it := range items {
		name := it.Name()
		if strings.HasPrefix(name, ".") {
			continue
		}
		p := path.Join(dir, name)
		if it.IsDir() {
			children, err := v.readDir(p)
			if err != nil {
				return nil, err
			}
			entries = append(entries, Entry{Name: name, Path: p, Dir: true, Children: children})
		} else if visibleExt[strings.ToLower(path.Ext(name))] {
			entries = append(entries, Entry{Name: name, Path: p})
		}
	}
	sort.SliceStable(entries, func(i, j int) bool {
		if entries[i].Dir != entries[j].Dir {
			return entries[i].Dir
		}
		return strings.ToLower(entries[i].Name) < strings.ToLower(entries[j].Name)
	})
	return entries, nil
}

func (v *Vault) Read(p string) (Note, error) {
	p, err := clean(p)
	if err != nil {
		return Note{}, err
	}
	data, err := v.root.ReadFile(p)
	if err != nil {
		return Note{}, err
	}
	info, err := v.root.Stat(p)
	if err != nil {
		return Note{}, err
	}
	return Note{Path: p, Content: string(data), Mtime: info.ModTime().UnixNano()}, nil
}

// Save writes the note atomically (temporary file + rename).
//
// baseMtime is the mtime the client read. If the file changed since then
// (Neovim, Obsidian, Syncthing...), it returns ErrConflict along with the
// current version on disk. baseMtime == 0 forces the write.
func (v *Vault) Save(p, content string, baseMtime int64) (Note, error) {
	v.mu.Lock()
	defer v.mu.Unlock()
	p, err := clean(p)
	if err != nil {
		return Note{}, err
	}
	if strings.ToLower(path.Ext(p)) != ".md" {
		return Note{}, ErrNotNote
	}
	if baseMtime != 0 {
		info, err := v.root.Stat(p)
		if err == nil && info.ModTime().UnixNano() != baseMtime {
			current, rerr := v.Read(p)
			if rerr != nil {
				return Note{}, rerr
			}
			return current, ErrConflict
		}
		if err != nil {
			return Note{}, err
		}
	}

	dir := path.Dir(p)
	if dir != "." {
		if err := v.root.MkdirAll(dir, 0o755); err != nil {
			return Note{}, err
		}
	}
	tmp := path.Join(dir, fmt.Sprintf(".%s.quirenote-%s.tmp", path.Base(p), randHex()))
	if err := v.root.WriteFile(tmp, []byte(content), 0o644); err != nil {
		return Note{}, err
	}
	if err := v.root.Rename(tmp, p); err != nil {
		_ = v.root.Remove(tmp)
		return Note{}, err
	}
	return v.Read(p)
}

// Mkdir creates a folder (and its parents) inside the vault. It does not fail if it exists.
func (v *Vault) Mkdir(p string) (string, error) {
	p, err := clean(p)
	if err != nil {
		return "", err
	}
	if !ValidName(p) {
		return "", ErrBadName
	}
	if err := v.root.MkdirAll(p, 0o755); err != nil {
		return "", err
	}
	return p, nil
}

// Create creates a new note; it fails with ErrExists if the file already exists
// (even if the browser does not know about it yet), never overwriting.
func (v *Vault) Create(p, content string) (Note, error) {
	p, err := clean(p)
	if err != nil {
		return Note{}, err
	}
	if strings.ToLower(path.Ext(p)) != ".md" {
		return Note{}, ErrNotNote
	}
	if !ValidName(p) {
		return Note{}, ErrBadName
	}
	if dir := path.Dir(p); dir != "." {
		if err := v.root.MkdirAll(dir, 0o755); err != nil {
			return Note{}, err
		}
	}
	if err := v.createExcl(p, []byte(content)); err != nil {
		return Note{}, err
	}
	return v.Read(p)
}

// createExcl writes a new file (O_EXCL: ErrExists if present) at an already validated p.
func (v *Vault) createExcl(p string, data []byte) error {
	f, err := v.root.OpenFile(p, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
	if errors.Is(err, fs.ErrExist) {
		return ErrExists
	}
	if err != nil {
		return err
	}
	_, werr := f.Write(data)
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	if werr != nil {
		_ = v.root.Remove(p) // do not leave a truncated file that would block retries
		return werr
	}
	return nil
}

// ErrNotImage means an attachment whose content is not an accepted image.
var ErrNotImage = errors.New("the file is not a PNG, JPEG, GIF, WebP or BMP image")

// MaxAttachment is the maximum size of a pasted image.
const MaxAttachment = 20 << 20

// Extension by content (not by the uploaded name). SVG is excluded: it may contain scripts.
var imageExt = map[string]string{
	"image/png": ".png", "image/jpeg": ".jpg", "image/gif": ".gif", "image/webp": ".webp", "image/bmp": ".bmp",
}

// attachmentDir follows Obsidian's attachment setting (.obsidian/app.json,
// "attachmentFolderPath"): empty or "/" is the vault root, "./" the note's folder,
// "./sub" a subfolder of it and any other value a fixed vault folder.
func (v *Vault) attachmentDir(note string) string {
	var cfg struct {
		AttachmentFolderPath string `json:"attachmentFolderPath"`
	}
	if data, err := v.root.ReadFile(".obsidian/app.json"); err == nil {
		_ = json.Unmarshal(data, &cfg)
	}
	dir := strings.TrimSpace(cfg.AttachmentFolderPath)
	switch {
	case dir == "" || dir == "/":
		return "."
	case dir == "." || strings.HasPrefix(dir, "./"):
		noteDir := "."
		if n, err := clean(note); err == nil {
			noteDir = path.Dir(n)
		}
		return path.Join(noteDir, strings.TrimPrefix(dir, "."))
	default:
		return path.Clean(strings.Trim(dir, "/"))
	}
}

// SaveImage writes a pasted/dropped image to the attachment folder, with the name
// Obsidian uses ("Pasted image YYYYMMDDhhmmss.png"), never overwriting anything.
func (v *Vault) SaveImage(note string, data []byte) (string, error) {
	if len(data) == 0 || len(data) > MaxAttachment {
		return "", ErrNotImage
	}
	ext, ok := imageExt[http.DetectContentType(data)]
	if !ok {
		return "", ErrNotImage
	}
	dir := v.attachmentDir(note)
	if dir != "." {
		d, err := clean(dir)
		if err != nil {
			return "", err
		}
		if err := v.root.MkdirAll(d, 0o755); err != nil {
			return "", err
		}
		dir = d
	}
	base := "Pasted image " + time.Now().Format("20060102150405")
	for i := 0; i < 100; i++ {
		name := base + ext
		if i > 0 {
			name = fmt.Sprintf("%s %d%s", base, i, ext)
		}
		p := path.Join(dir, name)
		err := v.createExcl(p, data)
		if errors.Is(err, ErrExists) {
			continue
		}
		return p, err
	}
	return "", ErrExists
}

// Move renames or moves a file or folder inside the vault, creating the destination
// folders. It fails with ErrExists if the destination exists (never overwrites).
func (v *Vault) Move(from, to string) (string, error) {
	v.mu.Lock()
	defer v.mu.Unlock()
	from, err := clean(from)
	if err != nil {
		return "", err
	}
	to, err = clean(to)
	if err != nil {
		return "", err
	}
	if from == to {
		return to, nil
	}
	if !ValidName(to) {
		return "", ErrBadName
	}
	if _, err := v.root.Lstat(from); err != nil {
		return "", err
	}
	// Moving a folder into itself (e.g. a -> a/b) would make everything unreachable.
	if strings.HasPrefix(to, from+"/") {
		return "", ErrInvalidPath
	}
	// Known limitation: between this Lstat and the Rename there is a tiny window in which
	// another program (e.g. Syncthing) could create the destination; os.Root has no
	// RENAME_NOREPLACE. Acceptable for a personal app.
	if _, err := v.root.Lstat(to); err == nil {
		return "", ErrExists
	} else if !errors.Is(err, fs.ErrNotExist) {
		return "", err
	}
	if dir := path.Dir(to); dir != "." {
		if err := v.root.MkdirAll(dir, 0o755); err != nil {
			return "", err
		}
	}
	if err := v.root.Rename(from, to); err != nil {
		return "", err
	}
	return to, nil
}

// TrashDir is the vault trash, the same one Obsidian uses ("Move to Obsidian trash").
const TrashDir = ".trash"

// Trash moves a file or folder to the vault trash, deleting nothing.
// If the trash already has something with the same name, it appends date and time.
func (v *Vault) Trash(p string) (string, error) {
	v.mu.Lock()
	defer v.mu.Unlock()
	p, err := clean(p)
	if err != nil {
		return "", err
	}
	if _, err := v.root.Lstat(p); err != nil {
		return "", err
	}
	if err := v.root.MkdirAll(TrashDir, 0o755); err != nil {
		return "", err
	}
	name := path.Base(p)
	dest := path.Join(TrashDir, name)
	if _, err := v.root.Lstat(dest); err == nil {
		ext := path.Ext(name)
		dest = path.Join(TrashDir, fmt.Sprintf("%s %s%s", strings.TrimSuffix(name, ext), time.Now().Format("2006-01-02 150405.000"), ext))
	}
	if err := v.root.Rename(p, dest); err != nil {
		return "", err
	}
	return dest, nil
}

// OpenFile opens a vault file for reading (PDF, EPUB, images).
func (v *Vault) OpenFile(p string) (*os.File, error) {
	p, err := clean(p)
	if err != nil {
		return nil, err
	}
	return v.root.Open(p)
}

func randHex() string {
	b := make([]byte, 4)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}
