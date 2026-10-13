# quirenote-rs

A Rust/Axum reimplementation of the Quirenote backend (`cmd/` + `internal/` in the Go tree),
written as an **experiment**: same HTTP contract, side by side with the Go server, so the two can
be compared on correctness and cost. The Svelte frontend (`web/`) is served unchanged, embedded in
the binary exactly as the Go version does with `go:embed`.

It is not wired into the Makefile, the systemd unit or the installer. The original experiment
left the Go tree unchanged; this branch now includes Go integrity fixes and the
[comparison with corrected Go](../docs/backend-comparison-2026-10-13.md).

```
cd web && npm ci && npm run build        # fills web/dist, which gets embedded
cd rust && cargo build --release         # target/release/quirenote-rs
./target/release/quirenote-rs -vault DIR -addr 127.0.0.1:8080 [-debug]
./target/release/quirenote-rs init [DIR] # same CLI as the Go binary (flags, QUIRENOTE_* env, config.toml)
```

Release profile: `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `strip = true`.

## Checks

```
cargo test                                  # 36 unit tests + 4 API tests (ports of the Go tests)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`#![deny(clippy::unwrap_used, clippy::expect_used)]` is on for the whole crate (relaxed only in
`cfg(test)`): no `unwrap()`/`expect()` can sit on a request path.

The black-box contract suite and the benchmarks live in [`../bench`](../bench/RESULTS.md).

## Layout (mirrors the Go packages)

| Rust | Go | What |
|---|---|---|
| `src/vault.rs` | `internal/vault` | Confined file access, atomic save, move, trash, images, tree |
| `src/search.rs` | `internal/search` | `rg --json` search, backlinks, snippet cutting |
| `src/relink.rs` | `internal/relink` | `[[link]]` rewriting after a rename |
| `src/setup.rs`, `src/config.rs` | `internal/setup`, `internal/config` | First-run setup, `config.toml` |
| `src/watch.rs` | `internal/watch` | inotify watcher + SSE broker, 150 ms debounce |
| `src/api.rs`, `src/api/*` | `internal/api`, `web` | Routes, middleware, JSON, `ServeContent`, embedded SPA |
| `src/main.rs` | `cmd/quirenote` | CLI, listener, graceful shutdown |
| `src/golike.rs` | `path`, `io/fs`, `strings`, `regexp` | The few Go helpers whose exact behaviour is part of the contract |

## Choices, and why

* **axum 0.8 on tokio, `http1` only.** The task asks for Axum; HTTP/2 stays off because Go serves
  plain-text HTTP/1.1 too. No `axum::Json`/`Path`/`Query` extractors for request data: Go's
  `encoding/json` and `net/http` have observable quirks (see "Parity" below) and a thin hand-written
  layer reproduces them exactly. Axum is used for routing, middleware and the server.
* **`cap-std` for the vault.** Go's `os.Root` has no direct equivalent in `std`; cap-std is the
  capability-style `Dir` handle that resolves `../`, absolute paths and symlinks *inside* the vault
  in the kernel (openat2-style), so path traversal is blocked at the same layer as in Go.
  `rustix` is used for `renameat2(RENAME_NOREPLACE)` on top of cap-std directory handles.
* **Plain `inotify` (the `inotify` crate) for the watcher, not `notify`.** Directories are watched
  one by one with hidden ones skipped, so `.obsidian`/`.trash`/`.git` never cost a watch (the Go
  code does the same with fsnotify), using exactly fsnotify's event mask and op names
  (`create|write|remove|rename`), debounced per path for 150 ms. The first version used `notify`,
  which also subscribes to `IN_OPEN`: every file opened by the server or by `rg` raised an event,
  a search over 10,000 notes meant ~30,000 useless events, and under load RSS grew by ~250 KB per
  search and ~70 MB per concurrent tree read (found by the benchmark; fixed by owning the mask).
* **`tokio::sync::broadcast` for SSE**: one ring buffer of 32 events shared by all clients instead
  of a channel per client; a slow client just loses events, like Go's non-blocking send. This keeps
  the per-connection cost tiny.
* **`rust-embed` for `web/dist`** (a single binary, like `go:embed`). `build.rs` creates the folder
  if it is missing so the crate always compiles; the binary then warns at start, as Go does.
* **`regex`** for `relink` (leftmost-first semantics, same as RE2); the search patterns are handed
  to `rg`, whose engine is also Rust's `regex`.
* **Search and backlinks shell out to `rg`, like the Go server.** A native searcher would be a
  different experiment (and would make the benchmark measure two different search engines). Both
  servers spawn `rg` with identical arguments, so those scenarios mostly measure process
  spawning and `rg` itself.
* **`serde_json` with a custom formatter** that escapes `<`, `>`, `&`, U+2028/9 like Go's
  `json.Encoder`, so response bodies are byte-identical to Go's for normal content.
* **`chrono`** (clock + `strftime`) for the `Pasted image YYYYMMDDhhmmss` and trash names;
  **`getrandom`** for the temp-file suffix; **`percent-encoding`**, **`httpdate`**, **`base64`**
  as the small obvious choices. No `unsafe` in this crate.
* **One dedicated writer thread for save/move/trash.** These are check-then-write operations that
  Go serialises with a `sync.Mutex`. The first version used an async mutex and then
  `spawn_blocking`; with 1000 clients doing read-modify-write on one note the critical section sat
  in the blocking pool's FIFO queue behind hundreds of reads and a 2000-save run took 140 s (Go:
  13 s). A single consumer thread fed by a queue gives the same mutual exclusion with FIFO
  fairness and no hand-off, and the same run takes ~2.5 s. Reads, listings, creates and image saves
  stay on the blocking pool.

## Parity with the Go contract

Same routes, status codes, JSON, headers; the black-box suite (`bench/contract`) compares 378 HTTP
cases + 33 first-run cases against the Go server and finds them identical except for four
documented items (`bench/contract/known_divergences.json`). Notably reproduced on purpose:

* the 415 check on every non-GET/HEAD request, case-sensitive on `application/json`;
* mux behaviour: 307 redirects for unclean paths and `/api/notes` → `/api/notes/`, `GET` on any
  unknown path falls through to the SPA, 405 + `Allow` lists, `HEAD`;
* `encoding/json` behaviour: first value only (trailing garbage ignored), `null` = zero value,
  case-insensitive field names, `mtime` as a string, error texts;
* `ServeContent`: Go's MIME table (built-in + the system shared-MIME database), `Range`
  (single range), `If-Modified-Since`, `If-Range`, 416 rules, `Last-Modified`;
* error texts of OS failures (`openat x: no such file or directory`), including the absolute-path
  leak of `GET /api/notes/<dir>` (500) that the Go server has;
* save conflict (409 with the version on disk), stale-save-after-move/trash is a 404, atomic write
  (`.<name>.quirenote-<hex>.tmp` + rename), file permissions preserved, trash names, Obsidian
  attachment-folder rules, the first-run setup flow and config file format.

### Integrity choices compared with the original Go baseline

The corrected Go on this branch also reads through one descriptor and uses no-replace
rename on Linux. The descriptions below refer to the original Go baseline; the
[new report](../docs/backend-comparison-2026-10-13.md) records the remaining limits and Rust relink failures.

1. **`GET /api/notes/<path>` reads content and mtime from the same file descriptor.** Go reads the
   file and then `stat`s the path, so a save landing in between pairs *old content with the new
   mtime*; the client then saves over the newer version with an accepted `mtime` (a lost update,
   reproduced in `bench/repro/lost_update.py`).
2. **Saves get a strictly increasing mtime** (`max(now, previous+1ns)` set on the temp file before
   the rename). Every atomic save is a new inode stamped with the kernel's coarse clock, so in
   principle two saves in the same tick carry the same mtime and defeat the stale-version check.
   On the benchmark machine (kernel 7.2, tmpfs) I could not provoke a tie even against the Go
   server (`bench/repro/mtime_tie.py`, 3000 rounds, 0 ties), so this is defence in depth for older
   kernels and other filesystems, not a measured fix; it costs nothing measurable (writes/s are the
   same with it off). `QUIRENOTE_RS_COARSE_MTIME=1` turns it off.
3. **`move` and `trash` use `renameat2(RENAME_NOREPLACE)`**, closing the window in which Go's
   `Lstat`-then-`Rename` can silently overwrite a file created in between (`Create` is not under
   the mutex in Go). Filesystems without support fall back to a plain rename.

### Known gaps

* A `Range` header with several ranges is answered with the whole file (200), not
  `multipart/byteranges`.
* Requesting an embedded directory (`/assets/`) returns `index.html`, Go lists the directory.
* No `ReadHeaderTimeout` (Go sets 10 s): `axum::serve` does not expose hyper's header timeout.
  Behind a reverse proxy or on localhost this does not matter; on an open port a slow-loris client
  would keep a connection open.
* hyper limits the request target to ~64 KB (414), where Go routes it and fails in the file system
  (500).
* The error text for some rare OS failures differs (`file exists` vs `is a directory` when a save
  targets a directory named `x.md`).
* Not memory-tuned: the default glibc allocator keeps freed memory after a burst, so RSS after a
  heavy load falls back less than Go's does (numbers in `bench/RESULTS.md`).
