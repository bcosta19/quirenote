# Verification

- The main application is Rust: backend in `rust/`, native egui interface in `gui/`. Go/Svelte is preserved on `backup/go-svelte-2026-10-14`; run legacy checks in a separate checkout.
- `make check` runs rustfmt for both crates, Clippy on all targets with warnings as errors, all backend/GUI tests, and shell syntax checks.
- `make build` compiles both release binaries into `bin/quirenote` and `bin/quirenote-gui`. Cargo uses one job by default to reduce memory pressure.
- With cached dependencies use `CARGO_FLAGS="--offline --locked"`. `SERVER_TARGET_DIR` and `GUI_TARGET_DIR` can reuse existing disk caches. Do not put large compilation caches in `/tmp` when it is tmpfs.
- Run GUI rendering and native flow checks separately from compilation/static analysis. Use a private Wayland compositor and a synthetic vault; never point automated editing checks at the real notes vault.
- `scripts/verify-native.py` checks autosave, restoring a selected note, external changes followed by editing, PDF and EPUB reading. It accepts a private compositor environment/FIFO and synthetic document fixtures; screenshots must be inspected before claiming rendered UI correctness.
- `make check-wayland WAYLAND_ENV=/path/to/private/env.sh SWAYMSG=/path/to/swaymsg` runs the hidden-workspace liveness regression. Use a private Sway config assigning `quirenote-gui` to workspace 1 with `no_focus [app_id="quirenote-gui"]`. The test keeps workspace 2 visible and requires the hidden application to process Wayland's close event within two seconds. Never run it against the user's compositor.
- Stress the concurrent relink regression with `cargo test --manifest-path rust/Cargo.toml run_accounts_for_all_references_during_atomic_saves -- --test-threads=1` in repeated invocations. Preserve accepted edits and account for each reference as updated or skipped.
- Inspect actual checkout and runtime, preserve user changes, keep secrets out of output, and do not commit/push/install externally unless authorized.
