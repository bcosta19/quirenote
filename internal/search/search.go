// Package search runs text searches over the vault notes using ripgrep.
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

var ErrNoRipgrep = errors.New("ripgrep (rg) is not installed")

// Part is a piece of the matched snippet; Hit marks the matching text.
type Part struct {
	Text string `json:"text"`
	Hit  bool   `json:"hit,omitempty"`
}

type Result struct {
	Path  string `json:"path"` // relative to the vault
	Line  int    `json:"line"`
	Parts []Part `json:"parts"`
}

const (
	perFile = 5
	// MaxTotal is the result limit; Search returns one extra so the frontend knows it was truncated.
	MaxTotal  = 200
	maxChars  = 160 // size of the snippet shown around the match
	searchFor = 5 * time.Second
)

// Accented variants: searching "funcao" also finds "função".
var accents = map[rune]string{
	'a': "aáàâãä", 'e': "eéèêë", 'i': "iíìîï", 'o': "oóòôõö", 'u': "uúùûü", 'c': "cç", 'n': "nñ",
}

// Pattern builds the ripgrep regex: literal text, accent-insensitive.
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

// rgData is UTF-8 text ("text") or, if the content is not valid UTF-8
// (e.g. an old Latin-1 note), the bytes in base64 ("bytes").
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

// Search looks for q in the .md files of dir (skipping hidden ones like .obsidian and .trash).
func Search(ctx context.Context, dir, q string) ([]Result, error) {
	q = strings.TrimSpace(q)
	if q == "" {
		return []Result{}, nil
	}
	return run(ctx, dir, Pattern(q))
}

// BacklinkPattern matches [[links]] to the note (path relative to the vault, with .md):
// by name or by path, with or without .md, followed by #section, |alias (or \|
// inside tables) or ]]. It is approximate: it does not resolve partial/relative paths
// and counts [[Name]] for every note with that name, in any folder.
func BacklinkPattern(note string) string {
	// Notes are referenced without ".md" (optional); other files (PDF, image) with their extension.
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

// Backlinks lists the snippets of other notes that link to note.
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
		if r.Path != note { // links from the note to itself do not count
			out = append(out, r)
		}
	}
	return out, nil
}

// Files lists ALL .md notes matching the pattern (without the search result
// limits). Unlike Search, it fails on timeout: its caller (link updating)
// must not act on an incomplete list.
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
		return nil, errors.New("the search took too long; nothing was changed")
	}
	var exit *exec.ExitError
	if errors.As(err, &exit) && exit.ExitCode() == 1 {
		return []string{}, nil // exit code 1 = no file matched
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

// run executes ripgrep with the pattern over the vault's .md notes and builds the snippets.
func run(ctx context.Context, dir, pattern string) ([]Result, error) {
	rg, err := exec.LookPath("rg")
	if err != nil {
		return nil, ErrNoRipgrep
	}
	ctx, cancel := context.WithTimeout(ctx, searchFor)
	defer cancel()

	// No shell: the term is passed as an argument after "-e" and is never interpreted.
	// --no-ignore: a .gitignore in the vault must not hide notes (hidden files stay excluded).
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
	// bufio.Reader instead of Scanner: very long JSON lines (a single-line note)
	// do not stop reading the remaining results.
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
	// Enough results or end of output: stop rg (exit code 1 = "nothing found").
	cancel()
	_ = cmd.Wait()
	if errors.Is(ctx.Err(), context.DeadlineExceeded) && len(results) == 0 {
		return nil, errors.New("the search took too long")
	}
	return results, nil
}

// snippet cuts the line around the first match and marks the matches.
// rg offsets are in bytes; invalid UTF-8 bytes become "�" at the end.
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
	// Byte window around the first match, adjusted so it does not split UTF-8.
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
	if len(parts) == 0 { // safeguard; with start < end there is always at least one part

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
