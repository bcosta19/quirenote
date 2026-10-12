import { acceptCompletion, autocompletion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete'
import { Prec } from '@codemirror/state'
import { keymap } from '@codemirror/view'
import type { EditorView } from '@codemirror/view'
import { score } from './fuzzy'

// [[link]] autocompletion: typing "[[" suggests notes, PDFs and images from the vault.

const MAX = 40

/**
 * Link text the way Obsidian writes it: the name only when it is unique in the vault,
 * otherwise the path. Notes without ".md"; other files with their extension.
 */
export function linkText(path: string, all: string[]): string {
  const name = path.split('/').pop()!
  const lower = name.toLowerCase()
  const unique = all.filter((p) => p.split('/').pop()!.toLowerCase() === lower).length <= 1
  const text = unique ? name : path
  return /\.md$/i.test(text) ? text.slice(0, -3) : text
}

function insertLink(view: EditorView, text: string, from: number, to: number) {
  // Replaces the whole target name, not just up to the cursor: completing in the middle of
  // [[Nov|alias]] or [[Li|vewire]] swaps the name and keeps |alias and #section.
  // Only extends when the link really closes with "]]" on this line (directly or after |alias/#section);
  // otherwise, turning an existing word into a link would delete the rest of the line.
  const rest = view.state.sliceDoc(to, view.state.doc.lineAt(to).to)
  const m = /^[^[\]|#]*(?=\]\]|[|#][^[\]]*\]\])/.exec(rest)
  if (m) to += m[0].length
  const next = view.state.sliceDoc(to, to + 2)
  const insert = m ? text : text + ']]'
  // With the link already closed right after the name, the cursor skips the "]]".
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
      filter: false, // the order already comes from our matcher (accent-insensitive)
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
    // Tab accepts the suggestion (like Obsidian); without an open list, it keeps indenting.
    Prec.highest(keymap.of([{ key: 'Tab', run: acceptCompletion }])),
  ]
}
