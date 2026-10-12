import type { Entry } from './api'

// Links no formato do Obsidian: [[alvo#page=12|apelido]]
const WIKILINK = /\[\[([^\]|#]+)(#[^\]|]*)?(\|[^\]]*)?\]\]/g

export interface WikiLink {
  target: string
  page?: number
}

/** Encontra o wikilink que contém a posição `col` da linha. */
export function linkAt(line: string, col: number): WikiLink | null {
  for (const m of line.matchAll(WIKILINK)) {
    const start = m.index!
    if (col >= start && col <= start + m[0].length) {
      const page = m[2]?.match(/page=(\d+)/)?.[1]
      return { target: m[1].trim(), page: page ? Number(page) : undefined }
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
 * Resolve o alvo como o Obsidian: caminho completo ou só o nome do arquivo,
 * com ou sem a extensão .md.
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

/** Link para a página atual, no formato que o Obsidian também entende. */
export const pageLink = (path: string, page: number) => `[[${path}#page=${page}]]`
