import type { Extension } from '@codemirror/state'
import { EditorView, GutterMarker, ViewPlugin, gutter, type BlockInfo, type ViewUpdate } from '@codemirror/view'

// Numeração como a do Word: cada linha que aparece na tela ganha um número,
// inclusive as criadas pela quebra automática. Reflui ao redimensionar a janela.

const HEADING = /^#{1,6}\s/

/** Quantas linhas de tela o bloco ocupa (títulos têm fonte maior que a linha padrão). */
function rowsOf(view: EditorView, block: BlockInfo): number {
  const text = view.state.doc.sliceString(block.from, Math.min(block.to, block.from + 8))
  const scale = HEADING.test(text) ? 1.3 : 1
  return Math.max(1, Math.round(block.height / (view.defaultLineHeight * scale)))
}

// Primeiro número de cada linha visível (chave: posição inicial do bloco).
class Counter {
  starts = new Map<number, number>()
  constructor(view: EditorView) {
    this.compute(view)
  }
  update(u: ViewUpdate) {
    if (u.docChanged || u.viewportChanged || u.geometryChanged) this.compute(u.view)
  }
  compute(view: EditorView) {
    this.starts.clear()
    const { doc } = view.state
    const visible = view.viewportLineBlocks
    if (!visible.length) return
    // Linhas acima da área visível: soma pelas alturas do mapa do CodeMirror.
    let n = 1
    for (let pos = 0; pos < visible[0].from; ) {
      const block = view.lineBlockAt(pos)
      n += rowsOf(view, block)
      pos = block.to + 1
      if (pos > doc.length) break
    }
    for (const block of visible) {
      this.starts.set(block.from, n)
      n += rowsOf(view, block)
    }
  }
}
const counter = ViewPlugin.fromClass(Counter)

class Numbers extends GutterMarker {
  constructor(
    readonly start: number,
    readonly rows: number,
    readonly height: number,
  ) {
    super()
  }
  eq(o: Numbers) {
    return o.start === this.start && o.rows === this.rows && o.height === this.height
  }
  toDOM() {
    const el = document.createElement('div')
    const h = this.height / this.rows
    for (let i = 0; i < this.rows; i++) {
      const row = el.appendChild(document.createElement('div'))
      row.textContent = String(this.start + i)
      row.style.height = row.style.lineHeight = `${h}px`
    }
    return el
  }
}

export const visualLineNumbers: Extension = [
  counter,
  gutter({
    class: 'cm-visualLineNumbers',
    lineMarker(view, block) {
      const start = view.plugin(counter)?.starts.get(block.from)
      if (start === undefined) return null
      return new Numbers(start, rowsOf(view, block), block.height)
    },
    lineMarkerChange: (u) => u.docChanged || u.viewportChanged || u.geometryChanged,
    initialSpacer: () => new Numbers(999, 1, 0),
  }),
]
