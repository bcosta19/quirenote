# notas

Leitor de PDF + editor de notas (vim mode) sobre um vault markdown do Obsidian, servido por um binário Go.
Veja o [MVP.md](MVP.md) para escopo e decisões.

## Uso

```sh
make dev        # Go :8080 + Vite :5173 (abra http://localhost:5173)
make build      # gera bin/notas com o front embutido
make install    # instala em ~/.local/bin + unit systemd --user
systemctl --user enable --now notas
make check      # gofmt, vet, staticcheck, errcheck, nilaway, testes -race, svelte-check
```

Flags/variáveis: `-vault` / `NOTAS_VAULT` (padrão `~/Documents/NotasObsidian/Notas`),
`-addr` / `NOTAS_ADDR` (padrão `127.0.0.1:8080`).

## Atalhos

| Atalho | Ação |
|---|---|
| `Ctrl+\` | recolher/mostrar notas |
| `Ctrl+E` | alternar preview do markdown / edição (como no Obsidian) |
| `Alt+\` | recolher/mostrar o PDF (sem fechar; volta na mesma página) |
| `Ctrl+Shift+\` | barra lateral |
| `Tab` / `Shift+Tab` | indentar / desindentar (inclusive itens de lista) |
| `Ctrl+Shift+L` | inserir link da página atual do PDF |
| `Alt+↑` / `Alt+↓` | rolar o PDF sem sair do editor |
| `Alt+Shift+↑` / `Alt+Shift+↓` | página anterior / próxima do PDF |
| `Ctrl+clique` / `Ctrl+Enter` | seguir `[[link]]` (se a nota não existir, oferece criar) |
| `[[` | autocompletar links (Enter ou Tab aceita) |
| `Ctrl+O` / `Ctrl+P` | busca rápida de notas e PDFs (Shift+Enter cria) |
| `:w` / `Ctrl+S` | salvar (também salva sozinho após 1 s) |

Na barra lateral: **+** cria uma nota e **📁+** cria uma pasta (passe o mouse numa pasta para criar
nota ou subpasta direto nela);
**📂** abre um PDF do dispositivo em que você está, sem enviar nada para o servidor.
Links para esse PDF são gravados só com o nome (`[[livro.pdf#page=N]]`) e funcionam enquanto ele estiver aberto.

Arraste as divisórias (entre barra lateral, PDF e notas) para redimensionar; duplo clique restaura.
Enter numa lista continua com `- `, `1.` ou `- [ ]`; Enter num item vazio sai da lista. Indentação com TAB, como no Obsidian.

## Aparência

Tema claro "papel" por padrão (☾/☀ no topo alterna para o escuro; a escolha fica salva por dispositivo).
O editor usa **EB Garamond**, embutida no binário (igual em qualquer dispositivo); código fica em mono.
Cores do editor: `web/src/lib/theme.ts`; cores da interface: `web/src/app.css`.
`docs/fontes.html` é a prévia usada para escolher a fonte.
