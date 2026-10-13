# Backend do Quirenote: Go vs Rust (Axum)

**Campanha histórica, com o Go anterior às correções de integridade.** Esta branch agora contém
as correções e a [nova comparação com Go corrigido](../docs/backend-comparison-2026-10-13.md),
incluindo medições completas e as falhas de consistência do relink Rust.

Relatório do experimento que reimplementou o backend Go (`cmd/` + `internal/`) em Rust com Axum
(`rust/`, binário `quirenote-rs`) e comparou os dois lado a lado. A versão completa, com todas as
tabelas, variações entre execuções e dados brutos, está em [`RESULTS.md`](RESULTS.md) (em inglês).

- **Base medida:** commit `2bc1018`. O código Go não foi alterado nessa campanha original.
- **Builds:** o Go foi compilado como no Makefile (`go build -trimpath -ldflags="-s -w"`, go1.27.0). O Rust
  1.98.1 usou `cargo build --release` com `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`,
  `strip = true` e o alocador da glibc.
- **Mesmo frontend** Svelte embutido nos dois binários.

## O que foi medido

1. **Paridade de contrato HTTP.** Uma suíte black-box (`contract/`) roda os mesmos 378 casos de API,
   mais 33 de setup, contra os dois servidores e compara status, headers e corpo. A suíte também roda 46
   checagens de invariantes (versões, lixeira, path traversal, symlinks).
2. **Frontend real.** Chromium contra cada backend (`e2e/`): carregar a árvore, abrir, editar,
   autosalvar, recarregar e receber eventos SSE.
3. **Escritas concorrentes na mesma nota:**
   - 50, 200 e 1000 clientes salvando a mesma versão: quantos são aceitos, quantos recebem 409, se o
     arquivo fica íntegro e se sobram temporários;
   - 1000 workers em leitura-modificação-escrita: se há *lost update*.
4. **Escritas em notas diferentes:** throughput, latência p50/p95/p99 e CPU por requisição.
5. **Leituras num vault de 10.000 notas** (1.114 pastas, com wikilinks, PDFs e imagens): nota, `/api/tree`,
   `/api/search` e `/api/backlinks`.
6. **Memória:** RSS ocioso, pico (`VmHWM`) em cada cenário, RSS depois da carga e uma carga mista de 90 s.
7. **SSE:** até 2.000 clientes em `/api/events`, com mudanças externas no disco. Mede a latência até o último
   cliente receber e a memória por conexão.
8. **Lixeira, move e relink concorrentes**, para checar correção.
9. **Tempo de start, tamanho do binário, tempo de build limpo e tamanho do código.**

## Metodologia

- **Máquina:** AMD Ryzen 7 7735HS (8 núcleos / 16 threads), 5,5 GiB de RAM (só ~1 GiB livre, com uma sessão
  desktop ativa), Linux 7.2.5.
- **Isolamento:**
  - um servidor por vez, com processo novo e cópia nova do vault em cada execução;
  - `HOME`/`XDG_CONFIG_HOME` temporários, `-vault` explícito e portas 9100-9199;
  - o vault real do usuário, a config e o serviço instalado nunca foram usados.
- **Fixação de CPU:** servidor nos CPUs 0-7 e gerador de carga nos CPUs 8-15 (`taskset`).
- **Repetições:** 3 por cenário, alternando Go e Rust. As tabelas mostram a mediana.
- **Gerador de carga:** o mesmo para os dois (`loadgen/`, Go com `net/http`).

## Resultados (medianas de 3 execuções)

| | Go | Rust |
|---|---|---|
| Contrato HTTP (378 + 33 casos) | referência | 374 + 33 idênticos, 4 diferenças documentadas |
| Checagens de invariantes | 44/46 | 46/46 |
| Frontend real no Chromium | 9/9 | 9/9 |
| Lost updates, 1000 workers na mesma nota | **105 de 2.000 escritas aceitas perdidas** | 0 |
| Mesma versão salva por 50/200/1000 clientes | 1 aceita, resto 409, arquivo íntegro | idêntico |
| Saves em notas diferentes (64 clientes) | 7.455 req/s, p99 de 10,4 ms | 14.877 req/s, p99 de 5,2 ms |
| Leitura de nota (64 clientes) | 45.634 req/s, p99 de 6,8 ms | 57.594 req/s, p99 de 4,8 ms |
| `/api/tree` de 1,1 MB (16 clientes) | 94 req/s, p99 de 272 ms | 250 req/s, p99 de 96 ms |
| Search / backlinks (16 clientes) | 65 / 47 req/s | 65 / 49 req/s (empate: os dois chamam o `rg`) |
| RSS ocioso, vault de 10.000 notas | 19,4 MB | 7,0 MB |
| Pico de RSS com 1000 saves simultâneos | 69 MB | 24,5 MB |
| Pico de RSS em carga mista de 90 s | **42 MB** | 82 MB (33 MB com `MALLOC_ARENA_MAX=2`, mas -55% de throughput no tree) |
| SSE com 2.000 clientes: memória por conexão | 29 KB | 20 KB |
| SSE: latência até o último cliente | 157,7 ms | 158,0 ms (empate, dominado pelo debounce de 150 ms) |
| Start até o primeiro `/api/tree` 200 (vault grande) | 58 ms | 23 ms |
| Binário | 12,0 MiB | 8,1 MiB |
| Build limpo | **8 s** | 73 s |
| Linhas de código sem testes | **1.909** | 3.941 |

**Onde o Rust é melhor:** contenção de escrita, a resposta grande do `/api/tree`, memória em repouso e por
conexão SSE, start e binário.

**Onde empata:** busca, backlinks, latência de SSE e correção quando todos salvam a mesma versão.

**Onde o Go é melhor:**
- tempo de build;
- metade do código: o `net/http`, o `encoding/json` e a tabela MIME do Go tiveram de ser refeitos à mão
  em Rust;
- memória sob carga mista: o Rust guarda a memória que cresceu, o Go devolve.

## Bugs encontrados no Go (só reportados; a reprodução está em `repro/`)

1. **Lost update.** `Vault.Read` lê o conteúdo e depois o mtime, em duas chamadas separadas
   (`internal/vault/vault.go`). Um save entre as duas faz o cliente receber conteúdo antigo com mtime novo,
   e o save seguinte apaga uma edição aceita.
   - `lost_update.py` perdeu edições em 20/20 rodadas. Numa verificação independente, perdeu em 5/5
     rodadas (24 edições).
   - O relink usa o mesmo `Read`.
   - Correção: tirar o mtime do arquivo aberto (`f.Stat()`).
2. **Create vs Move no mesmo destino** destrói um arquivo sem erro. O `Create` não pega o mutex que o
   `Move` usa entre o `Lstat` e o `Rename`.
   - `create_vs_move.py`: 31 de 200 rodadas (15%).
   - Na carga de consistência, 36-48% das rodadas.
   - Correção: `renameat2(RENAME_NOREPLACE)` ou travar o `Create` no mesmo mutex.
3. `GET /api/notes/<pasta>` devolve 500 e mostra o caminho absoluto do vault na mensagem. Outros erros do
   cliente também viram 500.
4. Um `echo > novo.md` feito por fora do app chega como evento `write` depois do debounce, e a árvore da
   interface não atualiza.

## Limites

- Uma máquina, só loopback e vaults em tmpfs: sem latência de disco e sem `fsync`. Os números medem CPU e
  runtime, não armazenamento.
- Dados sintéticos e 3 repetições, num desktop ocupado: há ruído.
- Sem TLS, HTTP/2, outros sistemas operacionais ou carga mista maior que 90 s.
- A busca mede principalmente o `rg`.

## Conclusão

Para um app de notas de um usuário em localhost, nenhuma das diferenças de desempenho aparece no uso: os
dois respondem em bem menos de 1 ms e usam dezenas de MB. Os bugs de integridade estão no desenho do código
Go, não na linguagem, e se corrigem com poucas linhas. A recomendação é **corrigir os bugs 1 e 2 no Go e
manter o Go**. O Rust só valeria a pena se memória numa máquina pequena sempre ligada passasse a importar
mais do que o custo de build e de manutenção.

## Como reproduzir

Os comandos estão na seção 8 do [`RESULTS.md`](RESULTS.md). Todos os scripts se recusam a rodar fora de
`/tmp/qbench` ou de portas fora de 9100-9199.
