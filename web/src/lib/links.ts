import type { Entry } from './api'

// Links in Obsidian format: [[target#page=12|alias]]
const WIKILINK = /\[\[([^\]|#]+)(#[^\]|]*)?(\|[^\]]*)?\]\]/g

export interface WikiLink {
  target: string
  page?: number
  cfi?: string // position in an EPUB: [[book.epub#cfi=epubcfi(...)]]
}

export function parseWikiLink(raw: string): WikiLink & { ref: string; alias?: string } {
  const [ref, alias] = raw.replace(/\\([|#])/g, '$1').split('|')
  const page = ref.match(/#page=(\d+)/)?.[1]
  const cfi = ref.match(/#cfi=(.+)$/)?.[1]
  return { ref, alias, target: ref.split('#')[0].trim(), page: page ? Number(page) : undefined, cfi }
}

/** Finds the wikilink that contains position `col` of the line. */
export function linkAt(line: string, col: number): WikiLink | null {
  for (const m of line.matchAll(WIKILINK)) {
    const start = m.index!
    if (col >= start && col <= start + m[0].length) {
      const { target, page, cfi } = parseWikiLink(m[0].slice(2, -2))
      return { target, page, cfi }
    }
  }
  return null
}

function flatten(entries: Entry[], out: string[] = []): string[] {
  for (const e of entries) {
    if (e.dir) flatten(e.children ?? [], out)
    else out.push(e.path)
  }
  return out
}

/**
 * Resolves the target like Obsidian: full path or file name only,
 * with or without the .md extension.
 */
export function resolveLink(tree: Entry[], target: string): string | null {
  const files = flatten(tree)
  const candidates = [target, `${target}.md`]
  for (const c of candidates) {
    const exact = files.find((f) => f === c)
    if (exact) return exact
  }
  for (const c of candidates) {
    const byName = files.find((f) => f.endsWith(`/${c}`) || f === c)
    if (byName) return byName
  }
  return null
}

/** Link to the current page, in a format Obsidian understands too. */
export const pageLink = (path: string, page: number) => `[[${path}#page=${page}]]`

/**
 * Link to the current position in an EPUB (this app's convention; Obsidian just opens the book).
 * Strips the optional id assertions from the CFI ("[c3]"): the brackets would break the [[link]],
 * and the CFI stays valid without them.
 */
export const cfiLink = (path: string, cfi: string) => `[[${path}#cfi=${cfi.replace(/\[[^\]]*\]/g, '')}]]`
