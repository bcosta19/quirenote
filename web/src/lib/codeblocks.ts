import { syntaxTree } from '@codemirror/language'
import { RangeSetBuilder } from '@codemirror/state'
import { Decoration, ViewPlugin, type DecorationSet, type EditorView, type ViewUpdate } from '@codemirror/view'

// Marca as linhas de blocos ``` com a classe .cm-codeblock (fonte mono e fundo próprio).
// O realce da linguagem embutida não usa a tag "monospace", então a fonte vem por linha.
const line = Decoration.line({ class: 'cm-codeblock' })

function build(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>()
  const { doc } = view.state
  for (const { from, to } of view.visibleRanges) {
    syntaxTree(view.state).iterate({
      from,
      to,
      enter(node) {
        if (node.name !== 'FencedCode') return
        for (let pos = node.from; pos <= node.to; ) {
          const l = doc.lineAt(pos)
          builder.add(l.from, l.from, line)
          pos = l.to + 1
        }
        return false
      },
    })
  }
  return builder.finish()
}

export const codeBlocks = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet
    constructor(view: EditorView) {
      this.decorations = build(view)
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged || syntaxTree(u.startState) !== syntaxTree(u.state)) {
        this.decorations = build(u.view)
      }
    }
  },
  { decorations: (v) => v.decorations },
)
