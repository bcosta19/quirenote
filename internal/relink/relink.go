// Package relink reescreve [[links]] de outras notas quando um arquivo é renomeado.
package relink

import (
	"context"
	"errors"
	"path"
	"regexp"
	"strings"

	"notas/internal/search"
	"notas/internal/vault"
)

var (
	ErrTypeChanged = errors.New("a extensão mudou entre nota (.md) e outro tipo; atualize os links à mão")
	ErrNotRenamed  = errors.New("o arquivo não foi renomeado (origem ainda existe ou destino não existe)")
)

// Plan é o que seria alterado. Ambiguous: ainda existe outro arquivo com o nome
// antigo, então links só pelo nome ([[Nome]]) ficam como estão (podem ser dele).
type Plan struct {
	Files     []string `json:"files"`
	Ambiguous bool     `json:"ambiguous"`
}

// Result lista as notas atualizadas e as puladas (mudaram durante a operação).
type Result struct {
	Updated   []string `json:"updated"`
	Skipped   []string `json:"skipped"`
	Ambiguous bool     `json:"ambiguous"`
}

// forms devolve as formas de citar o arquivo num [[link]]: só o nome e o caminho,
// ambas sem ".md" para notas (o Obsidian omite) e com extensão para outros arquivos.
func forms(p string) (name, full string) {
	full = p
	if isNote(p) {
		full = strings.TrimSuffix(p, path.Ext(p))
	}
	return path.Base(full), full
}

func isNote(p string) bool { return strings.EqualFold(path.Ext(p), ".md") }

// Rewrite troca os links para from por links para to em content. Preserva !, .md,
// #seção, |apelido e \|. Links só pelo nome: com nameLinks=false ficam intactos;
// senão continuam pelo nome se o novo nome for único no vault (nameUnique), ou
// passam ao caminho, como o Obsidian.
func Rewrite(content, from, to string, nameUnique, nameLinks bool) (string, int) {
	oldName, oldFull := forms(from)
	newName, newFull := forms(to)
	alts := regexp.QuoteMeta(oldName)
	if oldFull != oldName {
		alts += "|" + regexp.QuoteMeta(oldFull)
	}
	md := `()` // outros arquivos já trazem a extensão no nome
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

// prepare valida o pedido e calcula o novo conteúdo de cada nota afetada, sem gravar.
func prepare(ctx context.Context, v *vault.Vault, from, to string) ([]change, bool, error) {
	// Só depois do rename: barra pedidos velhos ou forjados.
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
	// Outro arquivo com o nome antigo? (Renomear só maiúsculas, Lexer → lexer, conta o próprio.)
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
			continue // sumiu ou ilegível: nada a fazer nela
		}
		content, _ := Rewrite(note.Content, from, to, unique, !ambiguous)
		if content == note.Content {
			continue // nada mudou de fato (ex.: só os links pelo nome, ambíguos)
		}
		changes = append(changes, change{p, content, note.Mtime})
	}
	return changes, ambiguous, nil
}

// Preview diz quais notas seriam alteradas, sem gravar nada.
func Preview(ctx context.Context, v *vault.Vault, from, to string) (Plan, error) {
	changes, ambiguous, err := prepare(ctx, v, from, to)
	plan := Plan{Files: []string{}, Ambiguous: ambiguous}
	for _, c := range changes {
		plan.Files = append(plan.Files, c.path)
	}
	return plan, err
}

// Run atualiza as notas do vault que apontam para from, depois de from virar to.
func Run(ctx context.Context, v *vault.Vault, from, to string) (Result, error) {
	res := Result{Updated: []string{}, Skipped: []string{}}
	changes, ambiguous, err := prepare(ctx, v, from, to)
	res.Ambiguous = ambiguous
	if err != nil {
		return res, err
	}
	for _, c := range changes {
		// Grava só se a nota não mudou desde a leitura (Neovim, Obsidian, Syncthing).
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
