# Correção do travamento ao iniciar no Wayland

A GUI podia deixar de responder logo após abrir em um workspace oculto. A thread principal esperava dentro de `libEGL_mesa` e `wl_display_dispatch_queue`, impedindo o processamento de eventos do compositor. A captura foi feita numa reprodução com nota sintética sob GDB, sem alterar o `ptrace_scope` do sistema. O executável release é stripped: os frames das bibliotecas são identificáveis, mas os frames internos Rust não estão simbolizados.

O eframe 0.36.2 configurava o swap EGL com intervalo 1 por padrão. Quando o compositor retém callbacks de frame de uma janela fora do workspace visível, essa espera pode bloquear o event loop. O código atual upstream também descreve swap com intervalo 0 no Wayland: [eframe/glow_integration.rs](https://github.com/emilk/egui/blob/main/crates/eframe/src/native/glow_integration.rs). A correção local usa `glow_options.vsync = false` ao selecionar Wayland e preserva o padrão nos demais backends. Nenhuma dependência foi atualizada.

A seleção considera `WAYLAND_DISPLAY` e `WAYLAND_SOCKET`, como o winit usado pelo projeto. Não considera `WINIT_UNIX_BACKEND`, removida nessa biblioteca antes da versão instalada. O restante da configuração da janela foi preservado. O commit de código é `734dc9411d02a66623838f2c9ae607f1052f7d31`.

## Regressão reproduzível

`scripts/verify-wayland-liveness.py` usa backend e vault sintéticos e exige um compositor Sway privado. A configuração deve atribuir a GUI ao workspace 1 e incluir `no_focus [app_id="quirenote-gui"]`. O teste permanece no workspace 2, confirma que a janela está oculta, envia o pedido Wayland de fechamento por IPC e exige saída normal em até dois segundos. Não executa comandos no compositor do usuário.

```sh
make check-wayland \
  WAYLAND_ENV=/path/to/private/env.sh \
  SWAYMSG=/path/to/swaymsg \
  WAYLAND_RESULTS=.claude/wayland-liveness
```

O compositor privado deve estar ativo, com GPU/EGL disponível e sem `no_focus` no desktop do usuário. O arquivo de ambiente exporta `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`, `SWAYSOCK` e, se necessário, `LD_LIBRARY_PATH`; deve referir-se ao runtime privado.

- **Antes:** o release anterior não processou o evento de fechamento em dois segundos.
- **Depois:** três rodadas passaram, com fechamento normal em **33 ms, 17 ms e 17 ms**.
- Três testes unitários adicionais verificam seleção do backend, configuração de swap Wayland e preservação do padrão nos outros backends.

[Resultado anterior](wayland-startup-fix-2026-10-14/regression-before.json), [rodada 1](wayland-startup-fix-2026-10-14/regression-after-0.json), [rodada 2](wayland-startup-fix-2026-10-14/regression-after-1.json), [rodada 3](wayland-startup-fix-2026-10-14/regression-after-2.json), [pilha capturada](wayland-startup-fix-2026-10-14/main-stack.txt).

## Checks e instalação

`make check` passou com **76 testes** (38 unitários backend, 5 API e 33 GUI), rustfmt, Clippy em todos os targets com warnings tratados como erros e sintaxe shell. O script de regressão também passou na análise sintática Python. `make build` release offline/locked passou; a GUI compilou em 3 min 04 s.

Cinco fluxos nativos passaram em sessão privada: edição/autosave, restauração da nota, alteração externa seguida de edição preservando conteúdo, PDF na página 4 e EPUB com posição de leitura e preview. As capturas de PDF e EPUB foram inspecionadas.

O executável instalado em `~/.local/bin/quirenote-gui` foi substituído atomicamente, preservando o anterior em backup local. A nova aplicação foi aberta por `gtk-launch quirenote`, pelo desktop entry usado pelo Omarchy. O PID observado foi 1953788, com hash igual ao release corrigido e `xwayland=false`; a observação de 12,53 s não registrou diálogo de falta de resposta. A janela foi colocada no workspace 2. O backend e a configuração do vault não foram alterados nesta atualização.

SHA256 da GUI: `6afc3abe84c6f9f928ed65138505b6b25fe449fcdd724f9f6030994f38122880`. O backup da GUI anterior está indicado no inventário local `.claude/gui-startup-diagnostic/installation.json`.

[Verificação estruturada](wayland-startup-fix-2026-10-14/verification.json), [check completo](wayland-startup-fix-2026-10-14/check-after.log), [build](wayland-startup-fix-2026-10-14/build-after.log), [fluxos nativos](wayland-startup-fix-2026-10-14/native-flow-after/result.json), [execução instalada](wayland-startup-fix-2026-10-14/installed-runtime.json).

A regressão usa Mesa/Wayland desta máquina; não é uma medição nova de RAM nem uma validação de todas as combinações de driver e compositor. X11 mantém sua configuração anterior e teve essa seleção coberta em teste unitário. Os compositores e processos de teste foram encerrados ao final.
