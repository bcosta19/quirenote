# Quirenote

*[Read in English](README.md)*

Aplicação nativa em Rust para notas Markdown, com leitor de PDF/EPUB ao lado. A interface usa egui e o backend usa Axum. As notas continuam sendo arquivos comuns na pasta escolhida, compatíveis com links, anexos e lixeira do Obsidian.

![Interface nativa com nota e PDF](docs/gui-memory-2026-10-14/results/2026-10-14-clean/rust+gui-note+pdf-rep0.png)

## Compilar e executar

Requer Rust atual, Linux, ripgrep, fontconfig e bibliotecas de desenvolvimento Poppler GLib, Cairo, GLib, xkbcommon e Wayland/X11. No Arch, os pacotes correspondentes incluem `rust`, `ripgrep`, `fontconfig`, `poppler-glib`, `cairo`, `glib2`, `libxkbcommon`, `wayland`, `libx11` e `libxrandr`. O primeiro build baixa as dependências fixadas no Cargo.lock; com cache pronto, use `CARGO_FLAGS="--offline --locked"`.

```sh
make check       # rustfmt, Clippy estrito, testes do backend/GUI, sintaxe shell
make build       # release: bin/quirenote e bin/quirenote-gui
make dev         # backend e janela nativa debug; encerra seu backend ao fechar
make install     # instala os dois binários Rust, lançador, ícone e serviço de usuário
quirenote-app    # inicia o serviço local se necessário e abre a interface nativa
```

A compilação usa um job por padrão para limitar RAM. É possível definir `CARGO_JOBS`, `SERVER_TARGET_DIR` e `GUI_TARGET_DIR`. `make dev-api` e `make dev-gui` executam os componentes separadamente. Reinicie o modo de desenvolvimento após alterar os fontes.

Sem instalar:

```sh
bin/quirenote -addr 127.0.0.1:8080
# Em outro terminal:
bin/quirenote-gui --server http://127.0.0.1:8080
```

## Notas e configuração

Na primeira abertura, escolha uma pasta nova ou um vault existente. A escolha fica em `~/.config/quirenote/config.toml`; os arquivos existentes são usados no lugar, sem cópia. Depois de configurar, troque a pasta pelo terminal:

```sh
bin/quirenote init ~/Notas
bin/quirenote init
```

Backend: `-vault` / `QUIRENOTE_VAULT`, `-addr` / `QUIRENOTE_ADDR`. Interface: `--server` / `QUIRENOTE_URL`, `--note`, `--doc`, `--page`, `--state-dir`. Preferências e sessão ficam em `$XDG_STATE_HOME/quirenote-gui`, normalmente `~/.local/state/quirenote-gui`. A porta HTTP serve a API; acesso remoto requer o cliente nativo e acesso à rede do backend. A main não oferece interface de navegador nem autenticação na API.

## Atalhos

| Atalho | Ação |
|---|---|
| `Ctrl+O` / `Ctrl+P` | abrir nota, PDF ou EPUB |
| `Ctrl+Shift+F` | busca no conteúdo |
| `Ctrl+Shift+O` | títulos da nota |
| `Ctrl+E` | edição / preview Markdown |
| `Ctrl+S` / `:w` | salvar (também há autosave) |
| `Ctrl+Shift+L` | inserir link para a posição de leitura |
| `Alt+↑/↓` / `Alt+Shift+↑/↓` | rolar / virar página do leitor |
| `Ctrl+\` / `Alt+\` / `Ctrl+Shift+\` | recolher notas / leitor / árvore |

Editor com movimentos Vim, wikilinks, imagens, desfazer e refazer. A barra permite escolher tema e fontes por função. O PDF usa Poppler; EPUB e preview Markdown são desenhados pela interface nativa.

## Estrutura e backup

`rust/`: vault confinado, salvamento atômico, conflitos de versão, configuração inicial, busca, atualização de links e eventos. `gui/`: janela, fontes, editor e leitores nativos. `deploy/` e Makefile compilam, instalam e iniciam os dois binários Rust.

A versão anterior Go/Svelte está preservada em **`backup/go-svelte-2026-10-14`**. Abra outra worktree para consultar ou compilar o backup sem substituir esta main. O benchmark de backend fica em `bench/rust-axum-vs-go`; o [relatório de memória da GUI](docs/gui-memory-2026-10-14/RELATORIO.pt-BR.md) registra redução de 80–85% no PSS do cliente nesta máquina. A medição compara arquiteturas, não comprova superioridade universal da linguagem nem equivalência completa de funcionalidades.

Ainda existem diferenças de geometria e reflow EPUB; atalhos de ajuda podem cortar em larguras estreitas. Ao renomear, a atualização de links lê todas as notas visíveis e salva com verificação de versão; uma edição conflitante é informada como ignorada, preservando seu conteúdo.

Fontes EB Garamond (OFL) e ícones Lucide (ISC), com licenças em `gui/assets/fonts/`.
