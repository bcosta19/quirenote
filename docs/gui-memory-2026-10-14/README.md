# Historical GUI memory comparison

This directory preserves the experiment completed before the native application became the default. The report and raw data describe those benchmark binaries, rather than the rebuilt main binaries. The fixes to relink, formatting, and lints made during promotion are recorded separately in [the promotion report](../rust-promotion-2026-10-14.md).

The Go/Svelte application is preserved on `backup/go-svelte-2026-10-14`; backend comparison scripts and corrected Go are preserved on `bench/rust-axum-vs-go`. The original GUI harness and its generated books remain in the local `bench/rust-gui` worktree under `bench/gui/`. Run comparisons in that worktree; main builds only the Rust application.

Current validation: `make check`, `make build`, and the native flow checker described in the repository AGENTS.md.

Copied text logs have trailing whitespace normalized; the original logs remain in the benchmark worktree. JSON data and image bytes are unchanged.
