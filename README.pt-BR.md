# Quirenote

*[Read in English](README.md)*

Leitor de PDF/EPUB + editor de notas (modo vim) sobre o vault markdown do Obsidian, servido por um binário Go.
Os `.md` do vault são a fonte da verdade (sem banco): Neovim, Obsidian e o navegador editam os mesmos arquivos.

![Editor com o leitor de PDF ao lado](docs/img/editor-and-reader.png)

## Funcionalidades

**Leitor e notas lado a lado.** O PDF (ou EPUB) fica à esquerda e a nota à direita; os dois painéis recolhem e
redimensionam. `Ctrl+Shift+L` insere um link para a página atual, e clicar nele volta exatamente lá.

**Preview do markdown** (`Ctrl+E`): títulos, `[[links]]`, imagens, propriedades e tarefas clicáveis.

![Preview da nota](docs/img/preview.png)

**Busca rápida** (`Ctrl+O`) por nome, sem diferenciar acentos, e **busca no conteúdo** (`Ctrl+Shift+F`) com trechos destacados.

![Busca rápida](docs/img/quick-switcher.png) ![Busca no conteúdo](docs/img/full-text-search.png)

**Backlinks** (botão ↩ N): quais notas apontam para a nota aberta.

![Backlinks](docs/img/backlinks.png)

**Tema escuro** (botão lua/sol no topo):

![Tema escuro](docs/img/dark-theme.png)

## Uso

```sh
make dev        # Go :8080 + Vite :5173 (abra http://localhost:5173)
make build      # gera bin/quirenote com o front embutido
make install    # instala em ~/.local/bin + unit systemd --user
systemctl --user enable --now quirenote
make check      # gofmt, vet, staticcheck, errcheck, nilaway, testes -race, svelte-check
```

Flags/variáveis: `-vault` / `QUIRENOTE_VAULT` (padrão `~/Documents/NotasObsidian/Notas`),
`-addr` / `QUIRENOTE_ADDR` (padrão `127.0.0.1:8080`). Para acesso remoto, ponha o IP do NetBird em `QUIRENOTE_ADDR`
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

- Barra lateral: nova nota, nova pasta (também dentro de cada pasta) e abrir PDF/EPUB do
  dispositivo sem enviar nada ao servidor. Clique direito: renomear/mover (atualiza os `[[links]]`) e lixeira
  (`.trash` do vault; nada é apagado).
- Colar ou arrastar uma imagem a grava na pasta de anexos do Obsidian e insere `![[…]]`.
- A interface do app é em inglês.
- **↩ N** na barra superior lista os backlinks. Divisórias arrastáveis; duplo clique restaura.

## App de desktop (Omarchy)

`make install` instala `quirenote-app`, o atalho "Quirenote" do launcher e o ícone. `quirenote-app` abre o Quirenote numa janela
de app do Chromium (sem barra de endereço) com um perfil próprio e leve, sobe o serviço systemd se ele estiver parado e, se a janela já
existir, só foca nela. No Hyprland: `SUPER+SHIFT+L` (em `~/.config/hypr/bindings.lua`:
`o.bind("SUPER + SHIFT + L", "Quirenote", { launch = "quirenote-app" })`). O app também pode ser instalado como PWA.

## Aparência

Tema claro "papel" (o botão lua/sol alterna para o escuro) e EB Garamond embutida. Cores: `web/src/lib/theme.ts` e `web/src/app.css`.

## Estrutura

Go (`net/http`, `os.Root` confina o acesso ao vault, escrita atômica, conflito por mtime, fsnotify + SSE) em
`cmd/` e `internal/`; front Svelte 5 + CodeMirror 6 + PDF.js + foliate-js em `web/`. O `mtime` vai como string
no JSON porque em nanossegundos perde precisão no `Number` do JS. A página tem CSP `script-src 'self'`, que
também bloqueia JavaScript embutido em EPUBs.

Ícones: [Lucide](https://lucide.dev) (licença ISC), herdam a cor do tema.
