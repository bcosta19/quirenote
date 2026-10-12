// Applies the saved theme and fonts before the page paints (avoids flashing the wrong look).
// External file, not inline: the page CSP only allows the app's own scripts.
// One-time migration of preferences from the old name (notas:*) to quirenote:*.

for (const k of Object.keys(localStorage)) {
  if (k.startsWith('notas:')) {
    const nk = 'quirenote:' + k.slice(6)
    if (localStorage.getItem(nk) === null) localStorage.setItem(nk, localStorage.getItem(k))
    localStorage.removeItem(k)
  }
}
document.documentElement.dataset.theme = localStorage.getItem('quirenote:theme') === 'dark' ? 'dark' : 'light'

// Custom fonts: the app saves a ready-made CSS font list per role. Only what the app itself
// would write is accepted (same test as web/src/lib/fonts.ts), so a hand-edited value cannot
// inject CSS.
try {
  const fonts = JSON.parse(localStorage.getItem('quirenote:fonts') || '{}')
  const stack_re = /^(?:'[\p{L}\p{N} ._-]{1,64}'|[a-z][a-z0-9-]{0,20}), var\(--font-(?:ui|editor|mono)-tail\)$/u
  for (const role of ['ui', 'editor', 'mono']) {
    const stack = fonts?.[role]
    if (typeof stack === 'string' && stack_re.test(stack))
      document.documentElement.style.setProperty(`--font-${role}-choice`, stack)
  }
} catch {
  // corrupted fonts: keep the defaults
}
