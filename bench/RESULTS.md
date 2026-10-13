# Quirenote backend: Go vs Rust (Axum), side by side

An experiment: the Go backend (`cmd/` + `internal/`, 1,909 non-test lines) reimplemented in Rust with
Axum (`rust/`, 3,941 non-test lines, rustfmt layout), compared on HTTP-contract parity, correctness under
concurrency, speed and memory. Same Svelte frontend, embedded in both binaries.
Everything here was produced on one machine, in one afternoon, by the scripts in this directory.

**Base:** commit `2bc1018` (Go code untouched; `git diff 2bc1018 -- cmd internal web go.mod` is empty).
**Go:** `go build -trimpath -ldflags="-s -w"` (as in the Makefile), go1.27.0. **Rust:** 1.98.1,
`cargo build --release` with `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `strip = true`,
system glibc allocator.

## 1. The short version (medians of 3 runs; details and spreads in section 4)

| | Go | Rust | Comment |
|---|---|---|---|
| Contract: 378 + 33 black-box HTTP cases | reference | 374 + 33 identical, 4 documented differences, 0 undocumented | section 3 |
| Contract: invariant checks | 44/46 | 46/46 | the two Go failures are real bugs (section 5) |
| Real frontend in Chromium (load tree, open, edit, autosave, reload, SSE) | 9/9 | 9/9 | `bench/e2e` |
| Lost updates, 1000 workers read-modify-write one note | **105 of 2,000 accepted writes lost** | 0 | Go bug (section 5.1) |
| Same version saved by 50/200/1000 clients | 1 accepted, rest 409, file intact, no temp files | identical | no difference in correctness |
| Create racing a move onto the same name | **file silently destroyed in 36-48% of rounds** | 0 | Go bug (section 5.2) |
| Saves of different notes, 64 clients | 7,455 req/s, p99 10.4 ms | 14,877 req/s, p99 5.2 ms | same CPU seconds; 0.45 vs 0.22 ms of CPU per request |
| Read a note (64 clients) | 45,634 req/s, p99 6.8 ms | 57,594 req/s, p99 4.8 ms | Rust 26% more requests with 17% less CPU |
| Read `/api/tree` (1.1 MB, 16 clients) | 94 req/s, p99 272 ms | 250 req/s, p99 96 ms | both saturate their cores; ~2.6x more requests per CPU-second |
| Search / backlinks (16 clients) | 65 / 47 req/s | 65 / 49 req/s | tie: both wait for `rg` (95 CPU-s of `rg` in each run); server-side CPU 3.4/2.3 s (Go) vs 1.6/1.1 s (Rust) |
| Idle RSS, 10,000-note vault | 19.4 MB | 7.0 MB | |
| Idle RSS, 200-note vault | 12.8 MB | 4.9 MB | |
| Peak RSS under 1000 simultaneous 16 KiB saves | 69 MB | 24.5 MB | |
| Peak RSS, 90 s mixed load | 42 MB (steady 24-35) | 82 MB (steady ~78), **33 MB with `MALLOC_ARENA_MAX=2`** but tree throughput -55% | **Go is better here** (section 4) |
| SSE, 2,000 clients: memory per connection / peak | 29 KB / 71.5 MB | 20 KB / 42.8 MB | |
| SSE, latency until all clients got the event | 157.7 ms | 158.0 ms | tie (the 150 ms debounce dominates) |
| Start to first answer (large vault: first `/api/tree` 200) | 13 ms (58 ms) | 4 ms (23 ms) | |
| Binary | 12.0 MiB | 8.1 MiB | both stripped, frontend embedded |
| Clean build | **8 s** | 73 s | **Go is much better here**; 89 crates vs 2 |
| Source size for the same contract | 1,909 lines | 3,941 lines | `net/http`, `encoding/json`, `os.Root` and `mime` quirks had to be rebuilt by hand |

What this does **not** say: that Rust is "faster" in general. Reads of small notes, search, backlinks
and SSE latency are limited by the kernel, `rg` or a 150 ms timer, and there the two are the same within
noise. The differences that survive are in write contention, the 1 MB tree response, and resident memory
at rest, plus the correctness bugs (which are in the Go code's design, not in the language).

## 2. Machine and method

* **Machine:** AMD Ryzen 7 7735HS (8 cores / 16 threads, 4.8 GHz max, governor `performance`), 5.5 GiB RAM
  of which only ~1 GiB was free (a desktop session with a browser and editors was running: expect noise),
  Linux 7.2.5 (`CONFIG_HZ=1000`), glibc 2.44, ripgrep 15.2.0, Python 3.14, Node 26.5, Chromium 152.
* **Isolation:** one server at a time, a **fresh process and a fresh copy of the vault per run**, ports
  9130-9199, temporary `HOME`/`XDG_CONFIG_HOME`, explicit `-vault`; every start verifies that the log names the
  temporary vault. The real vault, `~/.config/quirenote`, the systemd service and the running instances on
  :8080/:8090 were never touched. Vaults live on tmpfs (`/tmp/qbench`): no disk latency, and neither server
  calls `fsync`, so the numbers measure CPU and runtime overhead, not storage.
* **CPU pinning:** servers on CPUs 0-7, load generator on CPUs 8-15 (`taskset`), so they do not fight for
  cores (the Go runtime and Tokio both see 8 CPUs).
* **Repetitions:** 3 per scenario, Go and Rust alternating inside each repetition; tables show the **median
  (min-max)**. Timed phases: 10 s after a 2 s warm-up for reads, 10 s for writes.
* **CPU** is `utime+stime` of the server process from `/proc/<pid>/stat` over the scenario (`rg` children are
  reported separately). **Peak RSS** is `VmHWM` of the fresh process (it includes start-up). **RSS right after**
  and **3 s after** show whether memory goes back down.
* **Load generator:** `bench/loadgen` (Go, standard `net/http`), the same binary for both servers
  (`oha`/`wrk`/`hey` are not installed here). Its own CPU is outside the server numbers but it shares the
  machine.
* **Data:** `bench/gen/genvault.py`, deterministic (seeded), the same bytes for both: small = 200 notes + 4
  PDFs + 8 PNGs; large = **10,000 notes in 1,114 nested folders**, `[[Name]]`/`[[folder/Name]]`/`[[Name|alias]]`
  links with a zipf-like popularity (so backlink counts vary), ~1% repeated names, 40 PDFs (60-240 KB), 80
  images; 23.2 MB, content hash in `manifest.json`.

## 3. HTTP contract parity

`bench/contract/contract.py` is a black-box suite: it only needs a URL (and the vault directory, to seed it and
to change files "from outside" like Neovim). It records status, headers and normalised body of **378 cases**
(routing, 405/415/307, path traversal and symlink escape, JSON quirks, save/conflict/atomicity, move, trash,
images, `/api/files` ranges and conditional requests, search, backlinks, relink, SPA and static files, raw
HTTP oddities, SSE for every kind of external change, concurrency) plus **33 first-run setup cases**, and
`diff` compares two recordings. Go is the oracle; **every difference had to be fixed in Rust or documented**.

Result against the final binaries (`results/contract-diff.txt`): **374 of 378 identical + 33 of 33 identical;
4 documented, 0 undocumented** (`contract/known_divergences.json`):

| case | Go | Rust | why it stays |
|---|---|---|---|
| `Range: bytes=0-5,10-15` | 206 `multipart/byteranges` | 200 whole file | allowed by RFC 9110; PDF.js and the app only send one range |
| `GET /assets/` (embedded directory) | HTML listing | `index.html` | not used by the app |
| 70 KB request target | 500 (file name too long) | 414 from hyper | transport limit |
| `PUT` on a path that is a directory named `x.md` | 500 `renameat <tmp> ...: file exists` | 500 `renameat ...: is a directory` | both 500; errno text differs |

Not byte-identical but equivalent and not detectable by the suite: header order and `Date`; Go answers large
bodies chunked (no `Content-Length`), Rust sends the length; the Rust server has no `ReadHeaderTimeout`
(axum does not expose it).

Go behaviours reproduced on purpose (found by the suite, each had been a divergence): the 415 check applies
to every non-GET/HEAD request and compares `application/json` case-sensitively; `GET` on any unknown path
(even `/api/nope`) returns the SPA; 405 bodies and `Allow` lists; 307 for unclean paths and `/api/notes` →
`/api/notes/`; JSON decoding takes the first value only (`{"content":"x"} garbage` is accepted), `null` is the
zero value, field names are case-insensitive, `mtime` must be a *string*; Go's MIME table (built-in + the
system shared-MIME database); `Range`/`If-Modified-Since`/`If-Range`/416 rules and which 416s carry
`Content-Range`; a missing `Host` is a 400; the `os.Root` error texts; HTML-escaped JSON; invalid UTF-8 in a
file is returned as one U+FFFD per byte; `GET /api/notes/<directory>` is a 500 that includes the absolute
vault path (a Go bug that Rust copies for parity).

**Where Rust deliberately differs** (all strictly more correct, none visible to a correct client):
(1) `GET /api/notes/...` reads content and mtime from one file descriptor (Go reads, then stats the path);
(2) every accepted save gets a strictly newer mtime (defence for coarse-clock ties; **not** demonstrated on this
machine: `bench/repro/mtime_tie.py` found 0 ties in 3,000 rounds on Go, and disabling it changed nothing
measurable, see the "coarse-mtime" rows below); (3) `move` and `trash` use `renameat2(RENAME_NOREPLACE)`.

Unit tests: `cargo test` = **36 unit + 4 API tests, all pass** (ports of the Go tests for vault, search, relink,
setup, config, the 415 and first-run API tests, plus watcher, ranges, mux behaviour, images, JSON quirks).
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean;
`#![deny(clippy::unwrap_used, clippy::expect_used)]` covers the whole crate, so no `unwrap()` can sit on a
request path. Go: `go test ./...` passes (unchanged).

## 4. Results

Medians of 3 runs, `(min-max)` where they differ. Rows named "Rust (coarse-mtime, Go-like)" are the same Rust
binary started with `QUIRENOTE_RS_COARSE_MTIME=1`, to price the mtime guard (it costs nothing measurable).
Raw data: `results/raw.json`, `results/raw-coarse-mtime.json`; tables regenerated by `aggregate.py`.

<!-- generated by bench/aggregate.py from results/raw.json, results/raw-coarse-mtime.json; 3 repetitions, median (min-max) -->

#### 1a. Concurrent saves of the SAME version of one note (all N clients hold the version they read)

Each client sends a body (16 KiB for `conflict-N`, 64 KiB for `conflict64k-N`) that carries its own id in every 1 KiB line, so truncation or interleaving would be detectable.

| case | server | accepted | rejected 409 | other status | transport errors | lost updates | tmp files left | wall ms | p50 ms | p99 ms | server CPU s | peak RSS MB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| conflict-50 | Go | 1 | 49 | - | 0 | 0 | 0 | 13 (9-95) | 7 (7-81) | 9 (8-93) | 0.04 (0.03-0.17) | 14.6 (13.9-15.1) |
| conflict-50 | Rust | 1 | 49 | - | 0 | 0 | 0 | 6 (5-8) | 3 (3-4) | 6 (5-7) | 0.01 | 6.5 (6.4-6.6) |
| conflict-200 | Go | 1 | 199 | - | 0 | 0 | 0 | 41 (34-42) | 28 (21-35) | 38 (32-40) | 0.13 | 23.1 (21.6-24.1) |
| conflict-200 | Rust | 1 | 199 | - | 0 | 0 | 0 | 24 (22-34) | 11 (7-25) | 22 (20-34) | 0.06 (0.05-0.06) | 9.2 (9.1-9.7) |
| conflict-1000 | Go | 1 | 999 | - | 0 | 0 | 0 | 141 (135-181) | 109 (86-109) | 133 (118-149) | 0.61 (0.61-0.67) | 69.2 (67.2-76.9) |
| conflict-1000 | Rust | 1 | 999 | - | 0 | 0 | 0 | 90 (76-95) | 62 (50-70) | 80 (70-91) | 0.30 (0.24-0.30) | 24.5 (17.8-27.9) |
| conflict64k-50 | Go | 1 | 49 | - | 0 | 0 | 0 | 416 (414-417) | 223 (218-224) | 225 (224-225) | 0.08 | 22.0 (19.9-22.7) |
| conflict64k-50 | Rust | 1 | 49 | - | 0 | 0 | 0 | 209 (9-218) | 7 (6-9) | 9 (8-11) | 0.04 (0.04-0.05) | 11.8 (11.3-13.9) |
| conflict64k-200 | Go | 1 | 199 | - | 0 | 0 | 0 | 416 (415-417) | 244 (243-255) | 265 (257-271) | 0.28 (0.28-0.29) | 41.8 (41.5-44.0) |
| conflict64k-200 | Rust | 1 | 199 | - | 0 | 0 | 0 | 203 (39-211) | 24 (13-30) | 37 (37-37) | 0.17 (0.17-0.19) | 23.1 (17.8-35.2) |
| conflict64k-1000 | Go | 1 | 999 | - | 0 | 0 | 0 | 557 (469-783) | 422 (392-441) | 544 (450-582) | 1.40 (1.33-1.82) | 153.3 (150.4-170.9) |
| conflict64k-1000 | Rust | 1 | 999 | - | 0 | 0 | 0 | 220 (212-221) | 131 (127-142) | 187 (184-190) | 1.12 (1.06-1.19) | 106.9 (95.9-126.1) |

Integrity verdict of the final file (it must be exactly one accepted client's content):

| case | server | verdict(s) seen | runs ok |
|---|---|---|---|
| conflict-50 | Go | ok | 3/3 |
| conflict-50 | Rust | ok | 3/3 |
| conflict-200 | Go | ok | 3/3 |
| conflict-200 | Rust | ok | 3/3 |
| conflict-1000 | Go | ok | 3/3 |
| conflict-1000 | Rust | ok | 3/3 |
| conflict64k-50 | Go | ok | 3/3 |
| conflict64k-50 | Rust | ok | 3/3 |
| conflict64k-200 | Go | ok | 3/3 |
| conflict64k-200 | Rust | ok | 3/3 |
| conflict64k-1000 | Go | ok | 3/3 |
| conflict64k-1000 | Rust | ok | 3/3 |

#### 1b. Read-modify-write on one note (each worker GETs, appends a unique line, PUTs with the mtime it read; 409 = retry)

Every accepted write must survive: lines on disk = accepted. A shortfall is a lost update.

| case | server | accepted | 409s | lines on disk | LOST updates | wall ms | server CPU s | peak RSS MB | tmp left |
|---|---|---|---|---|---|---|---|---|---|
| rmw-50x10 | Go | 500 | 7,556 (7,060-7,932) | 446 (441-449) | 54 (51-59) | 457 (435-460) | 2.4 (2.2-2.5) | 16.6 (15.0-17.9) | 0 |
| rmw-50x10 | Rust | 500 | 5,037 (5,007-5,118) | 500 | 0 | 272 (270-335) | 1.1 (1.1-1.1) | 10.7 (10.3-10.9) | 0 |
| rmw-50x10 | Rust (coarse-mtime, Go-like) | 500 | 4,981 (4,660-4,995) | 500 | 0 | 268 (260-269) | 1.1 (1.0-1.1) | 10.7 (10.2-11.3) | 0 |
| rmw-200x5 | Go | 1,000 | 48,978 (47,838-52,971) | 891 (879-903) | 109 (97-121) | 2,610 (2,587-2,988) | 17.3 (16.8-19.6) | 24.3 (22.5-25.5) | 0 |
| rmw-200x5 | Rust | 1,000 | 18,364 (18,103-18,766) | 1,000 | 0 | 938 (913-978) | 4.5 (4.4-4.5) | 15.3 (14.5-15.4) | 0 |
| rmw-200x5 | Rust (coarse-mtime, Go-like) | 1,000 | 18,676 (18,026-19,066) | 1,000 | 0 | 970 (917-977) | 4.5 (4.3-4.7) | 15.2 (15.2-16.4) | 0 |
| rmw-1000x2 | Go | 2,000 | 134,149 (128,385-142,527) | 1,895 (1,888-1,903) | 105 (97-112) | 9,002 (8,858-10,521) | 66.2 (64.2-75.3) | 76.9 (73.9-78.7) | 0 |
| rmw-1000x2 | Rust | 2,000 | 40,525 (40,381-40,743) | 2,000 | 0 | 2,526 (2,507-2,576) | 13.8 (13.6-13.9) | 35.0 (34.5-37.9) | 0 |
| rmw-1000x2 | Rust (coarse-mtime, Go-like) | 2,000 | 43,458 (42,504-43,531) | 2,000 | 0 | 2,678 (2,635-2,770) | 14.7 (14.6-14.8) | 35.4 (32.5-38.1) | 0 |

#### 2. Concurrent saves of DIFFERENT notes (64 clients, 4 KiB, 10 s, each with the mtime of its previous response)

| case | server | req/s | p50 ms | p95 ms | p99 ms | non-200 | transport errors | server CPU s |
|---|---|---|---|---|---|---|---|---|
| writes-64 | Go | 7,455 (7,077-7,522) | 8.54 (8.47-8.87) | 9.38 (9.31-10.74) | 10.43 (10.05-12.60) | 0 | 0 | 33.3 (33.3-33.4) |
| writes-64 | Rust | 14,877 (14,660-15,011) | 4.29 (4.26-4.36) | 4.83 (4.79-4.87) | 5.24 (5.14-5.26) | 0 | 0 | 33.1 (33.0-33.1) |
| writes-64 | Rust (coarse-mtime, Go-like) | 15,031 (13,885-15,738) | 4.17 (4.06-4.53) | 5.15 (4.54-5.38) | 5.92 (4.85-6.34) | 0 | 0 | 33.6 (33.6-33.7) |

| server | requests ok | CPU s | CPU ms per request |
|---|---|---|---|
| Go | 74,615 | 33.3 | 0.447 |
| Rust | 148,823 | 33.1 | 0.222 |
| Rust (coarse-mtime, Go-like) | 150,379 | 33.6 | 0.223 |

#### 3. Reads on the large vault (10,000 notes, 1,114 folders)

10 s measured after a 2 s warm-up. Search and backlinks run `rg` as a child process in both servers (its CPU is in the children column).

| case | server | clients | req/s | p50 ms | p95 ms | p99 ms | avg body B | non-200 | transport err | server CPU s | rg children CPU s | peak RSS MB |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| read-note | Go | 64 | 45,634 (45,239-46,605) | 0.81 (0.81-0.82) | 4.74 (4.62-4.88) | 6.82 (6.71-7.20) | 1,640 (1,639-1,640) | 0 | 0 | 82.1 (80.7-82.4) | 0.0 | 18.8 (18.6-18.9) |
| read-note | Rust | 64 | 57,594 (57,049-58,248) | 0.68 (0.62-0.68) | 3.28 (3.27-3.31) | 4.82 (4.78-4.85) | 1,640 | 0 | 0 | 68.3 (67.8-69.5) | 0.0 | 17.1 (16.8-18.1) |
| read-tree | Go | 16 | 94 (86-95) | 165.10 (164.11-180.67) | 240.53 (234.09-250.32) | 272.18 (271.38-289.69) | 1,146,871 | 0 | 0 | 92.2 (90.9-92.4) | 0.0 | 97.9 (95.3-103.0) |
| read-tree | Rust | 16 | 250 (248-254) | 63.27 (63.00-64.31) | 87.41 (84.38-88.45) | 96.34 (93.15-97.94) | 1,146,871 | 0 | 0 | 95.6 (94.6-95.8) | 0.0 | 78.7 (75.6-82.3) |
| read-search | Go | 16 | 65 (64-65) | 288.16 (286.63-294.64) | 379.75 (374.78-388.17) | 414.39 (409.21-422.53) | 37,977 (37,950-38,095) | 0 | 0 | 3.4 (3.4-3.4) | 94.2 (94.0-95.5) | 20.7 (19.7-21.4) |
| read-search | Rust | 16 | 65 (62-67) | 310.82 (303.97-320.46) | 390.21 (379.28-406.76) | 415.56 (397.61-459.99) | 38,019 (37,932-38,170) | 0 | 0 | 1.6 (1.5-1.6) | 96.9 (96.5-97.0) | 17.0 (16.7-17.7) |
| read-backlinks | Go | 16 | 47 (45-48) | 343.57 (340.12-364.45) | 393.88 (377.55-419.69) | 413.82 (392.41-443.88) | 17,091 (17,091-17,146) | 0 | 0 | 2.3 (2.3-2.4) | 96.6 (95.8-96.9) | 19.7 (17.9-20.4) |
| read-backlinks | Rust | 16 | 49 (48-49) | 336.35 (335.41-344.11) | 375.10 (373.87-385.39) | 398.43 (391.24-400.30) | 17,138 (17,115-17,160) | 0 | 0 | 1.1 (1.0-1.1) | 98.0 (97.8-98.0) | 15.2 (15.0-15.3) |

#### 4a. Memory at rest (RSS 1.5 s after the start, MB)

| vault | server | idle RSS MB | peak RSS (VmHWM) MB | threads |
|---|---|---|---|---|
| startup-small | Go | 12.8 | 12.8 | 8 (8-9) |
| startup-small | Rust | 4.9 | 4.9 | 11 |
| startup-large | Go | 19.4 | 19.4 | 13 |
| startup-large | Rust | 7.0 | 7.5 | 12 |

#### 4b. Memory in each scenario (fresh process per run: peak = VmHWM at the end; MB)

| scenario | server | idle RSS | PEAK (VmHWM) | RSS right after | RSS 3 s after | threads after |
|---|---|---|---|---|---|---|
| conflict-50 | Go | 10.0 | 14.6 | 14.3 | 14.3 | 14 |
| conflict-50 | Rust | 4.8 | 6.5 | 6.5 | 6.5 | 11 |
| conflict-200 | Go | 10.1 | 23.1 | 21.3 | 20.7 | 15 |
| conflict-200 | Rust | 4.8 | 9.2 | 9.4 | 9.1 | 11 |
| conflict-1000 | Go | 8.8 | 69.2 | 67.2 | 62.5 | 15 |
| conflict-1000 | Rust | 4.8 | 24.5 | 21.0 | 21.0 | 11 |
| conflict64k-50 | Go | 8.1 | 22.0 | 15.0 | 15.0 | 14 |
| conflict64k-50 | Rust | 4.8 | 11.8 | 9.3 | 9.3 | 11 |
| conflict64k-200 | Go | 8.4 | 41.8 | 33.5 | 21.3 | 15 |
| conflict64k-200 | Rust | 4.9 | 23.1 | 17.5 | 17.5 | 11 |
| conflict64k-1000 | Go | 9.8 | 153.3 | 145.6 | 71.7 | 14 |
| conflict64k-1000 | Rust | 4.7 | 106.9 | 77.4 | 66.9 | 11 |
| rmw-50x10 | Go | 9.7 | 16.6 | 16.6 | 16.6 | 27 |
| rmw-50x10 | Rust | 4.7 | 10.7 | 10.7 | 10.7 | 32 |
| rmw-50x10 | Rust (coarse-mtime, Go-like) | 4.8 | 10.7 | 10.7 | 10.7 | 24 |
| rmw-200x5 | Go | 9.5 | 24.3 | 20.7 | 19.7 | 28 |
| rmw-200x5 | Rust | 4.7 | 15.3 | 15.3 | 15.3 | 32 |
| rmw-200x5 | Rust (coarse-mtime, Go-like) | 4.9 | 15.2 | 15.2 | 15.2 | 49 |
| rmw-1000x2 | Go | 9.5 | 76.9 | 54.4 | 50.2 | 23 |
| rmw-1000x2 | Rust | 4.7 | 35.0 | 34.8 | 34.8 | 33 |
| rmw-1000x2 | Rust (coarse-mtime, Go-like) | 4.9 | 35.4 | 34.7 | 34.7 | 40 |
| writes-64 | Go | 9.6 | 16.9 | 16.5 | 16.1 | 14 |
| writes-64 | Rust | 4.7 | 12.1 | 12.1 | 12.1 | 10 |
| writes-64 | Rust (coarse-mtime, Go-like) | 4.8 | 12.2 | 12.2 | 12.2 | 10 |
| read-note | Go | 17.9 | 18.8 | 15.7 | 15.7 | 29 |
| read-note | Rust | 6.9 | 17.1 | 17.1 | 17.1 | 48 |
| read-tree | Go | 19.8 | 97.9 | 66.9 | 38.5 | 18 |
| read-tree | Rust | 6.9 | 78.7 | 69.8 | 69.8 | 26 |
| read-search | Go | 20.5 | 20.7 | 17.1 | 17.1 | 18 |
| read-search | Rust | 6.9 | 17.0 | 17.0 | 16.9 | 10 |
| read-backlinks | Go | 19.8 | 19.7 | 16.8 | 16.8 | 16 |
| read-backlinks | Rust | 7.0 | 15.2 | 15.2 | 15.2 | 10 |
| sse-500 | Go | 10.6 | 26.4 | 26.4 | 26.4 | 14 |
| sse-500 | Rust | 4.7 | 14.5 | 14.5 | 14.5 | 10 |
| sse-2000 | Go | 9.8 | 71.5 | 71.5 | 71.5 | 14 |
| sse-2000 | Rust | 4.7 | 42.8 | 42.8 | 42.8 | 10 |
| consistency | Go | 10.6 | 18.5 | 18.5 | 18.5 | 36 |
| consistency | Rust | 4.7 | 13.4 | 13.4 | 13.4 | 66 |

#### 5. SSE fan-out (K clients on /api/events, 20 external changes written to the vault by rename; latency = write done -> LAST client received)

| case | server | connected | failed | events delivered to all | partial | missed | latency p50 ms | latency max ms | bytes/conn (RSS) | threads | RSS idle MB | RSS w/ conns MB | peak MB | CPU s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| sse-500 | Go | 500 | 0 | 20 | 0 | 0 | 152.4 (152.4-152.5) | 153.0 (152.8-153.1) | 30,130 (29,434-31,564) | 13 | 10.6 | 25,320 | 26.4 | 0.42 |
| sse-500 | Rust | 500 | 0 | 20 | 0 | 0 | 153.2 (153.1-153.4) | 153.6 (153.5-153.9) | 20,136 (20,029-20,161) | 11 | 4.7 | 14,632 | 14.5 | 0.34 |
| sse-2000 | Go | 2,000 | 0 | 20 | 0 | 0 | 157.7 (157.6-157.7) | 160.1 (158.5-168.5) | 29,282 (27,521-29,350) | 14 | 9.8 | 66,728 | 71.5 | 1.58 |
| sse-2000 | Rust | 2,000 | 0 | 20 | 0 | 0 | 158.0 (158.0-158.1) | 159.0 (158.9-161.8) | 19,849 (19,845-19,857) | 11 | 4.7 | 43,636 | 42.8 | 1.28 |

#### 6. Concurrent trash / move / relink (correctness; passes out of runs)

| check | Go | Rust |
|---|---|---|
| trash: same-named notes trashed concurrently all succeed | 3/3 | 3/3 |
| trash: every destination distinct (no version overwritten) | 3/3 | 3/3 |
| trash: every version intact on disk | 3/3 | 3/3 |
| move: many sources onto one destination: exactly one 200 | 3/3 | 3/3 |
| move: losing sources left in place | 3/3 | 3/3 |
| create vs move onto one destination: never both succeed | 0/3 (71 of 200 rounds silently destroyed a file; 80 of 200 rounds silently destroyed a file; 96) | 3/3 |
| relink: ran while editors were saving | 3/3 | 3/3 |
| relink: no accepted edit lost (relink never overwrites a newer save) | 0/3 (3 accepted edits missing of 7444; 4 accepted edits missing of 7109; 5 accepted edits missi) | 3/3 |
| relink: no note left with the old link unless reported as skipped | 3/3 | 3/3 |
| no temporary files left in the vault | 3/3 | 3/3 |

#### 7. Start time (process spawn -> first answer; polled every 2 ms; page cache warm)

| vault | server | first answer (`/api/setup` 200) ms | first `GET /api/tree` 200 ms |
|---|---|---|---|
| small | Go | 6.0 (5.7-34.3) | 7.9 (7.6-36.8) |
| small | Rust | 3.8 (3.5-14.2) | 4.6 (4.5-15.8) |
| large | Go | 13.1 (6.0-16.4) | 57.9 (53.4-64.6) |
| large | Rust | 4.1 (3.6-5.7) | 23.1 (22.9-29.6) |


### How to read the main numbers

* **1a (same version, N clients).** Both servers behave identically: exactly one save accepted, N-1 answered
  409, the final file is byte-for-byte the winner's (no truncation, no interleaving; every 1 KiB line of a body
  carries its writer id), no temporary file left, no transport error, up to 1,000 simultaneous clients. The
  latency gap at 16 KiB (Rust ~2x lower) is small in absolute terms. At 64 KiB (`conflict64k-*`) both servers show
  stalls of ~200 ms per batch (Go in nearly every request, Rust in some): that is a loopback TCP effect (MSS of
  64 KiB, delayed ACK / window update), not server work, and it depends on how fast each process drains the
  socket. I kept both sizes so the effect is visible and not mistaken for server speed; read the 16 KiB rows for
  latency.
* **1b (read-modify-write).** Go loses 5-11% of the accepted writes (section 5.1); Rust none. Rust also finishes the
  same work 3.5x faster at 1000 workers (2.5 s vs 9.0 s) because the contended critical section is cheaper; Go's
  numbers include the cost of doing the work on stale content.
* **2 (different notes).** All saves are serialised by one lock in both servers (check-then-write), so this is
  a measure of the per-save cost, not of parallelism. CPU seconds are equal (33 s); Rust completes twice the saves.
* **3 (reads).** Notes: Rust +26% req/s and lower tail latency (p50 is similar). Tree: a 1.1 MB JSON response
  built from 10,120 entries per request; both use ~95 CPU-seconds, Rust serves 2.6x more of them. Peak memory is
  similar (79 vs 98 MB) but Go gives it back (38 MB 3 s later) while Rust keeps 70 MB (glibc). Search and
  backlinks are a tie.
* **4 (memory).** Idle and per-connection memory favour Rust by 2-3x. Under the 90-second mixed soak
  (`results/soak.txt`: 32 note readers, 2 tree readers, 4 searchers, 4 backlink readers, 8 writers) Go stays at
  24-35 MB (peak 42) while Rust climbs over a minute to ~78 MB and stays there (peak 82): no leak (flat for the
  last 30 s) but glibc per-thread arenas. `MALLOC_ARENA_MAX=2` brings it to 24-31 MB (peak 33) at the price of
  **-55% tree throughput** (249 -> ~110 req/s) and -7% on note reads (measured separately); I did not make that
  the default. A different allocator (mimalloc, jemalloc) did not help in an earlier experiment on the old
  watcher and was not retried on the final code.
* **5 (SSE).** 500 and 2,000 simultaneous clients, 20 external changes (write temp file + rename in the vault).
  Every event reached every client, nothing missed or partial, in both. The latency (write done -> last client
  has it) is the 150 ms debounce plus ~3-8 ms, identical for both. Rust needs ~20 KB per connection, Go ~30 KB.
* **6 (trash/move/relink).** 400 same-named notes trashed concurrently (every version kept, destinations
  distinct, contents intact), 200 sources moved onto one destination (exactly one wins), relink running while 16
  clients keep saving the referencing notes: Rust passes everything; Go fails two checks (section 5).
* **7 (start).** Both start in milliseconds; Rust is faster to listen (4 vs 13 ms) and to answer the first tree
  of the large vault (23 vs 58 ms).

### Other measurements

| | Go | Rust |
|---|---|---|
| Binary, stripped, frontend embedded | 12,607,751 B (12.0 MiB) | 8,531,400 B (8.1 MiB) |
| Clean build (`GOCACHE` empty / empty `target-dir`, deps already downloaded) | 8.1 s wall (48 s user) | 73 s wall (171 s user) |
| Crates / modules compiled | std + 2 modules | 89 crates |
| Non-test source | 1,909 lines | 3,941 lines (+1,298 test lines) |
| Threads at rest | 8-13 | 10-13 |
| Release profile | `-trimpath -ldflags="-s -w"` | `lto=fat`, `codegen-units=1`, `opt-level=3`, `strip` |

## 5. Bugs found

### In the Go code (reported, not fixed)

1. **Lost updates: `vault.Read` pairs content and mtime from different instants** (`internal/vault/vault.go`,
   `Read`: `ReadFile(p)` then `Stat(p)`). If a save renames a new file over `p` between the two calls, the
   client gets the *old* content with the *new* mtime and can then save over the newer version with an
   "up-to-date" mtime: the stale-write check (the whole point of the mtime protocol) is bypassed.
   Reproduction: `bench/repro/lost_update.py` (8 workers x 15 read-modify-write saves of one note, 20 rounds):
   **Go loses updates in 20/20 rounds (83-90 of 2,400 accepted writes)**, Rust in 0/20.
   Impact: any two concurrent editors (browser tab + Neovim/Syncthing, two tabs, autosave + external edit) can
   silently overwrite each other. Also `relink.prepare` uses `Read`, so a rename's link rewrite can overwrite a
   note saved a moment earlier: under 16 concurrent editors, **3-5 accepted edits out of ~7,300 vanished in
   every Go run** (`loadgen consistency`, check "relink: no accepted edit lost"), 0 in Rust.
   Fix: read the mtime from the open file (`f.Stat()`), as the Rust port does.
2. **`Create` vs `Move` onto the same name silently destroys a file.** `Move` does `Lstat(to)` (must not exist)
   then `Rename`; `Create` (O_EXCL) does not take the vault mutex, so a note created in between is replaced by the
   move: both requests answer success. `bench/repro/create_vs_move.py`: 31 of 200 barrier-synchronised rounds
   (15%); the contract suite saw 12-24% of 150 rounds and `loadgen consistency` 36-48% of 200. The source code
   calls it a "known limitation" (`os.Root` has no `RENAME_NOREPLACE`); `renameat2` through a directory handle
   closes it (Rust: 0).
3. **`GET /api/notes/<directory>` returns 500 and leaks the absolute vault path** in the error text
   (`read /home/.../Estudos: is a directory`). A client mistake should be a 4xx and the path should not be
   disclosed. Same shape of problem (500 for a client error): saving through a path whose parent is a file
   (`mkdirat Welcome.md: file exists`), saving over a directory named `x.md`, and a 70 KB path.
4. **App-level (both servers, same code path):** a plain `echo > vault/new.md` is coalesced with its `write` by
   the 150 ms debounce into a single `write` event, and the frontend only refreshes the tree on non-`write`
   events, so the new note does not appear in the sidebar until something else changes (seen in
   `bench/e2e/frontend.mjs`, which therefore creates the file with temp file + rename like editors do).

### In the Rust code, found while building it (all fixed, kept as history)

* Relink rewrite dropped the `.md` capture group (caught by the ported unit test).
* The watcher followed a symlinked directory out of the vault (found by the contract suite).
* **Async mutex + `spawn_blocking` for saves** made the contended critical section queue in the blocking pool
  behind hundreds of reads: 1000 workers read-modify-write took **140.8 s (462 CPU-s)** against Go's 12.8 s.
  Replaced by a single writer thread fed by a queue: 2.5 s. (`results/smoke-async-mutex-design.json`)
* **`notify` subscribes to `IN_OPEN`**: every file the server or `rg` opened raised an inotify event.
  Memory grew ~250 KB per search and a tree read peaked at **557 MB** (search 115 MB, backlinks 133 MB) in the
  first full run (`results/raw-run1-first-design.json`, `tables-run1-first-design.md`); with plain `inotify` and
  fsnotify's mask the same scenarios peak at 79 / 17 / 15 MB.
* Smaller: Range/304/416 header rules, MIME table, `Allow` lists, invalid-UTF-8 paths, JSON `null`, and
  case-insensitive keys; each was found as a contract divergence and fixed.

## 6. Honest conclusions

* **Correctness first.** The three data-integrity problems live in the *design* of the Go code (two
  non-atomic read/check-then-act sequences), not in Go as a language; a Go fix is a few lines (`f.Stat()` on the
  open file; `renameat2` through `golang.org/x/sys/unix` or a per-path lock in `Create`). The Rust port avoided them
  mostly because the contention benchmark forced me to look. Do not read "Rust has no lost updates" as a
  property of Rust.
* **Where Rust is clearly better:** memory at rest (2-3x less), memory per SSE connection, start time, per-request
  CPU on saves (2x), the contended same-note workload (3.5x faster), the 1 MB tree response (2.6x more requests
  per CPU-second), tail latency of small reads, binary size (-32%).
* **Where it is the same:** correctness of the same-version race, search and backlinks (bound by `rg` in both;
  I deliberately kept `rg` as the engine), SSE delivery and latency, trash/move behaviour, small-note read
  throughput within a factor of 1.3.
* **Where Go is better or Rust is not "free":**
  * clean build 8 s vs 73 s (and an edit-compile loop with `lto=fat` of ~1 minute; `cargo check` is fast but
    benchmarks need release builds);
  * **half the code**: Go's `net/http` gives ServeContent, the mux behaviours, `encoding/json` semantics and
    the MIME table for free; the Rust port needed ~850 lines (`api/content.rs`, `api/json.rs`, `api/spa.rs`, `golike.rs`) to reproduce them;
  * **memory under mixed load and after bursts** with the default allocator (82 MB vs 42 MB peak in the soak;
    Rust keeps what it grew, Go returns it), fixable but with a throughput cost (`MALLOC_ARENA_MAX`);
  * the two performance traps I fell into (`spawn_blocking` queue behind a lock, `notify`'s `IN_OPEN`) are the
    kind of thing that is easy to write and invisible in unit tests; the equivalent naive Go code (lock, then file calls on the request goroutine) was already fast;
  * no `ReadHeaderTimeout` and a few cosmetic differences (section 3).
* **Is the migration worth it?** For a single-user notes app on localhost, nothing here is user-visible: both
  answer in well under a millisecond and use tens of MB. The reasons to consider Rust would be the memory
  footprint on a small always-on machine and the safety margin under heavy contention; the reasons against are
  build time, code size and maintenance cost. The correctness findings are worth acting on **in the Go code
  regardless**.

## 7. What was not verified / limits

* One machine, loopback only, tmpfs vaults (no real-disk effects, no fsync), synthetic content, 3 repetitions
  (spreads are shown; some single runs were noisy, e.g. the first start of a series), a busy desktop with ~1 GB
  of free RAM, a load generator in Go on the same host.
* No TLS/HTTP/2/remote clients, no Windows/macOS, no long soak beyond 90 s, no real-world vault (forbidden).
* The coarse-clock mtime tie is a theoretical hole I could not trigger on this kernel/filesystem; the guard
  was therefore not shown to be necessary. I also could not run on ext4/btrfs/NFS.
* The repository's own Playwright suite (`npm test`) mocks the whole API and is independent of the backend, so
  it says nothing about the Rust server; `bench/e2e/frontend.mjs` is the real-backend check.
* Allocator alternatives were tried only on an earlier build; `MALLOC_ARENA_MAX` was measured on reads only.
* The search benchmark mostly measures `rg`; a native in-process search would be a different experiment.

## 8. Reproduce

```sh
# 0. frontend (embedded by both servers) and builds
cd web && npm ci && npm run build && cd ..
mkdir -p /tmp/qbench/bin
go build -trimpath -ldflags="-s -w" -o /tmp/qbench/bin/quirenote-go ./cmd/quirenote
(cd rust && cargo build --release && cp target/release/quirenote-rs /tmp/qbench/bin/)
(cd bench/loadgen && go build -o /tmp/qbench/bin/loadgen .)

# 1. Rust checks
(cd rust && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check)

# 2. contract suite against both servers, then compare (ports 9110/9111, vaults under /tmp/qbench)
bench/contract/run.sh go /tmp/qbench/bin/quirenote-go 9110
bench/contract/run.sh rs /tmp/qbench/bin/quirenote-rs 9111
python3 bench/contract/contract.py diff /tmp/qbench/contract-go/run.json /tmp/qbench/contract-rs/run.json
python3 bench/contract/contract.py diff /tmp/qbench/contract-go/setup.json /tmp/qbench/contract-rs/setup.json

# 3. the real frontend in Chromium (needs web/node_modules)
bench/e2e/run.sh rs /tmp/qbench/bin/quirenote-rs 9120

# 4. synthetic vaults, then the benchmark (about 20 minutes), then the tables
python3 bench/gen/genvault.py --out /tmp/qbench/data/small/vault --index /tmp/qbench/data/small --notes 200   --pdfs 4  --images 8
python3 bench/gen/genvault.py --out /tmp/qbench/data/large/vault --index /tmp/qbench/data/large --notes 10000 --pdfs 40 --images 80
python3 bench/run_bench.py --server go=/tmp/qbench/bin/quirenote-go --server rs=/tmp/qbench/bin/quirenote-rs --reps 3 --out /tmp/qbench/results/raw.json
python3 bench/run_bench.py --server rsc=/tmp/qbench/bin/quirenote-rs --env rsc:QUIRENOTE_RS_COARSE_MTIME=1 --reps 3 --scenarios rmw,writes --out /tmp/qbench/results/raw-coarse.json
python3 bench/aggregate.py /tmp/qbench/results/raw.json /tmp/qbench/results/raw-coarse.json > tables.md
python3 bench/soak.py --server go=/tmp/qbench/bin/quirenote-go --server rs=/tmp/qbench/bin/quirenote-rs --seconds 90

# 5. the Go bugs, one at a time against a server on a temporary vault
python3 bench/repro/lost_update.py  --url http://127.0.0.1:PORT --vault VAULT
python3 bench/repro/create_vs_move.py --url http://127.0.0.1:PORT
python3 bench/repro/mtime_tie.py     --url http://127.0.0.1:PORT
```

All scripts refuse to run outside `/tmp/qbench` or on ports outside 9100-9199 (`bench/lib.sh`,
`run_bench.py`). The clean-build timings were taken with
`GOCACHE=$(mktemp -d) go build ...` and `CARGO_TARGET_DIR=$(mktemp -d) cargo build --release --offline`.
