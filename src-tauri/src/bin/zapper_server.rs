//! zapper-server: the recording service. See docs/recording.md.
//!
//! Headless: no window, no webview, no libmpv. It keeps the catalogue and
//! guide current like the app does, wakes to record what was scheduled, and
//! answers the app (and, later, a browser) over HTTP on 127.0.0.1.
//!
//! ```text
//! zapper-server [--port N] [--no-sync]
//! ```
//!
//! `--no-sync` leaves the catalogue to whoever else keeps it (the app on the
//! same machine, or a harness that wrote one).

use std::sync::Arc;

use anyhow::{Context, Result};
use zapper_lib::catalog::sync_engine::{self, StatusSink};
use zapper_lib::ipc::types::SyncStatusDto;
use zapper_lib::recording::budget::SourceBudget;
use zapper_lib::recording::{config, http, server::Server, store};

/// The sync's progress, to the log: nobody is watching a toolbar.
struct LogSink;

impl StatusSink for LogSink {
    fn publish(&self, s: SyncStatusDto) {
        tracing::info!("catalogue: {:?} {}", s.phase, s.message);
    }
}

struct Args {
    port: Option<u16>,
    sync: bool,
}

fn args() -> Result<Option<Args>> {
    let mut a = Args {
        port: None,
        sync: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                let v = it.next().context("--port needs a number")?;
                a.port = Some(v.parse().context("--port needs a number")?);
            }
            "--no-sync" => a.sync = false,
            "--version" | "-V" => {
                println!("zapper-server {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                println!(
                    "zapper-server {}: zapper's recording service\n\n\
                     Usage: zapper-server [--port N] [--no-sync]\n\n  \
                     --port N    listen on 127.0.0.1:N (default: server.json's, else {})\n  \
                     --no-sync   do not keep the catalogue current from here\n\n\
                     Settings: {}\nAPI token: {}",
                    env!("CARGO_PKG_VERSION"),
                    config::DEFAULT_PORT,
                    config::config_path().display(),
                    config::token_path().display()
                );
                return Ok(None);
            }
            other => anyhow::bail!("unknown argument {other:?}; see --help"),
        }
    }
    Ok(Some(a))
}

/// The token on file, or a new one written there.
fn token() -> Result<String> {
    if let Some(t) = config::read_token() {
        return Ok(t);
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| anyhow::anyhow!("no randomness for the API token: {e}"))?;
    let t: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    config::write_token(&config::token_path(), &t)?;
    Ok(t)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let Some(args) = args()? else { return Ok(()) };

    let cfg_path = config::config_path();
    let mut cfg = config::load_from(&cfg_path);
    if let Some(p) = args.port {
        cfg.port = p;
    }
    if !cfg_path.exists() {
        // So the app finds the port, and the person finds the settings.
        config::save_to(&cfg_path, &cfg)?;
    }
    let token = token()?;
    let db = store::open(&zapper_lib::paths::data_dir().join("recordings.db"))?;
    let port = cfg.port;
    // How many streams a playlist allows is the app's setting, read from
    // the catalogue the two share; see `recording::budget`.
    let budget = Box::new(SourceBudget);
    let server = Server::new(db, cfg, cfg_path, token, budget);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        if args.sync {
            tokio::spawn(sync_engine::periodic(LogSink));
        }
        tokio::spawn(Arc::clone(&server).run_scheduler());
        tokio::select! {
            r = http::serve(server, port) => r,
            _ = shutdown() => {
                // Captures in progress keep their parts and their state: the
                // next start resumes or finishes them.
                tracing::info!("stopping");
                Ok(())
            }
        }
    })
}

async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
