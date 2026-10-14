> Relatório histórico do benchmark anterior à promoção para main. As correções de relink, formatação e Clippy posteriores estão descritas na documentação da versão Rust; os números abaixo se referem aos binários medidos naquela rodada.

# Memória da interface nativa Rust — 14/10/2026

A interface egui consumiu entre **80.1% e 84.8% menos PSS residente** que a interface Svelte em Chromium, comparando os clientes com o mesmo servidor Rust. A campanha aceita contém **45 medições**, três repetições de cinco cenários em três conjuntos. O ganho maior vem da troca da interface e do runtime gráfico; o servidor continua sendo uma parcela pequena do total.

## Resultado

Medianas das três repetições, em MiB de PSS residente do cliente. Chromium inclui toda a árvore de processos e os crash handlers vinculados ao perfil temporário. O servidor está separado nesta tabela.

| Cenário | Go + Chromium | Rust + Chromium | Rust + GUI | Economia da GUI* |
|---|---:|---:|---:|---:|
| Sem arquivo aberto | 472.5 | 468.3 | 73.1 | 84.4% |
| Nota | 487.6 | 489.4 | 74.8 | 84.7% |
| Nota + PDF | 528.4 | 530.8 | 80.5 | 84.8% |
| Nota + EPUB | 496.6 | 495.4 | 79.9 | 83.9% |
| Navegação e preview | 605.4 | 587.6 | 116.8 | 80.1% |

*Economia calculada entre as medianas de Rust + Chromium e Rust + GUI, mantendo o backend Rust. PSS divide as páginas compartilhadas entre os processos que as mapeiam. Somar RSS do Chromium contaria essas páginas mais de uma vez.

Na nota com PDF, a mediana incluindo servidor foi **548.4 MiB** em Go + Chromium e **84.3 MiB** em Rust + GUI.

Os clientes medidos tiveram medianas de Swap PSS próximas de zero. Houve uma amostra de navegação do Chromium com Rust com 73.6 MiB de Swap PSS; por isso a memória residente e a parcela em swap estão publicadas separadamente. Considerando PSS residente + Swap PSS, a mediana desse cenário foi 602.7 MiB no Chromium e 116.8 MiB na GUI.

[Medianas, mínimos e máximos](results/2026-10-14-clean/summary.md), [resumo estruturado](results/2026-10-14-clean/summary.json), [dados brutos](results/2026-10-14-clean/ram-2026-10-13.json) e [status](results/2026-10-14-clean/status.json).

## Retomada e correções

A sessão do Claude Code localizada no ai-memory foi `e2f983e3-da4b-4515-b053-6d38fa5effaa`. A worktree correta é `.claude/worktrees/rust-gui`, branch `bench/rust-gui`, derivada do benchmark de backend. O último commit da interface era `bbe54a9`, depois de `8b93cb3`, `774bf2b` e `9c79f97`. A sessão registrou 28 testes aprovados e uma recompilação release interrompida por pressão de memória. Os scripts em `bench/gui/` ainda estavam sem versionamento e não havia uma campanha de memória concluída.

O build foi retomado. A inspeção da navegação revelou dois defeitos no leitor nativo: a largura podia ser ajustada antes de chegarem as dimensões do PDF, produzindo uma escala inválida e páginas brancas; e a mudança de página por atalho podia deixar a sessão e o link de página com o valor anterior. O leitor agora refaz o ajuste após carregar as dimensões e comunica a página alterada entre frames. Duas regressões reproduziram as falhas antes da correção e passaram depois. O build medido inclui essa alteração local em `gui/src/pdf.rs`. Os hashes dos fontes e binários estão nos metadados.

## Verificação

- **30 testes da GUI aprovados**, incluindo as duas regressões do PDF.
- **Build release aprovado**; Clippy terminou com código 0 e 86 avisos. Warnings não foram tratados como erros.
- **Três verificações no aplicativo nativo real aprovadas:** edição/autosave, restauração da nota ao abrir outra janela e alteração externa por SSE seguida de outra edição que preserva os conteúdos anteriores.
- **163 arquivos do frontend conferidos byte a byte** nas respostas HTTP dos binários Go e Rust, iguais ao mesmo `web/dist`.
- **Nove navegações aceitas:** todas têm um checkpoint de EPUB aberto com posição de leitura e terminaram com o PDF na página 14. As capturas mostram o conteúdo renderizado.
- **Formatação padrão reprovada:** a GUI existente ainda não segue `cargo fmt --check`. Essa pendência foi preservada, junto dos avisos do Clippy.

[Evidência dos checks](results/verification/verification.json), [fluxo nativo](results/native-flow-fixed/result.json), [frontend servido](results/frontend-verification.json), [EPUB nativo durante a navegação](results/2026-10-14-clean/rust+gui-workout-rep0.epub.png), [PDF após a navegação](results/2026-10-14-clean/rust+gui-workout-rep0.png).

## Método

- Processos novos, cópia nova do mesmo vault e preferências novas por medição; perfis dedicados do Chromium, extensões desativadas e janela `--app`.
- Vault sintético de 200 notas geradas, nota de demonstração e anexos de teste, totalizando 217 arquivos. PDF de 24 páginas e EPUB de seis capítulos.
- Compositor Wayland privado, 1920×1080, escala 1.25, renderer GLES2, sem alterar a sessão do desktop.
- Mesmo frontend servido por ambos os backends. Go corrigido da branch, derivado de `aa3cae5`; GUI baseada em `bbe54a9` com a correção local do PDF.
- Ordem Go/Chromium, Rust/Chromium, Rust/GUI; invertida na segunda repetição.
- Oito segundos iniciais, validação da tela, três segundos de estabilização, três amostras de memória com intervalo de um segundo; a amostra com PSS mediano fornece RSS, USS e swap do cliente. O servidor é medido separadamente ao final do repouso.
- O cenário de navegação alterna preview, notas, busca, outline, EPUB e PDF. Um checkpoint confirma o EPUB aberto e a posição de leitura antes da navegação seguinte. Ele não mede throughput de edição ou salvamento.
- go version go1.27.0-X:nodwarf5 linux/amd64; rustc 1.98.1 (48a229cea 2026-09-01) (Arch Linux rust 1:1.98.1-1); Chromium 152.0.7977.82 Arch Linux; kernel 7.2.5-3-omarchy. CPU Ryzen 7 7735HS.

[Parâmetros e hashes](results/2026-10-14-clean/metadata.json), [ambiente](results/2026-10-14-clean/environment.json), [reprodução](README.md). Os scripts também verificam a configuração temporária do vault para que a tela inicial de setup não seja confundida com o aplicativo aberto. `--resume` valida binários, fontes da GUI, frontend, vault e parâmetros antes de reaproveitar amostras completas.

## Tentativas excluídas

A primeira campanha parcial, de 25 amostras, foi interrompida antes de limpar a tmpfs e corrigir o PDF; não participa das medianas. Depois, duas tentativas com pausas fixas não registraram posição do EPUB, uma em Chromium + Go e outra em Chromium + Rust. A causa dessas duas falhas não foi isolada. Os sete resultados completos anteriores desse cenário também foram separados, e as nove navegações foram refeitas com confirmação intermediária da abertura do EPUB. A etapa estática manteve as 36 amostras, com os mesmos binários, vault e parâmetros.

[Campanha prévia excluída](results/rejected-before-cleanup/STATUS.md), [duas tentativas rejeitadas](results/2026-10-14-clean/rejected-attempts.json), [sete navegações substituídas](results/2026-10-14-clean/fixed-wait-workout-superseded.json). Uma inicialização lenta do Chromium também interrompeu o primeiro lançamento da campanha aceita; o controlador passou a aguardar sua porta de inspeção e retomou as amostras com os mesmos hashes.

## RAM e /tmp

`/tmp` é tmpfs nesta máquina, com limite de cerca de 2.8 GiB. Estava em 80% de ocupação, com 2.2 GiB usados. Os maiores diretórios deste projeto eram o cache Go em `/tmp/quirenote-review-gocache` (881 MiB) e `/tmp/qbench` (899 MiB). Dados de tmpfs podem ocupar RAM e swap; o limite não é uma reserva de RAM e o uso de swap do sistema não pertence integralmente à tmpfs.

Com autorização do usuário, o cache Go e o build antigo `/tmp/qbench/corrected-ebt2nj_9/target` (295 MiB) foram movidos para `/home/brspike/Projects/notas/.claude/tmp-archive-2026-10-14/`, em Btrfs. Os arquivos permanecem recuperáveis; a contagem e o total de bytes foram conferidos após cada movimento. A ocupação caiu para cerca de 37–40%, liberando aproximadamente 1.15 GiB da tmpfs. Imediatamente após a movimentação, havia cerca de 1.5 GiB de RAM disponível e 5.8 GiB de swap usado, contra aproximadamente 869 MiB e 6.8 GiB no levantamento anterior.

[Inventário da movimentação e snapshots](results/verification/tmp-cleanup.json). Os grandes caches de compilação devem ficar no disco nesta máquina; a reprodução usa um cache Go dentro da worktree.

## Limites

Uma máquina, desktop ativo, três repetições, vault pequeno e sessões novas. Não são medidas de pico durante todo o uso, nem um teste de vazamento ou de sessão longa. GPU fora de `smaps_rollup`, compositor e serviços do sistema não estão no PSS do cliente. Os engines de PDF/EPUB e as políticas de cache também são diferentes; o resultado compara as arquiteturas completas dessas interfaces.

A equivalência completa das telas e dos fluxos não foi estabelecida. As capturas ainda mostram diferenças de geometria e reflow, e os atalhos de ajuda na tela vazia da GUI podem ficar cortados na borda direita. A falha de relink concorrente do backend Rust, documentada na comparação de backend, continua fora desta rodada. Os resultados sustentam a economia de memória neste experimento; a interface permanece experimental.
