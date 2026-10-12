// Package vault dá acesso seguro aos arquivos do vault de notas.
//
// Todo acesso passa por um os.Root, então caminhos vindos do navegador
// não conseguem escapar do diretório do vault (../, symlinks, absolutos).
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
	"time"
)

var (
	ErrConflict    = errors.New("arquivo foi modificado no disco")
	ErrInvalidPath = errors.New("caminho inválido")
	ErrNotNote     = errors.New("só arquivos .md podem ser editados")
	ErrExists      = errors.New("já existe um arquivo com esse nome")
)

// Extensões listadas na árvore. Imagens entram para o preview resolver ![[foto.png]]
// pelo nome, como o Obsidian; a barra lateral do front mostra só notas e livros.
var visibleExt = map[string]bool{
	".md": true, ".pdf": true, ".epub": true,
	".png": true, ".jpg": true, ".jpeg": true, ".gif": true, ".webp": true, ".svg": true, ".avif": true,
}

type Vault struct {
	dir  string
	root *os.Root
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
	Mtime   int64  `json:"mtime,string"` // UnixNano; string no JSON (passa de 2^53, o limite exato do Number em JS)
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

// Hidden reporta se o caminho (relativo) passa por arquivo/diretório oculto,
// como .obsidian, .stversions ou os temporários de escrita.
func Hidden(p string) bool {
	for _, part := range strings.Split(p, "/") {
		if strings.HasPrefix(part, ".") && part != "." {
			return true
		}
	}
	return false
}

func clean(p string) (string, error) {
	p = path.Clean(strings.TrimPrefix(p, "/"))
	if p == "." || !fs.ValidPath(p) || Hidden(p) {
		return "", ErrInvalidPath
	}
	return p, nil
}

// Tree devolve os diretórios e arquivos visíveis, diretórios primeiro.
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

// Save grava a nota de forma atômica (arquivo temporário + rename).
//
// baseMtime é o mtime que o cliente leu. Se o arquivo mudou desde então
// (Neovim, Obsidian, Syncthing...), devolve ErrConflict junto com a versão
// atual do disco. baseMtime == 0 força a gravação.
func (v *Vault) Save(p, content string, baseMtime int64) (Note, error) {
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
		if err != nil && !errors.Is(err, fs.ErrNotExist) {
			return Note{}, err
		}
	}

	dir := path.Dir(p)
	if dir != "." {
		if err := v.root.MkdirAll(dir, 0o755); err != nil {
			return Note{}, err
		}
	}
	tmp := path.Join(dir, fmt.Sprintf(".%s.notas-%s.tmp", path.Base(p), randHex()))
	if err := v.root.WriteFile(tmp, []byte(content), 0o644); err != nil {
		return Note{}, err
	}
	if err := v.root.Rename(tmp, p); err != nil {
		_ = v.root.Remove(tmp)
		return Note{}, err
	}
	return v.Read(p)
}

// Mkdir cria uma pasta (e as intermediárias) dentro do vault. Não falha se já existir.
func (v *Vault) Mkdir(p string) (string, error) {
	p, err := clean(p)
	if err != nil {
		return "", err
	}
	if err := v.root.MkdirAll(p, 0o755); err != nil {
		return "", err
	}
	return p, nil
}

// Create cria uma nota nova; falha com ErrExists se o arquivo já existir
// (mesmo que o navegador ainda não saiba dele), sem nunca sobrescrever.
func (v *Vault) Create(p, content string) (Note, error) {
	p, err := clean(p)
	if err != nil {
		return Note{}, err
	}
	if strings.ToLower(path.Ext(p)) != ".md" {
		return Note{}, ErrNotNote
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

// createExcl grava um arquivo novo (O_EXCL: ErrExists se já houver) em p já validado.
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
		_ = v.root.Remove(p) // não deixa arquivo truncado que bloquearia novas tentativas
		return werr
	}
	return nil
}

// ErrNotImage indica um anexo cujo conteúdo não é uma imagem aceita.
var ErrNotImage = errors.New("o arquivo não é uma imagem PNG, JPEG, GIF, WebP ou BMP")

// MaxAttachment é o tamanho máximo de uma imagem colada.
const MaxAttachment = 20 << 20

// Extensão pelo conteúdo (não pelo nome enviado). SVG fica de fora: pode conter script.
var imageExt = map[string]string{
	"image/png": ".png", "image/jpeg": ".jpg", "image/gif": ".gif", "image/webp": ".webp", "image/bmp": ".bmp",
}

// attachmentDir segue a configuração de anexos do Obsidian (.obsidian/app.json,
// "attachmentFolderPath"): vazio ou "/" é a raiz do vault, "./" a pasta da nota,
// "./sub" uma subpasta dela e outro valor uma pasta fixa do vault.
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

// SaveImage grava uma imagem colada/arrastada na pasta de anexos, com o nome que o
// Obsidian usa ("Pasted image AAAAMMDDhhmmss.png"), sem sobrescrever nada.
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

// Move renomeia ou move um arquivo ou pasta dentro do vault, criando as pastas
// do destino. Falha com ErrExists se o destino já existir (nunca sobrescreve).
func (v *Vault) Move(from, to string) (string, error) {
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
	if _, err := v.root.Lstat(from); err != nil {
		return "", err
	}
	// Pasta para dentro dela mesma (ex.: a -> a/b) deixaria tudo inacessível.
	if strings.HasPrefix(to, from+"/") {
		return "", ErrInvalidPath
	}
	// Limitação conhecida: entre este Lstat e o Rename há uma janela mínima em que
	// outro programa (ex.: Syncthing) poderia criar o destino; os.Root não oferece
	// RENAME_NOREPLACE. Aceitável num app pessoal.
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

// TrashDir é a lixeira do vault, a mesma do Obsidian ("Mover para a lixeira do Obsidian").
const TrashDir = ".trash"

// Trash move um arquivo ou pasta para a lixeira do vault, sem apagar nada.
// Se já houver algo com o mesmo nome na lixeira, acrescenta data e hora ao nome.
func (v *Vault) Trash(p string) (string, error) {
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

// OpenFile abre um arquivo do vault para leitura (PDF, EPUB, imagens).
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
