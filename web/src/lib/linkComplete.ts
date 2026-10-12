import { acceptCompletion, autocompletion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete'
import { Prec } from '@codemirror/state'
import { keymap } from '@codemirror/view'
import type { EditorView } from '@codemirror/view'
import { score } from './fuzzy'

// Autocompletar de [[links]]: ao digitar "[[", sugere notas, PDFs e imagens do vault.

const MAX = 40

/**
 * Texto do link como o Obsidian grava: só o nome quando é único no vault,
 * senão o caminho. Notas sem ".md"; outros arquivos com extensão.
 */
export function linkText(path: string, all: string[]): string {
  const name = path.split('/').pop()!
  const lower = name.toLowerCase()
  const unique = all.filter((p) => p.split('/').pop()!.toLowerCase() === lower).length <= 1
  const text = unique ? name : path
  return /\.md$/i.test(text) ? text.slice(0, -3) : text
}

function insertLink(view: EditorView, text: string, from: number, to: number) {
  // Substitui o nome inteiro do alvo, não só até o cursor: completar no meio de
  // [[Nov|apelido]] ou [[Li|vewire]] troca o nome e preserva |apelido e #seção.
  // Só estende quando o link realmente fecha com "]]" nesta linha (direto ou após |apelido/#seção);
  // sem isso, ao transformar uma palavra existente em link, o resto da linha seria apagado.
  const rest = view.state.sliceDoc(to, view.state.doc.lineAt(to).to)
  const m = /^[^[\]|#]*(?=\]\]|[|#][^[\]]*\]\])/.exec(rest)
  if (m) to += m[0].length
  const next = view.state.sliceDoc(to, to + 2)
  const insert = m ? text : text + ']]'
  // Com o link já fechado logo depois do nome, o cursor pula o "]]".
  const anchor = from + insert.length + (next === ']]' ? 2 : 0)
  view.dispatch({ changes: { from, to, insert }, selection: { anchor }, userEvent: 'input.complete' })
}

export function linkCompletion(targets: () => string[]) {
  const source = (ctx: CompletionContext): CompletionResult | null => {
    const m = ctx.matchBefore(/\[\[[^\]|#\n]*$/)
    if (!m) return null
    const all = targets()
    const ranked = all
      .map((path) => ({ path, s: score(m.text.slice(2), path) }))
      .filter((r): r is { path: string; s: number } => r.s !== null)
      .sort((a, b) => b.s - a.s)
      .slice(0, MAX)
    return {
      from: m.from + 2,
      filter: false, // a ordem já vem da nossa busca (ignora acentos)
      options: ranked.map(({ path }, i) => {
        const text = linkText(path, all)
        return {
          label: text.split('/').pop()!,
          detail: path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '',
          boost: MAX - i,
          apply: (view: EditorView, _c: unknown, from: number, to: number) => insertLink(view, text, from, to),
        }
      }),
    }
  }
  return [
    autocompletion({ override: [source], icons: false }),
    // Tab aceita a sugestão (como no Obsidian); sem lista aberta, segue indentando.
    Prec.highest(keymap.of([{ key: 'Tab', run: acceptCompletion }])),
  ]
}
