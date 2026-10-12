// Busca aproximada para a troca rápida de notas: as letras da consulta precisam
// aparecer em ordem; acentos e maiúsculas não importam.

const fold = (s: string) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase()

/** Pontuação (maior é melhor) ou null se não casar. Favorece o nome do arquivo e letras seguidas. */
export function score(query: string, path: string): number | null {
  const q = fold(query.trim())
  if (!q) return 0
  const p = fold(path)
  const nameStart = p.lastIndexOf('/') + 1
  let s = 0
  let pi = 0
  let prev = -2
  for (const ch of q) {
    if (ch === ' ') continue
    const i = p.indexOf(ch, pi)
    if (i < 0) return null
    s += 1
    if (i === prev + 1) s += 3 // sequência contínua
    if (i >= nameStart) s += 2 // no nome, não na pasta
    if (i === nameStart || /[\s/_-]/.test(p[i - 1] ?? '')) s += 4 // início de palavra
    prev = i
    pi = i + 1
  }
  if (p.slice(nameStart).startsWith(q)) s += 10
  return s - p.length * 0.01 // desempate: caminhos mais curtos primeiro
}
