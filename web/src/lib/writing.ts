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

// Writing tweaks, aligned with Obsidian (TAB, width 4) and with
// Neovim's wrap/linebreak/breakindent.

const TAB_SIZE = 4

// Line prefix that sets the continuation indent: spaces + list/quote marker.
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
 * "breakindent": when a list item wraps, the continuation aligns with the
 * item text, not the margin. Measures the prefix in the editor's actual font.
 */
const hangingIndent = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet
    constructor(readonly view: EditorView) {
      this.decorations = this.build()
      // The embedded font may load later: measure again once it is ready.
      document.fonts.ready.then(() => {
        widths.clear()
        view.dispatch({ effects: remeasure.of(null) })
      })
      // A font picked in the settings changes a CSS variable on <html>, which CodeMirror does not
      // see: without this the continuation stays aligned to the old font until the next edit.
      this.font = getComputedStyle(view.contentDOM).font
      this.fontWatch.observe(document.documentElement, { attributes: true, attributeFilter: ['style'] })
    }
    font: string
    fontWatch = new MutationObserver(() => {
      const font = getComputedStyle(this.view.contentDOM).font
      if (font === this.font) return
      this.font = font
      this.view.requestMeasure()
      this.view.dispatch({ effects: remeasure.of(null) })
    })
    destroy() {
      this.fontWatch.disconnect()
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
 * Editor base instead of CodeMirror's basicSetup, feeling like a word processor:
 * no line numbers, no fold gutter and no highlight of the current logical line
 * (which in a wrapped paragraph paints the whole paragraph). No autocompletion.
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
  // Tab indents instead of moving focus to another element on the page.
  Prec.high(
    keymap.of([
      { key: 'Tab', run: indentMore, shift: indentLess },
    ]),
  ),
  hangingIndent,
]
