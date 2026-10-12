# notas

Leitor de PDF/EPUB + editor de notas (modo vim) sobre o vault markdown do Obsidian, servido por um binário Go.
Os `.md` do vault são a fonte da verdade (sem banco): Neovim, Obsidian e o navegador editam os mesmos arquivos.

## Uso

```sh
make dev        # Go :8080 + Vite :5173 (abra http://localhost:5173)
make build      # gera bin/notas com o front embutido
make install    # instala em ~/.local/bin + unit systemd --user
systemctl --user enable --now notas
make check      # gofmt, vet, staticcheck, errcheck, nilaway, testes -race, svelte-check
```

Flags/variáveis: `-vault` / `NOTAS_VAULT` (padrão `~/Documents/NotasObsidian/Notas`),
`-addr` / `NOTAS_ADDR` (padrão `127.0.0.1:8080`). Para acesso remoto, ponha o IP do NetBird em `NOTAS_ADDR`
no unit; quem está na rede acessa sem login.

## Atalhos

| Atalho | Ação |
|---|---|
| `Ctrl+\` / `Alt+\` / `Ctrl+Shift+\` | recolher notas / leitor / barra lateral |
| `Ctrl+E` | alternar preview / edição |
| `Ctrl+O` / `Ctrl+P` | busca rápida de notas e PDFs (Shift+Enter cria) |
| `Ctrl+Shift+F` | busca no conteúdo (requer ripgrep) |
| `Ctrl+Shift+O` | sumário da nota |
| `[[` | autocompletar links |
| `Ctrl+clique` / `Ctrl+Enter` | seguir `[[link]]`; numa tarefa, alterna `[ ]`/`[x]` |
| `Ctrl+Shift+L` | inserir link da posição atual do leitor (`#page=N` ou `#cfi=…`) |
| `Alt+↑/↓` · `Alt+Shift+↑/↓` | rolar · virar página do leitor sem sair do editor |
| `Tab` / `Shift+Tab` | indentar / desindentar |
| `:w` / `Ctrl+S` | salvar (também salva sozinho após 1 s) |

- Barra lateral: **+** nota, **📁+** pasta (também dentro de cada pasta), **📂** abre PDF/EPUB do
  dispositivo sem enviar nada ao servidor. Clique direito: renomear/mover (atualiza os `[[links]]`) e lixeira
  (`.trash` do vault; nada é apagado).
- Colar ou arrastar uma imagem a grava na pasta de anexos do Obsidian e insere `![[…]]`.
- **↩ N** na barra superior lista os backlinks. Divisórias arrastáveis; duplo clique restaura.

## Aparência

Tema claro "papel" (☾/☀ alterna para o escuro) e EB Garamond embutida. Cores: `web/src/lib/theme.ts` e `web/src/app.css`.

## Estrutura

Go (`net/http`, `os.Root` confina o acesso ao vault, escrita atômica, conflito por mtime, fsnotify + SSE) em
`cmd/` e `internal/`; front Svelte 5 + CodeMirror 6 + PDF.js + foliate-js em `web/`. O `mtime` vai como string
no JSON porque em nanossegundos perde precisão no `Number` do JS. A página tem CSP `script-src 'self'`, que
também bloqueia JavaScript embutido em EPUBs.
