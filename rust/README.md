# Quirenote backend

Rust/Axum backend for the native interface in `../gui`. The default application, Makefile, launcher and service use this backend. It serves the HTTP API and does not embed a web frontend or require Node/Go to build.

```sh
cargo build --locked --release
./target/release/quirenote-rs -addr 127.0.0.1:8080
./target/release/quirenote-rs init ~/Notes
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Use `--offline` with cached dependencies. The crate keeps the existing vault/config format and API contracts used by the native GUI. `cap-std` confines vault access; atomic saves carry a version string and reject stale edits. One writer queue orders save, rename and trash operations. Rename relinking takes a snapshot of all regular, visible notes on that queue; conditional writes report concurrent changes as skipped. Search and backlinks still use ripgrep. inotify events feed SSE updates.

The previous Go implementation is preserved on `backup/go-svelte-2026-10-14`. The original port and experiments remain on the benchmark branches. Reports in `../docs` describe their historical binaries, which may differ from main.

The backend has no login; its default listener is loopback. It supports a single HTTP range rather than multipart ranges and runs on Linux. The release profile uses fat LTO, one codegen unit and stripped symbols.
