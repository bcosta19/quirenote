import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete'
import { defaultKeymap, history, historyKeymap, indentLess, indentMore } from '@codemirror/commands'
import { bracketMatching, foldKeymap, indentOnInput, indentUnit } from '@codemirror/language'
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search'
import { EditorState, Prec, RangeSetBuilder, StateEffect, type Extension } from '@codemirror/state'
import {
  Decoration,
  EditorView,
  ViewPlugin,
  drawSelection,
  dropCursor,
  highlightSpecialChars,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view'

// Ajustes de escrita, alinhados com o Obsidian (TAB, largura 4) e com o
// wrap/linebreak/breakindent do Neovim.

const TAB_SIZE = 4

// Prefixo de linha que define o recuo da continuação: espaços + marcador de lista/citação.
const PREFIX = /^([ \t]*)([-*+][ \t]+\[[ xX]\][ \t]+|[-*+][ \t]+|\d+[.)][ \t]+|>[ \t]*)?/

const remeasure = StateEffect.define<null>()

let canvas: CanvasRenderingContext2D | null = null
const widths = new Map<string, number>()

function prefixWidth(prefix: string, font: string): number {
  const key = font + '|' + prefix
  const cached = widths.get(key)
  if (cached !== undefined) return cached
  canvas ??= document.createElement('canvas').getContext('2d')
  if (!canvas) return 0
  canvas.font = font
  const space = canvas.measureText(' ').width
  let w = 0
  for (const ch of prefix) w += ch === '\t' ? space * TAB_SIZE : canvas.measureText(ch).width
  widths.set(key, w)
  return w
}

/**
 * "breakindent": quando um item de lista quebra linha, a continuação alinha com
 * o texto do item, não com a margem. Mede o prefixo na fonte real do editor.
 */
const hangingIndent = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet
    constructor(readonly view: EditorView) {
      this.decorations = this.build()
      // A fonte embutida pode carregar depois: mede de novo quando estiver pronta.
      document.fonts.ready.then(() => {
        widths.clear()
        view.dispatch({ effects: remeasure.of(null) })
      })
    }
    update(u: ViewUpdate) {
      const again = u.transactions.some((tr) => tr.effects.some((e) => e.is(remeasure)))
      if (u.docChanged || u.viewportChanged || again) this.decorations = this.build()
    }
    build(): DecorationSet {
      const builder = new RangeSetBuilder<Decoration>()
      const font = getComputedStyle(this.view.contentDOM).font
      const { doc } = this.view.state
      for (const { from, to } of this.view.visibleRanges) {
        for (let pos = from; pos <= to; ) {
          const line = doc.lineAt(pos)
          const prefix = line.text.match(PREFIX)?.[0] ?? ''
          if (prefix && prefix.length < line.length) {
            const w = prefixWidth(prefix, font)
            builder.add(
              line.from,
              line.from,
              Decoration.line({ attributes: { style: `padding-left: calc(6px + ${w}px); text-indent: -${w}px` } }),
            )
          }
          pos = line.to + 1
        }
      }
      return builder.finish()
    }
  },
  { decorations: (v) => v.decorations },
)

/**
 * Base do editor no lugar do basicSetup do CodeMirror, com cara de processador de
 * texto: sem números de linha, sem gutter de dobra e sem destacar a linha lógica
 * atual (que num parágrafo quebrado pinta o parágrafo inteiro). Sem autocompletar.
 */
const base: Extension = [
  highlightSpecialChars(),
  history(),
  drawSelection(),
  dropCursor(),
  EditorState.allowMultipleSelections.of(true),
  indentOnInput(),
  bracketMatching(),
  closeBrackets(),
  highlightSelectionMatches(),
  keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap]),
]

export const writing: Extension = [
  base,
  indentUnit.of('\t'),
  EditorState.tabSize.of(TAB_SIZE),
  // Tab indenta em vez de mover o foco para outro elemento da página.
  Prec.high(
    keymap.of([
      { key: 'Tab', run: indentMore, shift: indentLess },
    ]),
  ),
  hangingIndent,
]
