// Fuzzy matching for the quick switcher: the query letters must appear
// in order; accents and case do not matter.

const fold = (s: string) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase()

/** Score (higher is better) or null if it does not match. Favors the file name and consecutive letters. */
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
    if (i === prev + 1) s += 3 // consecutive run
    if (i >= nameStart) s += 2 // in the name, not the folder
    if (i === nameStart || /[\s/_-]/.test(p[i - 1] ?? '')) s += 4 // start of a word
    prev = i
    pi = i + 1
  }
  if (p.slice(nameStart).startsWith(q)) s += 10
  return s - p.length * 0.01 // tie-breaker: shorter paths first
}
