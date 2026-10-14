//! `quirenote-rs`: the Quirenote server (Rust/Axum port of cmd/quirenote).

#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quirenote::api::{self, AppState};
use quirenote::config::{self, Config};
use quirenote::setup;
use quirenote::vault::{Vault, go_errno};
use quirenote::watch::{self, Broker};
use tokio::net::TcpSocket;
use tokio_util::sync::CancellationToken;

fn usage(cfg_path: &str) -> String {
    format!(
        "Usage:
  quirenote-rs [-vault DIR] [-addr HOST:PORT] [-debug]   start the server
  quirenote-rs init [DIR]                                 choose the vault (shows it without DIR)

The vault comes from -vault, then QUIRENOTE_VAULT, then the config file
({cfg_path}). Without any of them, the browser shows a first-run screen
to start a new vault or use an existing folder (an Obsidian vault works as is).

Flags:
  -addr string
    \tHTTP address (use the NetBird/Tailscale IP for remote access) (default \"127.0.0.1:8080\")
  -debug
    \tverbose logging
  -vault string
    \tvault directory (overrides the config file)
"
    )
}

/// slog's text handler quotes values that contain spaces or quotes.
/// Opens the vault; `QUIRENOTE_RS_COARSE_MTIME=1` turns off the strictly-increasing mtime of
/// saves (restores Go's behaviour, used by the benchmark to price the guard).
fn open_vault(dir: &str) -> std::io::Result<Vault> {
    let coarse = std::env::var("QUIRENOTE_RS_COARSE_MTIME").is_ok_and(|x| x == "1");
    Vault::open_with(dir, !coarse)
}

fn kv(v: &str) -> String {
    if v.is_empty() || v.contains([' ', '"', '=']) {
        config::go_quote(v)
    } else {
        v.to_string()
    }
}

fn info(msg: &str, kvs: &[(&str, &str)]) {
    let mut line = format!("{} INFO {msg}", quirenote::log_time());
    for (k, v) in kvs {
        line.push_str(&format!(" {k}={}", kv(v)));
    }
    eprintln!("{line}");
}

fn fatal(err: &str) -> ExitCode {
    eprintln!("{} ERROR quirenote err={}", quirenote::log_time(), kv(err));
    ExitCode::from(1)
}

fn env(key: &str, old: &str, fallback: &str) -> String {
    for k in [key, old] {
        if let Ok(v) = std::env::var(k)
            && !v.is_empty()
        {
            return v;
        }
    }
    fallback.to_string()
}

struct Flags {
    addr: String,
    vault: String,
    debug: bool,
}

/// Go `flag`-style parsing: `-name value`, `-name=value`, `--name`.
fn parse_flags(args: &[String], cfg_path: &str) -> Result<Flags, ExitCode> {
    let mut f = Flags {
        addr: env("QUIRENOTE_ADDR", "NOTAS_ADDR", "127.0.0.1:8080"),
        vault: env("QUIRENOTE_VAULT", "NOTAS_VAULT", ""),
        debug: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a.trim_start_matches('-');
        if !a.starts_with('-') || name.is_empty() {
            break;
        }
        let (name, inline) = match name.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (name, None),
        };
        let mut value = |what: &str| -> Result<String, ExitCode> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| {
                    eprintln!("flag needs an argument: -{what}");
                    ExitCode::from(2)
                })
        };
        match name {
            "addr" => f.addr = value("addr")?,
            "vault" => f.vault = value("vault")?,
            "debug" => f.debug = inline.as_deref().is_none_or(|v| v != "false"),
            "h" | "help" => {
                eprint!("{}", usage(cfg_path));
                return Err(ExitCode::SUCCESS);
            }
            other => {
                eprintln!("flag provided but not defined: -{other}");
                eprint!("{}", usage(cfg_path));
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(f)
}

fn main() -> ExitCode {
    let cfg_path = match config::path() {
        Ok(p) => p,
        Err(e) => return fatal(&e),
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "init") {
        return match init_vault(&cfg_path, &args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fatal(&e),
        };
    }
    let flags = match parse_flags(&args, &cfg_path.to_string_lossy()) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let mut dir = flags.vault.clone();
    if dir.is_empty() {
        match config::load(&cfg_path) {
            Ok(c) => dir = c.vault,
            Err(e) => return fatal(&e),
        }
    }
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return fatal(&e.to_string()),
    };
    match rt.block_on(run(&flags.addr, &dir, &cfg_path, flags.debug)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fatal(&e),
    }
}

/// `quirenote init [DIR]`: an existing folder is used as is, a missing one is created with a
/// welcome note.
fn init_vault(cfg_path: &std::path::Path, args: &[String]) -> Result<(), String> {
    let cfg = config::load(cfg_path)?;
    let Some(dir) = args.first() else {
        if cfg.vault.is_empty() {
            println!("No vault configured. Run: quirenote-rs init <folder>");
        } else {
            println!("Vault: {}\nConfig: {}", cfg.vault, cfg_path.display());
        }
        return Ok(());
    };
    let home = std::env::var("HOME").map_err(|_| "$HOME is not defined".to_string())?;
    let res = setup::auto(dir, &home)?;
    config::save(
        cfg_path,
        &Config {
            vault: res.dir.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    println!("Vault: {}", res.dir);
    if res.obsidian {
        println!("Obsidian vault detected: your notes keep working in Obsidian.");
    }
    println!(
        "Saved to {}. Restart the server to use it (systemctl --user restart quirenote).",
        cfg_path.display()
    );
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

async fn run(addr: &str, dir: &str, cfg_path: &std::path::Path, debug: bool) -> Result<(), String> {
    use axum::serve::ListenerExt;

    let shutdown = CancellationToken::new();
    let broker = Broker::new();
    let watchers: Arc<Mutex<Vec<watch::Handle>>> = Arc::new(Mutex::new(Vec::new()));
    let serve_watch = {
        let (broker, watchers) = (broker.clone(), Arc::clone(&watchers));
        move |dir: &std::path::Path| match watch::spawn(dir.to_path_buf(), broker.clone()) {
            Ok(h) => {
                if let Ok(mut w) = watchers.lock() {
                    w.push(h);
                }
            }
            Err(e) => eprintln!(
                "{} ERROR watch err={}",
                quirenote::log_time(),
                kv(&e.to_string())
            ),
        }
    };

    let mut vault = None;
    if !dir.is_empty() {
        let v = open_vault(dir).map_err(|e| {
            format!(
                "opening the vault: open {dir}: {} (choose another one with `quirenote init <folder>`)",
                go_errno(&e)
            )
        })?;
        serve_watch(v.dir());
        vault = Some(Arc::new(v));
    }

    let state = AppState::new(vault.clone(), broker, shutdown.clone(), debug);
    if vault.is_none() {
        let home = std::env::var("HOME").map_err(|_| "$HOME is not defined".to_string())?;
        let st = Arc::clone(&state);
        let cfg_file = cfg_path.to_path_buf();
        state.enable_setup(
            home,
            Box::new(move |res: setup::Prepared| {
                let nv = open_vault(&res.dir)
                    .map_err(|e| format!("open {}: {}", res.dir, go_errno(&e)))?;
                config::save(
                    &cfg_file,
                    &Config {
                        vault: res.dir.clone(),
                    },
                )
                .map_err(|e| e.to_string())?;
                serve_watch(nv.dir());
                st.set_vault(Arc::new(nv));
                info(
                    "quirenote",
                    &[("vault", &res.dir), ("config", &cfg_file.to_string_lossy())],
                );
                Ok(())
            }),
        );
        info(
            "quirenote: no vault configured; open the app to choose one",
            &[("config", &cfg_path.to_string_lossy())],
        );
    }

    let sock_addr: std::net::SocketAddr = tokio::net::lookup_host(addr)
        .await
        .map_err(|e| format!("listen tcp {addr}: {e}"))?
        .next()
        .ok_or_else(|| format!("listen tcp {addr}: no such host"))?;
    let socket = if sock_addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .map_err(|e| e.to_string())?;
    socket.set_reuseaddr(true).map_err(|e| e.to_string())?;
    socket
        .bind(sock_addr)
        .map_err(|e| format!("listen tcp {addr}: {}", go_errno(&e)))?;
    // A deep accept queue, like Go's (the kernel caps it at net.core.somaxconn).
    let listener = socket
        .listen(4096)
        .map_err(|e| e.to_string())?
        .tap_io(|tcp| {
            let _ = tcp.set_nodelay(true);
        });

    info(
        "quirenote",
        &[("addr", &format!("http://{addr}")), ("vault", dir)],
    );

    let app = api::router(state);
    let token = shutdown.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown_signal().await;
        token.cancel();
    });
    tokio::select! {
        r = server => r.map_err(|e| e.to_string()),
        () = async {
            shutdown.cancelled().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        } => Ok(()),
    }
}
