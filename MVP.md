# Notas — MVP

## Problema
Anotar em qualquer lugar (Neovim, Obsidian, navegador de outro dispositivo) sobre o **mesmo vault markdown**,
e estudar com o PDF/EPUB de um lado e as notas do outro.

## Princípio central
**Os arquivos `.md` do vault são a fonte da verdade.** Sem banco de dados. O app web lê e escreve
os mesmos arquivos que o Neovim (`alias notas`) e o Obsidian. As notas continuam válidas nos três.

## Escopo (MoSCoW)

| | Item | Status |
|---|---|---|
| Must | Árvore do vault + filtro | ✅ |
| Must | Editor markdown com vim mode (CodeMirror 6), autosave, `:w`/`Ctrl+S` | ✅ |
| Must | Detecção de conflito com edições externas (mtime) + recarga automática via SSE | ✅ |
| Must | Leitor de PDF (PDF.js) com texto selecionável, zoom e memória da última página | ✅ |
| Must | Painel de notas recolhível (`Ctrl+\`) | ✅ |
| Must | Inserir link da página atual `[[livro.pdf#page=N]]` (`Ctrl+Shift+L`) | ✅ |
| Must | Seguir `[[link]]` com Ctrl+clique / `Ctrl+Enter` (resolve como o Obsidian) | ✅ |
| Must | Abrir PDF do dispositivo (📂), sem upload, com links por nome do arquivo | ✅ |
| Must | Criar nota direto numa subpasta (+ ao passar o mouse na pasta) | ✅ |
| Must | Criar pastas e subpastas pela barra lateral | ✅ |
| Must | Edição estilo processador de texto (Tab, listas, numeração por linha visual) | ✅ |
| Must | Recolher o PDF e redimensionar painéis com o mouse | ✅ |
| Must | Binário único com o front embutido + unit systemd | ✅ |
| Should | Leitor EPUB (foliate-js) com link por CFI | |
| Should | Busca no conteúdo (`rg --json`) | |
| Should | Divisor arrastável entre leitor e notas | ✅ |
| Should | Preview do markdown renderizado (imagens `![[...]]`, tarefas, propriedades) | ✅ |
| Could | Destaques (highlights) no PDF salvos no vault | |
| Could | Auto-commit git do vault (cuidado: o vault já é sincronizado pelo Syncthing) | |
| Could | PWA (instalar no celular) | |
| Could | Web clipper próprio (extensão/bookmarklet → `PUT /api/notes`) | |
| Later | IA sobre os PDFs (resumo, perguntas): só se virar necessidade real | |

## Stack
- **Go (stdlib `net/http`)**: API fina de arquivos, binário único e leve, roda como serviço no notebook.
  - `os.Root` impede que caminhos vindos do navegador escapem do vault (`../`, symlinks).
  - Escrita atômica (temporário + `rename`), segura com Syncthing e Neovim.
  - `fsnotify` + SSE avisam o navegador quando algo muda no disco.
  - `http.ServeContent` atende range requests (PDF.js carrega por partes).
- **Svelte 5 + TypeScript (Vite)**: toda a interface.
  - CodeMirror 6 + `@replit/codemirror-vim` + tema One Dark.
  - `pdfjs-dist` (PDFViewer com camada de texto, então extensões de dicionário/tradução funcionam).

## Arquitetura
```
 Neovim (alias notas) ─┐
 Obsidian ─────────────┼──► ~/Documents/NotasObsidian/Notas  (Syncthing)
                       │            ▲
 Navegador ── NetBird ─┴──► notas (Go, systemd --user)
                              ├─ GET  /api/tree
                              ├─ GET  /api/notes/{path}     → {path, content, mtime}
                              ├─ PUT  /api/notes/{path}     ← {content, mtime}  (409 = conflito)
                              ├─ GET  /api/files/{path}     (PDF/EPUB, com Range)
                              ├─ GET  /api/events           (SSE: mudanças no disco)
                              └─ GET  /*                    (front embutido)
```
O `mtime` vai como **string** no JSON: em nanossegundos ele passa de 2^53 e o `Number` do JS perde precisão.

## Ambientes e deploy
- **Dev:** `make dev` (Go em `:8080` + Vite com hot reload em `:5173`).
- **Produção (o notebook):** `make install` e depois `systemctl --user enable --now notas`.
- **Acesso remoto:** NetBird. Trocar `NOTAS_ADDR` no unit pelo IP do notebook na rede NetBird
  (`netbird status`). Quem está na rede NetBird acessa sem login, então use as políticas de acesso do NetBird.
- HTTPS (necessário só para PWA/clipboard): Caddy na frente, ou Tailscale (`tailscale serve`) se trocar de VPN.

## Decisões adiadas (e o sinal para revisitar)
- **Auth própria:** só se o app sair da VPN.
- **Banco/índice (SQLite FTS):** só se o `rg` ficar lento.
- **Tipos compartilhados Go↔TS:** espelhados à mão em `web/src/lib/api.ts` enquanto forem poucos.
- **Notebook dormindo = app fora do ar:** migrar para mini PC/VPS é copiar binário + pasta.

## Próximos passos
1. Usar no dia a dia por uma semana com PDFs reais e anotar o que incomoda.
2. EPUB (foliate-js).
3. Busca (`rg`).
4. Divisor arrastável + preview de imagens.
