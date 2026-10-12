// Package api expõe o vault por HTTP para o front.
package api

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"net/http"
	"path"
	"strings"
	"time"

	"notas/internal/relink"
	"notas/internal/search"
	"notas/internal/vault"
	"notas/internal/watch"
)

type Server struct {
	vault  *vault.Vault
	broker *watch.Broker
	static fs.FS // build do front; nil em dev (Vite serve o front)
}

func New(v *vault.Vault, b *watch.Broker, static fs.FS) *Server {
	return &Server{vault: v, broker: b, static: static}
}

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/tree", s.tree)
	mux.HandleFunc("GET /api/notes/{path...}", s.readNote)
	mux.HandleFunc("PUT /api/notes/{path...}", s.saveNote)
	mux.HandleFunc("GET /api/files/{path...}", s.file)
	mux.HandleFunc("POST /api/dirs/{path...}", s.mkdir)
	mux.HandleFunc("POST /api/notes/{path...}", s.createNote)
	mux.HandleFunc("POST /api/move", s.move)
	mux.HandleFunc("POST /api/trash", s.trash)
	mux.HandleFunc("GET /api/relink", s.relinkPreview)
	mux.HandleFunc("POST /api/relink", s.relink)
	mux.HandleFunc("POST /api/images", s.saveImage)
	mux.HandleFunc("GET /api/events", s.events)
	mux.HandleFunc("GET /api/search", s.search)
	mux.HandleFunc("GET /api/backlinks", s.backlinks)
	if s.static != nil {
		mux.Handle("GET /", spa(s.static))
	}
	return logRequests(requireJSON(mux))
}

// requireJSON recusa requisições que alteram o vault sem Content-Type JSON.
// Formulários e fetch "no-cors" de outras páginas não conseguem enviar esse
// cabeçalho sem preflight, então isso bloqueia CSRF a partir do navegador.
func requireJSON(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			ct := r.Header.Get("Content-Type")
			if mt, _, _ := strings.Cut(ct, ";"); strings.TrimSpace(mt) != "application/json" {
				writeJSON(w, http.StatusUnsupportedMediaType, map[string]string{"error": "use Content-Type: application/json"})
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Server) tree(w http.ResponseWriter, r *http.Request) {
	entries, err := s.vault.Tree()
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, entries)
}

func (s *Server) readNote(w http.ResponseWriter, r *http.Request) {
	note, err := s.vault.Read(r.PathValue("path"))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, note)
}

type saveRequest struct {
	Content string `json:"content"`
	Mtime   int64  `json:"mtime,string"`
}

func (s *Server) saveNote(w http.ResponseWriter, r *http.Request) {
	var req saveRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 10<<20)).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "JSON inválido"})
		return
	}
	note, err := s.vault.Save(r.PathValue("path"), req.Content, req.Mtime)
	if errors.Is(err, vault.ErrConflict) {
		// Devolve a versão do disco para o front decidir (recarregar ou sobrescrever).
		writeJSON(w, http.StatusConflict, note)
		return
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, note)
}

type createRequest struct {
	Content string `json:"content"`
}

// createNote cria uma nota nova; 409 se já existir (nunca sobrescreve).
func (s *Server) createNote(w http.ResponseWriter, r *http.Request) {
	var req createRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 10<<20)).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "JSON inválido"})
		return
	}
	note, err := s.vault.Create(r.PathValue("path"), req.Content)
	if errors.Is(err, vault.ErrExists) {
		writeJSON(w, http.StatusConflict, map[string]string{"error": err.Error()})
		return
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusCreated, note)
}

type moveRequest struct {
	From string `json:"from"`
	To   string `json:"to"`
}

// move renomeia/move um arquivo ou pasta; 409 se o destino já existir.
func (s *Server) move(w http.ResponseWriter, r *http.Request) {
	var req moveRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "JSON inválido"})
		return
	}
	p, err := s.vault.Move(req.From, req.To)
	if errors.Is(err, vault.ErrExists) {
		writeJSON(w, http.StatusConflict, map[string]string{"error": err.Error()})
		return
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"path": p})
}

// relinkPreview diz quais notas teriam os [[links]] atualizados (nada é gravado).
func (s *Server) relinkPreview(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	plan, err := relink.Preview(r.Context(), s.vault, q.Get("from"), q.Get("to"))
	if err != nil {
		writeRelinkErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, plan)
}

// relink atualiza os [[links]] de outras notas depois de renomear from para to.
func (s *Server) relink(w http.ResponseWriter, r *http.Request) {
	var req moveRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "JSON inválido"})
		return
	}
	res, err := relink.Run(r.Context(), s.vault, req.From, req.To)
	if err != nil {
		writeRelinkErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, res)
}

func writeRelinkErr(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, search.ErrNoRipgrep):
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": err.Error()})
	case errors.Is(err, relink.ErrTypeChanged), errors.Is(err, relink.ErrNotRenamed), errors.Is(err, vault.ErrBadName):
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
	default:
		writeErr(w, err)
	}
}

type trashRequest struct {
	Path string `json:"path"`
}

// trash move para a lixeira do vault (.trash), sem apagar nada.
func (s *Server) trash(w http.ResponseWriter, r *http.Request) {
	var req trashRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "JSON inválido"})
		return
	}
	p, err := s.vault.Trash(req.Path)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"path": p})
}

type imageRequest struct {
	Note string `json:"note"` // nota onde a imagem foi colada (define a pasta de anexos)
	Data string `json:"data"` // base64; JSON por causa da proteção anti-CSRF
}

// saveImage grava uma imagem colada/arrastada no editor e devolve o caminho.
func (s *Server) saveImage(w http.ResponseWriter, r *http.Request) {
	var req imageRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, vault.MaxAttachment*4/3+4096)).Decode(&req); err != nil {
		var tooBig *http.MaxBytesError
		if errors.As(err, &tooBig) {
			writeJSON(w, http.StatusRequestEntityTooLarge, map[string]string{"error": "imagem grande demais (máx. 20 MB)"})
		} else {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "JSON inválido"})
		}
		return
	}
	data, err := base64.StdEncoding.DecodeString(req.Data)
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "imagem em base64 inválida"})
		return
	}
	p, err := s.vault.SaveImage(req.Note, data)
	if err != nil {
		if errors.Is(err, vault.ErrNotImage) {
			writeJSON(w, http.StatusUnsupportedMediaType, map[string]string{"error": err.Error()})
		} else {
			writeErr(w, err)
		}
		return
	}
	writeJSON(w, http.StatusCreated, map[string]string{"path": p})
}

func (s *Server) mkdir(w http.ResponseWriter, r *http.Request) {
	p, err := s.vault.Mkdir(r.PathValue("path"))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusCreated, map[string]string{"path": p})
}

func (s *Server) file(w http.ResponseWriter, r *http.Request) {
	f, err := s.vault.OpenFile(r.PathValue("path"))
	if err != nil {
		writeErr(w, err)
		return
	}
	defer func() { _ = f.Close() }()
	info, err := f.Stat()
	if err != nil || info.IsDir() {
		http.NotFound(w, r)
		return
	}
	// Aberto direto pela URL, um SVG/HTML do vault não roda script na origem do app.
	// (fetch do PDF.js e <img> não são afetados por estes cabeçalhos.)
	w.Header().Set("Content-Security-Policy", "sandbox")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	// ServeContent cuida de Range (PDF.js carrega por partes), ETag e cache.
	http.ServeContent(w, r, info.Name(), info.ModTime(), f)
}

func (s *Server) search(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query().Get("q")
	if len(q) > 200 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "busca longa demais"})
		return
	}
	res, err := search.Search(r.Context(), s.vault.Dir(), q)
	if errors.Is(err, search.ErrNoRipgrep) {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": err.Error()})
		return
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, res)
}

// backlinks lista os trechos de outras notas com [[link]] para ?note=.
func (s *Server) backlinks(w http.ResponseWriter, r *http.Request) {
	res, err := search.Backlinks(r.Context(), s.vault.Dir(), r.URL.Query().Get("note"))
	if errors.Is(err, search.ErrNoRipgrep) {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": err.Error()})
		return
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, res)
}

// events mantém uma conexão SSE e envia as mudanças do vault.
func (s *Server) events(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	rc := http.NewResponseController(w)

	ch, unsubscribe := s.broker.Subscribe()
	defer unsubscribe()

	_, _ = fmt.Fprint(w, ": conectado\n\n")
	_ = rc.Flush()
	ping := time.NewTicker(30 * time.Second)
	defer ping.Stop()

	for {
		select {
		case <-r.Context().Done():
			return
		case <-ping.C:
			_, _ = fmt.Fprint(w, ": ping\n\n")
		case e := <-ch:
			data, _ := json.Marshal(e)
			_, _ = fmt.Fprintf(w, "data: %s\n\n", data)
		}
		if err := rc.Flush(); err != nil {
			return
		}
	}
}

// spa serve o build do front e devolve index.html para rotas desconhecidas.
func spa(static fs.FS) http.Handler {
	files := http.FileServerFS(static)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := strings.TrimPrefix(path.Clean(r.URL.Path), "/")
		if p == "" {
			p = "index.html"
		}
		if _, err := fs.Stat(static, p); err != nil {
			http.ServeFileFS(w, r, static, "index.html")
			return
		}
		files.ServeHTTP(w, r)
	})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, err error) {
	status := http.StatusInternalServerError
	switch {
	case errors.Is(err, fs.ErrNotExist):
		status = http.StatusNotFound
	case errors.Is(err, vault.ErrInvalidPath), errors.Is(err, vault.ErrNotNote), errors.Is(err, vault.ErrBadName):
		status = http.StatusBadRequest
	default:
		// os.Root devolve erro próprio para caminhos que escapam do vault.
		if strings.Contains(err.Error(), "escapes from parent") {
			status = http.StatusBadRequest
		} else {
			slog.Error("api", "err", err)
		}
	}
	writeJSON(w, status, map[string]string{"error": err.Error()})
}

func logRequests(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		start := time.Now()
		next.ServeHTTP(w, r)
		if r.URL.Path != "/api/events" {
			slog.Debug("http", "method", r.Method, "path", r.URL.Path, "dur", time.Since(start))
		}
	})
}
