// Applies the saved theme before the page paints (avoids flashing the wrong theme).
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
