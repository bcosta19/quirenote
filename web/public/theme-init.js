// Aplica o tema salvo antes de a página desenhar (evita piscar no tema errado).
// Arquivo externo, não inline: a CSP da página só permite scripts do próprio app.
// Migração única das preferências do nome antigo (notas:*) para quirenote:*.
for (const k of Object.keys(localStorage)) {
  if (k.startsWith('notas:')) {
    const nk = 'quirenote:' + k.slice(6)
    if (localStorage.getItem(nk) === null) localStorage.setItem(nk, localStorage.getItem(k))
    localStorage.removeItem(k)
  }
}
document.documentElement.dataset.theme = localStorage.getItem('quirenote:theme') === 'dark' ? 'dark' : 'light'
