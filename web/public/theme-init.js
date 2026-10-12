// Aplica o tema salvo antes de a página desenhar (evita piscar no tema errado).
// Arquivo externo, não inline: a CSP da página só permite scripts do próprio app.
document.documentElement.dataset.theme = localStorage.getItem('notas:theme') === 'dark' ? 'dark' : 'light'
