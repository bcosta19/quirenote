// Package relink rewrites [[links]] in other notes when a file is renamed.
package relink

import (
	"context"
	"errors"
	"path"
	"regexp"
	"strings"

	"quirenote/internal/search"
	"quirenote/internal/vault"
)

var (
	ErrTypeChanged = errors.New("the extension changed between a note (.md) and another type; update the links by hand")
	ErrNotRenamed  = errors.New("the file was not renamed (the source still exists or the destination does not)")
)

// Plan is what would change. Ambiguous: another file with the old name still
// exists, so name-only links ([[Name]]) stay as they are (they may point to it).
type Plan struct {
	Files     []string `json:"files"`
	Ambiguous bool     `json:"ambiguous"`
}

// Result lists the updated notes and the skipped ones (changed during the operation).
type Result struct {
	Updated   []string `json:"updated"`
	Skipped   []string `json:"skipped"`
	Ambiguous bool     `json:"ambiguous"`
}

// forms returns the ways to reference the file in a [[link]]: name only and full path,
// both without ".md" for notes (Obsidian omits it) and with the extension for other files.
func forms(p string) (name, full string) {
	full = p
	if isNote(p) {
		full = strings.TrimSuffix(p, path.Ext(p))
	}
	return path.Base(full), full
}

func isNote(p string) bool { return strings.EqualFold(path.Ext(p), ".md") }

// Rewrite replaces links to from with links to to in content. It keeps !, .md,
// #section, |alias and \|. Name-only links: with nameLinks=false they stay untouched;
// otherwise they stay name-only if the new name is unique in the vault (nameUnique),
// or switch to the full path, like Obsidian does.
func Rewrite(content, from, to string, nameUnique, nameLinks bool) (string, int) {
	oldName, oldFull := forms(from)
	newName, newFull := forms(to)
	alts := regexp.QuoteMeta(oldName)
	if oldFull != oldName {
		alts += "|" + regexp.QuoteMeta(oldFull)
	}
	md := `()` // other files already carry the extension in the name
	if isNote(from) {
		md = `(\.md)?`
	}
	re := regexp.MustCompile(`(?i)(\[\[)(` + alts + `)` + md + `(\\?[#|][^\]\n]*)?(\]\])`)
	n := 0
	out := re.ReplaceAllStringFunc(content, func(m string) string {
		g := re.FindStringSubmatch(m)
		byName := strings.EqualFold(g[2], oldName)
		if byName && !nameLinks {
			return m
		}
		target := newFull
		if byName && nameUnique {
			target = newName
		}
		n++
		return g[1] + target + g[3] + g[4] + g[5]
	})
	return out, n
}

type change struct {
	path, content string
	mtime         int64
}

// prepare validates the request and computes the new content of each affected note, without writing.
func prepare(ctx context.Context, v *vault.Vault, from, to string) ([]change, bool, error) {
	// Only after the rename: rejects stale or forged requests.
	if v.Exists(from) || !v.Exists(to) {
		return nil, false, ErrNotRenamed
	}
	if !vault.ValidName(to) {
		return nil, false, vault.ErrBadName
	}
	if isNote(from) != isNote(to) {
		return nil, false, ErrTypeChanged
	}
	files, err := search.Files(ctx, v.Dir(), search.BacklinkPattern(from))
	if err != nil {
		return nil, false, err
	}
	tree, err := v.Tree()
	if err != nil {
		return nil, false, err
	}
	// Another file with the old name? (A case-only rename, Lexer → lexer, counts itself.)
	same := 0
	if strings.EqualFold(path.Base(from), path.Base(to)) {
		same = 1
	}
	ambiguous := countName(tree, path.Base(from)) > same
	unique := countName(tree, path.Base(to)) <= 1

	var changes []change
	for _, p := range files {
		note, err := v.Read(p)
		if err != nil {
			continue // gone or unreadable: nothing to do with it
		}
		content, _ := Rewrite(note.Content, from, to, unique, !ambiguous)
		if content == note.Content {
			continue // nothing actually changed (e.g. only ambiguous name-only links)
		}
		changes = append(changes, change{p, content, note.Mtime})
	}
	return changes, ambiguous, nil
}

// Preview tells which notes would change, writing nothing.
func Preview(ctx context.Context, v *vault.Vault, from, to string) (Plan, error) {
	changes, ambiguous, err := prepare(ctx, v, from, to)
	plan := Plan{Files: []string{}, Ambiguous: ambiguous}
	for _, c := range changes {
		plan.Files = append(plan.Files, c.path)
	}
	return plan, err
}

// Run updates the vault notes that link to from, after from became to.
func Run(ctx context.Context, v *vault.Vault, from, to string) (Result, error) {
	res := Result{Updated: []string{}, Skipped: []string{}}
	changes, ambiguous, err := prepare(ctx, v, from, to)
	res.Ambiguous = ambiguous
	if err != nil {
		return res, err
	}
	for _, c := range changes {
		// Write only if the note did not change since it was read (Neovim, Obsidian, Syncthing).

		if _, err := v.Save(c.path, c.content, c.mtime); err != nil {
			if !errors.Is(err, vault.ErrConflict) {
				return res, err
			}
			res.Skipped = append(res.Skipped, c.path)
			continue
		}
		res.Updated = append(res.Updated, c.path)
	}
	return res, nil
}

func countName(entries []vault.Entry, name string) int {
	n := 0
	for _, e := range entries {
		if e.Dir {
			n += countName(e.Children, name)
		} else if strings.EqualFold(e.Name, name) {
			n++
		}
	}
	return n
}
