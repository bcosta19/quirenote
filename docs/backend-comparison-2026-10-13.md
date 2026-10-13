# Go corrigido versus Rust — 13/10/2026

As duas falhas reproduzidas de integridade foram corrigidas no backend Go. Nesta campanha,
Go e Rust preservaram todas as edições aceitas nos cenários de salvamento concorrente,
movimento e lixeira. Rust manteve vantagens de desempenho e de memória em repouso;
Go consumiu menos memória na carga mista. Uma verificação adicional de relink falhou
em duas das quatro rodadas do Rust: algumas referências antigas ficaram fora do processamento.

Os resultados completos estão em [dados brutos](backend-comparison-2026-10-13/raw.json),
[carga mista](backend-comparison-2026-10-13/soak.json) e
[metadados](backend-comparison-2026-10-13/metadata.json).

## Correções no Go

- `Vault.Read` abre o arquivo uma vez e lê conteúdo e `mtime` pelo mesmo descritor.
  Um salvamento que substitui o inode durante a leitura deixa de associar conteúdo antigo
  à versão do arquivo novo. O acesso continua passando por `os.Root`.
- No Linux, movimento e lixeira usam `renameat2(RENAME_NOREPLACE)`, com descritores dos
  diretórios pais abertos por `os.Root` e somente nomes finais passados ao syscall.
  Um destino criado entre a verificação e o rename resulta em conflito, preservando o arquivo.
  Se o kernel/filesystem não suportar essa operação, retorna erro; não usa rename comum como fallback.
- A lixeira tenta outro nome quando uma criação externa ocupa o destino escolhido.
- `Create` compartilha o mutex de escrita. Isso também fecha a corrida entre criações e
  movimentos da própria aplicação no caminho portátil. Fora do Linux, a limitação anterior
  contra criações externas permanece; Windows e macOS tiveram somente compilação verificada.
- `golang.org/x/sys`, que já estava no lock na versão `v0.13.0`, passou a dependência direta.

Código: [leitura e operações do vault](../internal/vault/vault.go),
[rename Linux](../internal/vault/rename_linux.go),
[caminho portátil](../internal/vault/rename_other.go) e
[regressões](../internal/vault/regression_test.go).

## Validação

As novas regressões de leitura e criação versus movimento falharam no código anterior
e passaram após a correção. Elas verificam a associação entre conteúdo e versão durante
substituições do arquivo e a preservação dos conteúdos quando criação e movimento disputam
um destino. A regressão da lixeira também verifica uma criação externa concorrente.

- `make check`: passou, incluindo vet, Staticcheck, Errcheck, NilAway, testes Go com race
  detector e Svelte/TypeScript. Para executar sem rede e sem escrever no cache pessoal,
  foram usados `GOPROXY=file:///home/brspike/go/pkg/mod/cache/download`,
  `GOCACHE=/tmp/quirenote-review-gocache` e `XDG_CACHE_HOME=/tmp/quirenote-review-cache`.
  As versões das ferramentas vieram do cache local.
- Build de produção do frontend: passou.
- Regressões novas e stress de save/lixeira: 30 repetições com race detector em tmpfs.
- As três novas regressões: mais três repetições com race detector em Btrfs.
- Compilação dos testes do vault: Windows/amd64 e macOS/arm64; não foram executados nesses sistemas.
- Rust: 36 testes unitários e quatro testes de API, Clippy com warnings proibidos e formatação passaram.

Os builds medidos também foram executados contra o Chromium e contra a suíte HTTP real,
com configurações e vaults sintéticos. [Resumo da verificação](backend-comparison-2026-10-13/verification.json).

| Verificação | Go corrigido | Rust |
|---|---:|---:|
| Etapas do frontend real, incluindo autosave, reload e SSE | 9/9 | 9/9 |
| Invariantes da suíte HTTP | 46/46 | 46/46 |
| Casos de configuração inicial | 33 | 33 |
| Invariantes da configuração inicial | 3/3 | 3/3 |
| Reprodução de perda de edições, cinco rodadas | 0 perdas | 0 perdas |
| Reprodução de criação versus movimento, 200 rodadas | 0 sobrescritas | 0 sobrescritas |

A comparação encontrou 374 de 378 casos HTTP idênticos e quatro diferenças já documentadas
no experimento: multirange, listagem de diretório de assets, URI muito longa e texto de erro
ao salvar sobre um diretório com nome terminado em `.md`. Os 33 casos de configuração inicial
foram idênticos. [Diff HTTP](backend-comparison-2026-10-13/contract-diff.txt),
[diff de setup](backend-comparison-2026-10-13/setup-diff.txt).

## Metodologia da comparação

- Go: checkout `main`, base `bee0c18`, com as correções locais; build com
  `go build -trimpath -ldflags='-s -w'`. Rust: código da branch `bench/rust-axum-vs-go`,
  commit `6aa4c7f`, copiado para staging temporário; build release com o perfil original
  (`lto=fat`, `codegen-units=1`, `opt-level=3`, `strip=true`).
- Go `1.27.0-X:nodwarf5`, Rust `1.98.1`, Linux `7.2.5-3-omarchy`, Ryzen 7 7735HS.
  [Versões e hashes das fontes Go](backend-comparison-2026-10-13/inputs.json).
- Os dois builds incorporaram o mesmo frontend: 163 arquivos, conferidos byte a byte antes
  da compilação. [Manifesto do frontend](backend-comparison-2026-10-13/frontend-manifest.json).
- Mesmo gerador de carga Go e mesmos datasets determinísticos: 200 notas no pequeno;
  10.000 notas e 120 anexos no grande, totalizando 23,2 MB. Os manifestos registram o hash
  do conteúdo: [pequeno](backend-comparison-2026-10-13/manifest-small.json),
  [grande](backend-comparison-2026-10-13/manifest-large.json).
- Um processo novo e uma cópia nova do vault por medição; HOME e configuração temporários;
  somente loopback e portas 9130–9189. Servidores nos CPUs lógicos 0–7 e cliente nos 8–15.
- Quatro repetições por cenário, ordem Go/Rust, Rust/Go, Go/Rust, Rust/Go.
  Leituras e escritas por cinco segundos; leituras com um segundo de aquecimento.
  Foram concluídas 128 medições, incluindo cenários de um cliente e de alta concorrência.
- Duas cargas mistas de 90 segundos por backend, também com ordem invertida entre rodadas.
  Cada uma usa 32 leitores de notas, dois leitores da árvore, quatro buscas, quatro leitores
  de backlinks e oito escritores. Registra RSS a cada dois segundos, pico, RSS cinco segundos
  depois da carga e resultados de cada gerador, incluindo erros e operações concluídas.
- Overrides de GOMAXPROCS, GOMEMLIMIT, GOGC, allocator e coarse-mtime foram removidos apenas
  do ambiente dos processos da comparação; os runtimes usaram os defaults.

## Desempenho

Mediana das quatro execuções, seguida pelo mínimo–máximo. Taxas em requisições por segundo.

| Cenário | Go corrigido | Rust |
|---|---:|---:|
| Escrita em notas distintas, 64 clientes | 5.838 (5.158–6.788) | 12.917 (11.359–15.016) |
| Leitura de nota, 64 clientes | 38.983 (36.932–44.892) | 50.485 (46.930–55.934) |
| Árvore de 10 mil notas, 16 clientes | 84,3 (77,3–96,8) | 231,1 (215,9–259,4) |
| Busca, 16 clientes | 55,9 (51,5–61,4) | 57,9 (52,7–63,4) |
| Backlinks, 16 clientes | 40,1 (34,7–44,8) | 42,4 (39,5–45,3) |

A máquina permaneceu com o desktop ativo e houve variação entre rodadas. As razões de
throughput entre os dois backends dentro de cada rodada ajudam a controlar essa variação:
Rust/Go ficou entre 2,19× e 2,22× na escrita e entre 2,68× e 2,84× na árvore.
Busca e backlinks continuam próximos, com ambos usando `rg`.

| Latência p99 com carga | Go corrigido | Rust |
|---|---:|---:|
| Escrita, 64 clientes | 14,39 ms | 6,57 ms |
| Leitura de nota, 64 clientes | 7,82 ms | 5,13 ms |
| Árvore, 16 clientes | 303,97 ms | 110,60 ms |

No cenário de 1.000 clientes fazendo leitura–modificação–escrita, cada backend preservou
as 2.000 edições de cada rodada, totalizando 8.000 por backend. O tempo mediano foi
14,26 s no Go (12,50–15,93 s) e 3,44 s no Rust (2,66–3,59 s).
As disputas de uma mesma versão por 50 e 1.000 clientes aceitaram somente um salvamento,
com conteúdo íntegro e sem temporários restantes.

### Um cliente

São requisições contínuas de um cliente HTTP; estes números medem o servidor, sem o custo
de renderização do navegador ou o intervalo de autosave.

| Latência p50 | Go corrigido | Rust |
|---|---:|---:|
| Leitura de nota | 0,166 ms | 0,144 ms |
| Salvamento de 4 KiB | 0,321 ms | 0,211 ms |
| Árvore de 10 mil notas | 47,63 ms | 17,76 ms |

A leitura e o salvamento de uma nota ficam abaixo de 1 ms nas medianas medidas. A árvore
grande leva dezenas de milissegundos; a conclusão anterior de que todos os endpoints
responderiam em menos de 1 ms não se sustenta nesses dados.

## Memória e SSE

| Medida | Go corrigido | Rust |
|---|---:|---:|
| RSS ocioso, vault grande, mediana de quatro execuções | 23,2 MiB | 7,7 MiB |
| Pico com 1.000 salvamentos de uma mesma versão, mediana | 72,7 MiB | 36,5 MiB |
| Memória incremental por conexão SSE, 2.000 clientes | 28,0 KiB | 19,4 KiB |
| SSE até o último cliente receber, mediana p50 | 160,3 ms | 160,6 ms |
| Pico na carga mista, rodada 1 | 44,3 MiB | 83,2 MiB |
| Pico na carga mista, rodada 2 | 46,0 MiB | 86,9 MiB |
| RSS cinco segundos após a carga mista, rodada 1 | 27,3 MiB | 82,0 MiB |
| RSS cinco segundos após a carga mista, rodada 2 | 31,0 MiB | 86,2 MiB |

Todas as 20 mudanças externas chegaram aos 2.000 clientes SSE em cada rodada, sem entrega
parcial ou evento perdido. O debounce de 150 ms continua dominando a latência.

Os backends receberam a mesma concorrência e duração na carga mista, mas completaram
quantidades diferentes de trabalho:

| Operações concluídas em 90 s | Go, rodadas 1 / 2 | Rust, rodadas 1 / 2 |
|---|---:|---:|
| Leitura de nota | 440.481 / 438.541 | 643.344 / 654.298 |
| Árvore | 154 / 152 | 958 / 960 |
| Busca | 2.065 / 2.056 | 2.138 / 2.144 |
| Backlinks | 1.648 / 1.639 | 1.526 / 1.530 |
| Salvamento | 30.220 / 28.970 | 85.248 / 85.714 |

Não houve erro de transporte ou resposta diferente de 200 nesses geradores. Rust completou
mais leituras de notas, árvores e salvamentos, usando mais RSS; Go reduziu mais seu RSS
depois da carga. Essas amostras não demonstram ausência de vazamento nem isolam a causa
do crescimento de memória. Não foram testados outros allocators nesta campanha.

## Falha adicional de relink sob edição concorrente

As quatro rodadas de cada backend preservaram as 1.600 versões enviadas à lixeira e as
fontes dos movimentos rejeitados. Também terminaram sem sobrescrita nas 800 disputas de
criação versus movimento por backend e sem perda de edições aceitas durante o relink.

A verificação de que nenhuma referência antiga ficaria fora da lista de arquivos ignorados
falhou no Rust em duas rodadas: dez referências na segunda e uma na quarta. Go passou
nessa verificação nas quatro rodadas. [Falhas registradas](backend-comparison-2026-10-13/consistency-failures.json).

Uma cópia temporária instrumentada do Rust reproduziu a omissão em uma de oito tentativas:
a busca entregou 199 candidatos, a leitura em lote conseguiu ler os 199, mas existiam
200 referências. Ao final, uma estava atualizada, 198 estavam na lista de ignoradas e uma
mantinha o link antigo sem estar nessa lista. Todas as 11.108 edições aceitas dessa
tentativa estavam presentes. [Diagnóstico](backend-comparison-2026-10-13/relink-diagnostic.txt),
[checagens](backend-comparison-2026-10-13/relink-diagnostic.json).

A omissão ocorreu antes da leitura em lote; a causa interna entre a busca `rg` e a
travessia do filesystem sob renames concorrentes não foi isolada. Ambos os backends
dependem dessa estratégia de descoberta. O resultado não permite atribuir uma garantia
geral de integridade à linguagem. O código Rust medido foi preservado; a instrumentação
ficou separada em uma cópia temporária.

A primeira campanha parou nessa falha. Os dados já medidos foram preservados e a campanha
foi retomada, conferindo os hashes dos binários e os parâmetros. O executor passou a
concluir medições independentes e acumular as falhas de consistência, mantendo código de
saída **1** ao final. Portanto, as 128 medições e quatro cargas mistas estão completas,
mas o gate de consistência do Rust está **reprovado**. [Status final](backend-comparison-2026-10-13/status.json).

## Limites e reprodução

Esta campanha usa uma máquina, loopback, tmpfs, conteúdo sintético e processos novos.
Não mede latência de SSD nem durabilidade em falta de energia: os salvamentos continuam
sem `fsync`. Os testes funcionais adicionais em Btrfs não são benchmarks de armazenamento.
Cada carga mista durou 90 s; não foi um teste de longa duração de um processo único.
Também não foram comparados novamente builds limpos, TLS, HTTP/2 ou outros sistemas operacionais.
Os números da campanha anterior usavam duração e aquecimento diferentes, portanto não
servem para atribuir uma mudança de desempenho especificamente às correções do Go.

As correções de leitura eliminam a inconsistência demonstrada entre inodes substituídos;
não criam uma transação com programas externos que escrevem no mesmo inode ou entre a
verificação de versão e o salvamento.

O executor está em [scripts/compare-backends.py](../scripts/compare-backends.py). Ele usa
o harness em `bench/` desta branch `bench/rust-axum-vs-go`, que deve ser revisado antes de executar
uma cópia diferente. Os binários devem ser previamente compilados com o mesmo `web/dist`;
o Go deve vir deste checkout corrigido e o Rust, das fontes do experimento.

```sh
mkdir -p /tmp/qbench
# Execute a partir da raiz desta branch de benchmark.
# Os três caminhos de binários abaixo devem apontar para os builds desejados.
python3 -B scripts/compare-backends.py \
  --bench-dir bench \
  --go /tmp/qbench/corrected-ebt2nj_9/bin/quirenote-go \
  --rust /tmp/qbench/corrected-ebt2nj_9/bin/quirenote-rs \
  --loadgen /tmp/qbench/corrected-ebt2nj_9/bin/loadgen \
  --reps 4 --seconds 5 --soak-reps 2 --soak-seconds 90
```

Cada execução cria seu próprio diretório em `/tmp/qbench` e rejeita uma porta ocupada antes
de iniciar o servidor. `--resume-dir` permite retomar uma execução com os mesmos hashes e
parâmetros; as falhas de consistência permanecem nos dados e no status final.
