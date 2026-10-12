// Package search faz busca de texto nas notas do vault usando o ripgrep.
package search

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

var ErrNoRipgrep = errors.New("ripgrep (rg) não está instalado")

// Part é um pedaço do trecho encontrado; Hit marca o texto que casou.
type Part struct {
	Text string `json:"text"`
	Hit  bool   `json:"hit,omitempty"`
}

type Result struct {
	Path  string `json:"path"` // relativo ao vault
	Line  int    `json:"line"`
	Parts []Part `json:"parts"`
}

const (
	perFile = 5
	// MaxTotal é o limite de resultados; Search devolve um a mais para o front saber que houve corte.
	MaxTotal  = 200
	maxChars  = 160 // tamanho do trecho mostrado ao redor do que casou
	searchFor = 5 * time.Second
)

// Variantes acentuadas: buscar "funcao" também acha "função".
var accents = map[rune]string{
	'a': "aáàâãä", 'e': "eéèêë", 'i': "iíìîï", 'o': "oóòôõö", 'u': "uúùûü", 'c': "cç", 'n': "nñ",
}

// Pattern monta a regex do ripgrep: texto literal, sem diferenciar acentos.
func Pattern(q string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(q) {
		base := fold(r)
		if v, ok := accents[base]; ok {
			b.WriteString("[" + v + strings.ToUpper(v) + "]")
		} else {
			b.WriteString(regexp.QuoteMeta(string(r)))
		}
	}
	return b.String()
}

func fold(r rune) rune {
	for base, variants := range accents {
		if strings.ContainsRune(variants, r) {
			return base
		}
	}
	return r
}

// rgData é texto UTF-8 ("text") ou, se o conteúdo não for UTF-8 válido
// (ex.: nota antiga em Latin-1), os bytes em base64 ("bytes").
type rgData struct {
	Text  string `json:"text"`
	Bytes string `json:"bytes"`
}

func (d rgData) String() string {
	if d.Bytes == "" {
		return d.Text
	}
	b, err := base64.StdEncoding.DecodeString(d.Bytes)
	if err != nil {
		return ""
	}
	return string(b)
}

type rgMessage struct {
	Type string `json:"type"`
	Data struct {
		Path       rgData `json:"path"`
		Lines      rgData `json:"lines"`
		LineNumber int    `json:"line_number"`
		Submatches []struct {
			Start int `json:"start"`
			End   int `json:"end"`
		} `json:"submatches"`
	} `json:"data"`
}

// Search procura q nos arquivos .md de dir (ignorando ocultos, como .obsidian e .trash).
func Search(ctx context.Context, dir, q string) ([]Result, error) {
	q = strings.TrimSpace(q)
	if q == "" {
		return []Result{}, nil
	}
	return run(ctx, dir, Pattern(q))
}

// BacklinkPattern casa [[links]] para a nota (caminho relativo ao vault, com .md):
// pelo nome ou pelo caminho, com ou sem .md, seguidos de #seção, |apelido (ou \|
// dentro de tabelas) ou ]]. É aproximado: não resolve caminhos parciais/relativos e
// conta [[Nome]] para todas as notas com esse nome, em qualquer pasta.
func BacklinkPattern(note string) string {
	// Notas são citadas sem ".md" (opcional); outros arquivos (PDF, imagem), com extensão.
	full, md := note, ""
	if strings.EqualFold(filepath.Ext(note), ".md") {
		full, md = strings.TrimSuffix(note, filepath.Ext(note)), `(?:\.md)?`
	}
	name := filepath.Base(full)
	alts := regexp.QuoteMeta(name)
	if full != name {
		alts += "|" + regexp.QuoteMeta(full)
	}
	return `\[\[(?:` + alts + `)` + md + `(?:\\?[#|][^\]]*)?\]\]`
}

// Backlinks lista os trechos de outras notas que apontam para note.
func Backlinks(ctx context.Context, dir, note string) ([]Result, error) {
	note = filepath.ToSlash(strings.TrimPrefix(note, "/"))
	if note == "" || strings.HasPrefix(note, "../") {
		return []Result{}, nil
	}
	res, err := run(ctx, dir, BacklinkPattern(note))
	if err != nil {
		return nil, err
	}
	out := res[:0]
	for _, r := range res {
		if r.Path != note { // links da nota para ela mesma não contam
			out = append(out, r)
		}
	}
	return out, nil
}

// Files lista TODAS as notas .md que casam com o padrão (sem os limites de
// resultados da busca). Diferente de Search, falha se o tempo estourar: quem usa
// (atualizar links) não pode agir sobre uma lista incompleta.
func Files(ctx context.Context, dir, pattern string) ([]string, error) {
	rg, err := exec.LookPath("rg")
	if err != nil {
		return nil, ErrNoRipgrep
	}
	ctx, cancel := context.WithTimeout(ctx, 3*searchFor)
	defer cancel()
	cmd := exec.CommandContext(ctx, rg, "--files-with-matches", "--ignore-case", "--no-ignore",
		"--iglob", "*.md", "-e", pattern, "--", ".")
	cmd.Dir = dir
	out, err := cmd.Output()
	if ctx.Err() != nil {
		return nil, errors.New("a busca demorou demais; nada foi alterado")
	}
	var exit *exec.ExitError
	if errors.As(err, &exit) && exit.ExitCode() == 1 {
		return []string{}, nil // código 1 = nenhum arquivo casou
	}
	if err != nil {
		return nil, err
	}
	files := []string{}
	for _, l := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if l != "" {
			files = append(files, filepath.ToSlash(strings.TrimPrefix(l, "./")))
		}
	}
	return files, nil
}

// run executa o ripgrep com o padrão nas notas .md do vault e monta os trechos.
func run(ctx context.Context, dir, pattern string) ([]Result, error) {
	rg, err := exec.LookPath("rg")
	if err != nil {
		return nil, ErrNoRipgrep
	}
	ctx, cancel := context.WithTimeout(ctx, searchFor)
	defer cancel()

	// Sem shell: o termo vai como argumento depois de "-e", nunca é interpretado.
	// --no-ignore: um .gitignore no vault não deve esconder notas (ocultos seguem de fora).
	cmd := exec.CommandContext(ctx, rg, "--json", "--ignore-case", "--no-ignore", "--max-count", "5",
		"--iglob", "*.md", "--max-filesize", "2M", "-e", pattern, "--", ".")
	cmd.Dir = dir
	out, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, err
	}

	results := []Result{}
	count := map[string]int{}
	// bufio.Reader em vez de Scanner: linhas JSON muito longas (nota de uma linha só)
	// não interrompem a leitura do resto dos resultados.
	rd := bufio.NewReaderSize(out, 64*1024)
	for len(results) <= MaxTotal {
		raw, err := rd.ReadBytes('\n')
		if len(raw) == 0 && err != nil {
			break
		}
		var m rgMessage
		if json.Unmarshal(raw, &m) != nil || m.Type != "match" {
			continue
		}
		p := filepath.ToSlash(strings.TrimPrefix(m.Data.Path.String(), "./"))
		if count[p] >= perFile {
			continue
		}
		count[p]++
		results = append(results, Result{Path: p, Line: m.Data.LineNumber, Parts: snippet(m)})
	}
	// Resultados suficientes ou fim da saída: encerra o rg (o código de saída 1 = "nada achado").
	cancel()
	_ = cmd.Wait()
	if errors.Is(ctx.Err(), context.DeadlineExceeded) && len(results) == 0 {
		return nil, errors.New("a busca demorou demais")
	}
	return results, nil
}

// snippet recorta a linha ao redor da primeira ocorrência e marca as ocorrências.
// Os offsets do rg são em bytes; bytes inválidos em UTF-8 viram "�" no fim.
func snippet(m rgMessage) []Part {
	line := strings.TrimRight(m.Data.Lines.String(), "\r\n")
	var subs []struct{ Start, End int }
	for _, s := range m.Data.Submatches {
		if s.Start >= 0 && s.Start <= s.End && s.End <= len(line) {
			subs = append(subs, struct{ Start, End int }{s.Start, s.End})
		}
	}
	if len(subs) == 0 {
		return []Part{{Text: clip(strings.ToValidUTF8(line, "�"))}}
	}
	// Janela em bytes ao redor da primeira ocorrência, ajustada para não cortar UTF-8.
	start := max(0, subs[0].Start-maxChars/2)
	end := min(len(line), start+maxChars*2)
	for start > 0 && !utf8Start(line[start]) {
		start--
	}
	for end < len(line) && !utf8Start(line[end]) {
		end++
	}
	var parts []Part
	pos := start
	for _, s := range subs {
		if s.Start < pos || s.End > end {
			continue
		}
		if s.Start > pos {
			parts = append(parts, Part{Text: line[pos:s.Start]})
		}
		parts = append(parts, Part{Text: line[s.Start:s.End], Hit: true})
		pos = s.End
	}
	if pos < end {
		parts = append(parts, Part{Text: line[pos:end]})
	}
	if len(parts) == 0 { // salvaguarda; com start < end sempre há ao menos uma parte
		return []Part{{Text: clip(strings.ToValidUTF8(line, "�"))}}
	}
	for i := range parts {
		parts[i].Text = strings.ToValidUTF8(parts[i].Text, "�")
	}
	if start > 0 {
		parts[0].Text = "…" + strings.TrimLeft(parts[0].Text, " \t")
	}
	if end < len(line) {
		parts[len(parts)-1].Text += "…"
	}
	return parts
}

func utf8Start(b byte) bool { return b&0xC0 != 0x80 }

func clip(s string) string {
	r := []rune(s)
	if len(r) > maxChars*2 {
		return string(r[:maxChars*2]) + "…"
	}
	return s
}
