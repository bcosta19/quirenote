import { EditorView } from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { tags as t } from '@lezer/highlight'
import { oneDark } from '@codemirror/theme-one-dark'

export type Theme = 'light' | 'dark'

// Tipografia comum aos dois temas: títulos maiores, como num livro, e código em mono.
const typography = syntaxHighlighting(
  HighlightStyle.define([
    { tag: t.heading1, fontSize: '1.45em', fontWeight: '700' },
    { tag: t.heading2, fontSize: '1.25em', fontWeight: '700' },
    { tag: t.heading3, fontSize: '1.1em', fontWeight: '700' },
    { tag: [t.heading4, t.heading5, t.heading6], fontWeight: '700' },
    { tag: t.monospace, fontFamily: 'var(--mono)', fontSize: '0.8em' },
    { tag: t.emphasis, fontStyle: 'italic' },
    { tag: t.strong, fontWeight: '700' },
    { tag: t.strikethrough, textDecoration: 'line-through' },
  ]),
)

// "Papel": bege quente, tinta marrom, destaques em sépia.
const paper = [
  EditorView.theme(
    {
      '&': { backgroundColor: '#f2eadb', color: '#332e26' },
      '.cm-content': { caretColor: '#9a5b2e' },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: '#9a5b2e' },
      '.cm-fat-cursor': { background: 'rgba(154, 91, 46, 0.55) !important' },
      '&:not(.cm-focused) .cm-fat-cursor': { outline: '1px solid #9a5b2e', background: 'transparent !important' },
      '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground':
        { backgroundColor: '#ddcba8' },
      '.cm-activeLine': { backgroundColor: 'rgba(154, 91, 46, 0.05)' },
      '.cm-gutters': { backgroundColor: '#f2eadb', color: '#b3a78c', border: 'none' },
      '.cm-activeLineGutter': { backgroundColor: 'transparent', color: '#8a8170' },
      '.cm-foldPlaceholder': { backgroundColor: '#e0d6c1', border: 'none', color: '#8a8170' },
      '.cm-searchMatch': { backgroundColor: '#e9d190' },
      '.cm-matchingBracket': { backgroundColor: '#dfd0b2', outline: 'none' },
      '.cm-panels': { backgroundColor: '#e0d6c1', color: '#332e26' },
      '.cm-tooltip': { backgroundColor: '#e0d6c1', border: '1px solid #cfc2a7', color: '#332e26' },
      '.cm-tooltip-autocomplete > ul > li[aria-selected]': { backgroundColor: '#d9c6a3', color: '#332e26' },
    },
    { dark: false },
  ),
  syntaxHighlighting(
    HighlightStyle.define([
      { tag: t.heading, color: '#8b4a2b' },
      { tag: [t.processingInstruction, t.punctuation, t.contentSeparator], color: '#b8ab90' }, // #, *, -, `
      { tag: [t.link, t.url], color: '#4b6a88' },
      { tag: t.quote, color: '#6f6553', fontStyle: 'italic' },
      { tag: t.monospace, color: '#6b4f3a' },
      { tag: [t.meta, t.comment], color: '#a09680' }, // frontmatter
      { tag: [t.keyword, t.atom, t.bool], color: '#8b4a2b' },
      { tag: [t.string, t.special(t.string)], color: '#5f7a3a' },
      { tag: [t.number, t.literal], color: '#9a5b2e' },
      { tag: [t.variableName, t.propertyName], color: '#332e26' },
      { tag: t.typeName, color: '#4b6a88' },
      { tag: t.invalid, color: '#b3261e' },
    ]),
  ),
]

export const editorTheme = (theme: Theme): Extension => [typography, theme === 'light' ? paper : oneDark]
