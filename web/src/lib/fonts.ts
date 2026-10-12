// EB Garamond embedded in the build (and in the Go binary): it looks the same on any device.
// Only latin + latin-ext (accented characters) to keep the binary small.
import '@fontsource/eb-garamond/latin-400.css'
import '@fontsource/eb-garamond/latin-ext-400.css'
import '@fontsource/eb-garamond/latin-400-italic.css'
import '@fontsource/eb-garamond/latin-ext-400-italic.css'
import '@fontsource/eb-garamond/latin-700.css'
import '@fontsource/eb-garamond/latin-ext-700.css'
import '@fontsource/eb-garamond/latin-700-italic.css'
import '@fontsource/eb-garamond/latin-ext-700-italic.css'

/** What a custom font changes: the app chrome, the notes (editor and preview) or the code. */
export const FONT_ROLES = ['ui', 'editor', 'mono'] as const
export type FontRole = (typeof FONT_ROLES)[number]

export const FONT_LABELS: Record<FontRole, string> = {
  ui: 'Interface',
  editor: 'Notes',
  mono: 'Code',
}

export const FONT_HINTS: Record<FontRole, string> = {
  ui: 'Sidebar, toolbars, dialogs',
  editor: 'Editor and preview (EB Garamond by default)',
  mono: 'Code blocks and shortcuts',
}

/** Custom font per role; an empty name means "keep the default chain". */
export type FontPrefs = Record<FontRole, string>

const KEY = 'quirenote:fonts'

/**
 * A saved stack is a ready-made CSS font list, applied before the first paint by
 * public/theme-init.js. Keep that script's test in sync with this one.
 *
 * It accepts exactly what fontStack() writes: the family (quoted, or a CSS generic keyword) and
 * the tail of the role. Anything else is ignored on both paths, so a hand-edited value cannot
 * reach font-family as something the app itself would never produce.
 */
const STACK_RE = /^(?:'[\p{L}\p{N} ._-]{1,64}'|[a-z][a-z0-9-]{0,20}), var\(--font-(?:ui|editor|mono)-tail\)$/u

/** Generic families are CSS keywords: quoting one turns it into a family name that does not exist. */
const GENERIC = new Set([
  'serif',
  'sans-serif',
  'monospace',
  'cursive',
  'fantasy',
  'system-ui',
  'ui-serif',
  'ui-sans-serif',
  'ui-monospace',
  'ui-rounded',
  'math',
  'emoji',
  'fangsong',
])

/**
 * What can be part of a font family name, and nothing else: the name goes straight into a
 * CSS value, so quotes, semicolons and braces are dropped (an emptied field falls back to the
 * default chain).
 */
export function cleanFontName(value: unknown): string {
  if (typeof value !== 'string') return ''
  return keepFontChars(value).replace(/\s+/g, ' ').trim().slice(0, 64).trim()
}

/**
 * The name without the characters it cannot have, but keeping the spaces and the length: a text
 * field uses it to tell a dropped character from a space it should let the user type.
 */
export function keepFontChars(value: string): string {
  return value.replace(/[^\p{L}\p{N} ._-]+/gu, '')
}

/** The custom font as CSS: the family, then the default chain of the role (accents still render). */
export function fontStack(role: FontRole, name: string): string {
  const clean = cleanFontName(name)
  if (!clean) return ''
  const family = GENERIC.has(clean.toLowerCase()) ? clean.toLowerCase() : `'${clean}'`
  return `${family}, var(--font-${role}-tail)`
}

/** The family of a saved stack, for the settings field ('' = the default chain). */
function familyOf(stack: string): string {
  const first = stack.split(',')[0]?.trim() ?? ''
  return cleanFontName(first.replace(/^['"]|['"]$/g, ''))
}

/** Reads the saved fonts; anything unexpected (old value, hand-edited, corrupted) is ignored. */
export function loadFonts(): FontPrefs {
  const out: FontPrefs = { ui: '', editor: '', mono: '' }
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) || '{}') as Record<string, unknown> | null
    for (const role of FONT_ROLES) {
      const stack = saved?.[role]
      if (typeof stack === 'string' && STACK_RE.test(stack)) out[role] = familyOf(stack)
    }
  } catch {
    // a corrupted value must not keep the app from starting
  }
  return out
}

/** Applies the fonts to the page and saves the stacks for the next reload (theme-init.js reads them). */
export function applyFonts(prefs: FontPrefs) {
  const stacks = {} as Record<FontRole, string>
  for (const role of FONT_ROLES) {
    stacks[role] = fontStack(role, prefs[role])
    const style = document.documentElement.style
    if (stacks[role]) style.setProperty(`--font-${role}-choice`, stacks[role])
    else style.removeProperty(`--font-${role}-choice`)
  }
  localStorage.setItem(KEY, JSON.stringify(stacks))
}

interface LocalFontData {
  family?: string
}

/**
 * Font families installed on this device (Local Font Access API: Chromium, with permission).
 * Empty when the browser does not have it or the permission is refused; the settings then
 * fall back to typing the name.
 */
export async function listSystemFonts(): Promise<string[]> {
  try {
    const fonts = await window.queryLocalFonts?.()
    if (!fonts) return []
    const names = new Set<string>()
    for (const font of fonts) {
      const name = cleanFontName(font?.family)
      if (name) names.add(name)
    }
    return [...names].sort((a, b) => a.localeCompare(b))
  } catch {
    return []
  }
}

declare global {
  interface Window {
    /** Not in the TypeScript DOM library yet (it asks the user for a permission). */
    queryLocalFonts?: () => Promise<Iterable<LocalFontData>>
  }
}
