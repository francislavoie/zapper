//! zapper-server: the recording service. See docs/recording.md.
//!
//! Headless: no window, no webview, no libmpv. It keeps the catalogue and
//! guide current like the app does, wakes to record what was scheduled, and
//! answers the app over HTTP on 127.0.0.1 (and other devices on the network,
//! when Settings › Recording allows them).
//!
//! ```text
//! zapper-server [--port N] [--no-sync] [--home DIR]
//! ```
//!
//! `--no-sync` leaves the catalogue to whoever else keeps it (the app on the
//! same machine, or a harness that wrote one). `--home` keeps everything in
//! one folder, as `ZAPPER_HOME` does (crate::paths): a Windows scheduled
//! task cannot set the environment of what it starts.
//!
//! On Windows it is a windowless program, so that the task that starts it at
//! sign-in (Settings › Recording) opens no console window. Started from a
//! terminal, it prints to that terminal's console; started by the task, with
//! nowhere to print, it logs to `zapper-server.log` in the data folder.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::sync::Arc;

use anyhow::{Context, Result};
use zapper_lib::catalog::sync_engine::{self, StatusSink};
use zapper_lib::ipc::types::SyncStatusDto;
use zapper_lib::recording::budget::SourceBudget;
use zapper_lib::recording::{config, server::Server, store};

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
            "--home" => {
                let v = it.next().context("--home needs a folder")?;
                // Before anything has read a path, and before any thread
                // exists: paths are read from the environment each time.
                std::env::set_var(zapper_lib::paths::HOME_OVERRIDE, v);
            }
            "--version" | "-V" => {
                println!("zapper-server {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                println!(
                    "zapper-server {}: zapper's recording service\n\n\
                     Usage: zapper-server [--port N] [--no-sync] [--home DIR]\n\n  \
                     --port N    listen on port N (default: server.json's, else {})\n  \
                     --no-sync   do not keep the catalogue current from here\n  \
                     --home DIR  keep settings, catalogue and recordings' data in DIR\n\n\
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
    let console = console::attach();
    let Some(args) = args()? else { return Ok(()) };
    match (!console).then(log_file).flatten() {
        Some(file) => tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init(),
        None => tracing_subscriber::fmt::init(),
    }

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
        // On 127.0.0.1, and on the network too when Settings says so; a
        // change there replaces the listeners while this keeps running.
        server.listen().await?;
        shutdown().await;
        // Captures in progress keep their parts and their state: the next
        // start resumes or finishes them.
        tracing::info!("stopping");
        Ok(())
    })
}

/// `zapper-server.log` in the data folder, for a service with no console
/// (Windows' task). Started afresh past a few megabytes, the last one kept
/// beside it as `zapper-server.old.log`.
fn log_file() -> Option<std::fs::File> {
    let dir = zapper_lib::paths::data_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("zapper-server.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 4 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("zapper-server.old.log"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Where a windowless program prints.
#[cfg(windows)]
mod console {
    use windows::core::w;
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_HANDLE, STD_OUTPUT_HANDLE,
    };

    fn given(id: STD_HANDLE) -> bool {
        unsafe { GetStdHandle(id) }.is_ok_and(|h| !h.is_invalid() && !h.0.is_null())
    }

    /// Whether there is anywhere to print: output handed down (a pipe or a
    /// file: a harness, a script capturing it), or the console of the
    /// terminal this was started from, attached to here for whichever of
    /// stdout and stderr was not handed down. Neither when the task starts
    /// it.
    pub fn attach() -> bool {
        let (out, err) = (given(STD_OUTPUT_HANDLE), given(STD_ERROR_HANDLE));
        if out && err {
            return true;
        }
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_err() {
            return err;
        }
        let Ok(console) = (unsafe {
            CreateFileW(
                w!("CONOUT$"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }) else {
            return err;
        };
        // Rust's stdout and stderr ask for these handles on every write.
        unsafe {
            if !out {
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, console);
            }
            if !err {
                let _ = SetStdHandle(STD_ERROR_HANDLE, console);
            }
        }
        true
    }
}

/// Elsewhere a program always has its terminal's, or the journal's.
#[cfg(not(windows))]
mod console {
    pub fn attach() -> bool {
        true
    }
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
