//! Quirenote backend in Rust (Axum): a port of the Go server to compare both side by side.
//!
//! Module map (mirrors the Go packages): `vault` = internal/vault, `search` =
//! internal/search, `relink` = internal/relink, `setup` = internal/setup, `config` =
//! internal/config, `watch` = internal/watch, `api` = internal/api + `web`.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod api;
pub mod config;
pub mod golike;
pub mod relink;
pub mod search;
pub mod setup;
pub mod vault;
pub mod watch;

/// Timestamp in the format of Go's default `log`/`slog` output (local time).
pub fn log_time() -> String {
    chrono::Local::now().format("%Y/%m/%d %H:%M:%S").to_string()
}
