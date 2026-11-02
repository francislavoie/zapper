//! zapper-server: the recording service. See docs/recording.md.
//!
//! Headless: no window, no webview, no libmpv. It keeps the catalogue and
//! guide current like the app does, wakes to record what was scheduled, and
//! answers the app over HTTP on 127.0.0.1 (and other devices on the network,
//! when Settings › Recording allows them).
//!
//! ```text
//! zapper-server [--port N] [--no-sync] [--home DIR]
//! zapper-server --install-service | --remove-service [--home DIR]
//! zapper-server --busy | --stop-for-setup [--ask] [--home DIR]
//! zapper-server --import-account FILE | --set-password | --network on|off [--home DIR]
//! ```
//!
//! `--install-service` and `--remove-service` do what Settings › Recording's
//! Install and Remove do (recording::service), for a computer without the
//! app's window, or a script. `--busy` and `--stop-for-setup` are the
//! installers' (packaging/windows/): whether the service on this computer is
//! recording, and stopping it cleanly before files are replaced, asking
//! first with `--ask` when it is.
//!
//! `--import-account`, `--set-password` and `--network` set up a service
//! that has no app beside it (a headless box, a container): the provider
//! account from an `account.json`, the password other devices sign in with
//! (read from standard input, never the command line, where other users
//! would see it in the process list), and whether it listens on the network.
//! Through the running service's API when it answers, so it takes them at
//! once; on its files otherwise, for its next start.
//!
//! It stops or restarts itself without spoiling a recording
//! (recording::lifecycle): asked through the API, now or once idle; on
//! SIGTERM or Ctrl+C, at once but finishing what it recorded into playable
//! files; and, on Linux, restarting into a newer version of itself once one
//! has been installed over it and it is idle.
//!
//! `--no-sync` leaves the catalogue to whoever else keeps it (the app on the
//! same machine, or a harness that wrote one). `--home` keeps everything in
//! one folder, as `ZAPPER_HOME` does (crate::paths): a Windows scheduled
//! task cannot set the environment of what it starts.
//!
//! On Windows it is a windowless program, so that the task that starts it at
//! sign-in (Settings › Recording) opens no console window. It logs to
//! `zapper-server.log` in the data folder (`zapper_lib::logfile`), and
//! started from a terminal, to that terminal's console as well.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use zapper_lib::catalog::sync_engine::{self, StatusSink};
use zapper_lib::ipc::types::SyncStatusDto;
use zapper_lib::recording::budget::SourceBudget;
use zapper_lib::recording::api::{ShutdownRequest, ShutdownWhen};
use zapper_lib::recording::client::Client;
use zapper_lib::recording::lifecycle::{self, Exit, FileId};
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
    let mut service: Option<bool> = None;
    let mut setup: Option<Setup> = None;
    let mut admin: Option<Admin> = None;
    let mut ask = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                let v = it.next().context("--port needs a number")?;
                a.port = Some(v.parse().context("--port needs a number")?);
            }
            "--no-sync" => a.sync = false,
            "--install-service" => service = Some(true),
            "--remove-service" => service = Some(false),
            "--busy" => setup = Some(Setup::Busy),
            "--stop-for-setup" => setup = Some(Setup::Stop),
            "--ask" => ask = true,
            "--set-password" => admin = Some(Admin::Password),
            "--import-account" => {
                let v = it
                    .next()
                    .context("--import-account needs a file (- for standard input)")?;
                admin = Some(Admin::Account(v));
            }
            "--network" => {
                let on = match it.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => anyhow::bail!("--network needs on or off"),
                };
                admin = Some(Admin::Network(on));
            }
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
            "--licenses" => {
                print_licenses();
                return Ok(None);
            }
            "--help" | "-h" => {
                println!(
                    "zapper-server {}: zapper's recording service\n\n\
                     Usage: zapper-server [--port N] [--no-sync] [--home DIR]\n       \
                     zapper-server --install-service | --remove-service [--home DIR]\n       \
                     zapper-server --import-account FILE | --set-password | --network on|off [--home DIR]\n\n  \
                     --port N            listen on port N (default: server.json's, else {})\n  \
                     --no-sync           do not keep the catalogue current from here\n  \
                     --home DIR          keep settings, catalogue and recordings' data in DIR\n  \
                     --install-service   run this zapper-server for you from now on, and now\n  \
                     --remove-service    stop it and undo that (recordings are kept)\n  \
                     --busy              say whether the running service is recording (exit 3 if so)\n  \
                     --stop-for-setup    stop it cleanly for an installer; with --ask, ask first if it records\n  \
                     --import-account F  use the provider account in F, an account.json (- reads standard input)\n  \
                     --set-password      set the password other devices sign in with, read from standard input\n  \
                     --network on|off    let other devices on the network reach it (needs the password)\n  \
                     --licenses          print the licenses of zapper-server and what is built into it\n\n\
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
    if let Some(install) = service {
        manage_service(install)?;
        return Ok(None);
    }
    if let Some(what) = setup {
        let code = for_setup(what, ask)?;
        std::process::exit(code);
    }
    if let Some(what) = admin {
        administer(what)?;
        return Ok(None);
    }
    Ok(Some(a))
}

/// The licence notices compiled in (build.rs, `notices`): the static
/// zapper-server for Linux carries them, being one file with nothing beside
/// it. A build without them says where its package put them.
fn print_licenses() {
    const NOTICES: &str = include_str!(concat!(env!("OUT_DIR"), "/notices.txt"));
    if NOTICES.is_empty() {
        println!(
            "zapper-server {} is under the MIT license. This build carries no copy of the notices: \
             zapper's packages install them (/usr/share/doc/zapper on Debian and Arch, \
             /usr/share/licenses/zapper on Fedora, licenses\\ beside zapper-server.exe on Windows), \
             and docs/licensing.md in zapper's source describes them.",
            env!("CARGO_PKG_VERSION")
        );
    } else {
        // Into a pager or `head`, which may stop reading: not an error.
        use std::io::Write;
        let _ = std::io::stdout().lock().write_all(NOTICES.as_bytes());
    }
}

/// Setting up a service with no app beside it.
enum Admin {
    /// The provider account, from this file, or `-` for standard input.
    Account(String),
    /// The sign-in password for other devices, from standard input.
    Password,
    /// Network access on or off.
    Network(bool),
}

/// `--import-account`, `--set-password`, `--network`: through the running
/// service's API when it answers (it takes the change at once, and a new
/// password signs every device out as Settings does), else on its files.
fn administer(what: Admin) -> Result<()> {
    use zapper_lib::catalog::credentials;
    use zapper_lib::recording::access::{self, Access};
    use zapper_lib::recording::api::NetworkSettingsDto;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        // Running, and this home's: it takes this home's token. Something
        // else on the port (another user's service) is not written around.
        let running = match Client::local() {
            Ok(c) if c.version().await.is_ok() => {
                c.status().await.with_context(|| {
                    format!(
                        "a recording service answers on port {}, but not with this home's settings",
                        config::load().port
                    )
                })?;
                Some(c)
            }
            _ => None,
        };
        let dir = zapper_lib::paths::config_dir();
        match what {
            Admin::Account(from) => {
                let text = if from == "-" {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                    s
                } else {
                    std::fs::read_to_string(&from).with_context(|| format!("reading {from}"))?
                };
                // Never the parser's words, which may quote what it read: a
                // password.
                let value: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|_| anyhow::anyhow!("{from} is not an account file (JSON)"))?;
                let account = credentials::parse_portable(value)?;
                credentials::save_portable(&account)?;
                println!(
                    "account: {}, {}; saved in {}",
                    account.kind().label(),
                    account.describe(),
                    credentials::account_path().display()
                );
                if let Some(c) = running {
                    c.sync_now().await?;
                    println!("the service is refreshing the catalogue from it");
                }
            }
            Admin::Password => {
                let pw = read_password()?;
                if let Some(why) = access::password_problem(&pw) {
                    anyhow::bail!(why);
                }
                match running {
                    Some(c) => drop(c.set_password(&pw).await?),
                    None => Access::load(&dir).set_password(&pw).await?,
                }
                println!("password set; any device signed in before must sign in again");
            }
            Admin::Network(on) => match running {
                Some(c) => {
                    let now = c.network().await?;
                    let n = c
                        .set_network(&NetworkSettingsDto {
                            enabled: on,
                            port: now.port,
                            https: now.https,
                        })
                        .await?;
                    println!("network access {}", if on { "on" } else { "off" });
                    if on {
                        for a in &n.addresses {
                            println!("  {a}");
                        }
                    }
                }
                None => {
                    if on && !Access::load(&dir).password_set() {
                        anyhow::bail!("set a sign-in password first (--set-password)");
                    }
                    let path = config::config_path();
                    let mut cfg = config::load_from(&path);
                    cfg.network.enabled = on;
                    config::save_to(&path, &cfg)?;
                    println!(
                        "network access {} from the service's next start ({})",
                        if on { "on" } else { "off" },
                        path.display()
                    );
                }
            },
        }
        Ok(())
    })
}

/// The password from standard input: typed twice without echo at a
/// terminal, else its first line (a pipe, a file, a secret mounted into a
/// container).
fn read_password() -> Result<String> {
    use std::io::{BufRead, IsTerminal};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    let first = prompt_hidden("Password for other devices: ")?;
    let again = prompt_hidden("Again: ")?;
    if first != again {
        anyhow::bail!("the two passwords differ");
    }
    Ok(first)
}

/// One line from the terminal, not shown as it is typed (Unix; elsewhere
/// it is shown, said so first).
fn prompt_hidden(prompt: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    #[cfg(unix)]
    let restore = {
        // SAFETY: tcgetattr and tcsetattr on standard input, with a termios
        // they fill in themselves.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut t) } == 0 {
            let old = t;
            t.c_lflag &= !libc::ECHO;
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
            Some(old)
        } else {
            None
        }
    };
    #[cfg(not(unix))]
    eprint!("(shown as typed) ");
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    #[cfg(unix)]
    if let Some(old) = restore {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &old) };
        eprintln!();
    }
    read?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[derive(Clone, Copy)]
enum Setup {
    Busy,
    Stop,
}

/// `--busy`: exit 0 when no service runs here or it is idle, 3 when it is
/// recording (or about to), printing until when. `--stop-for-setup`: stops
/// it cleanly, now (lifecycle), and waits for it to be gone; with `ask` and a
/// recording in progress, asks first, and exits 2 if the answer is no.
fn for_setup(what: Setup, ask: bool) -> Result<i32> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let Ok(c) = Client::local() else { return Ok(0) };
        let Ok(st) = c.status().await else {
            println!("not running");
            return Ok(0);
        };
        let until = st.idle_at.or(st.active.iter().map(|a| a.until).max());
        let line = match (st.busy(), until) {
            (true, Some(t)) => format!("recording until {}", hhmm(t)),
            (true, None) => "recording".to_string(),
            (false, _) => "idle".to_string(),
        };
        // No line break after it: the NSIS installer puts it in a sentence.
        print!("{line}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        if let Setup::Busy = what {
            return Ok(if st.busy() { 3 } else { 0 });
        }
        if st.busy() && ask && !confirm(&line) {
            return Ok(2);
        }
        c.shutdown(&ShutdownRequest {
            when: ShutdownWhen::Now,
            restart: false,
        })
        .await?;
        // Gone once it no longer answers: it closes its listeners last.
        for _ in 0..240 {
            if c.version().await.is_err() {
                println!("\nstopped");
                return Ok(0);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("the recording service did not stop within two minutes")
    })
}

fn hhmm(t: i64) -> String {
    let c = zapper_lib::localtime::Civil::local(t);
    format!("{:02}:{:02}", c.hour, c.minute)
}

/// Asks whether to stop a recording in progress: a message box on Windows,
/// whose installers ask this way (the MSI cannot ask itself); elsewhere
/// nobody is asked.
fn confirm(line: &str) -> bool {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, IDYES, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNO,
        };
        let text = HSTRING::from(format!(
            "zapper's recording service is {line}.\n\nStop it now and continue? What was recorded so far is kept."
        ));
        let title = HSTRING::from("zapper");
        let answer = unsafe {
            MessageBoxW(None, &text, &title, MB_YESNO | MB_ICONWARNING | MB_SETFOREGROUND)
        };
        answer == IDYES
    }
    #[cfg(not(windows))]
    {
        let _ = line;
        true
    }
}

/// `--install-service` / `--remove-service`: the systemd user unit or the
/// Windows sign-in task, with this executable, as Settings › Recording
/// would set it up.
fn manage_service(install: bool) -> Result<()> {
    use zapper_lib::recording::service;
    if install {
        if std::env::var_os("ZAPPER_SERVER_BIN").is_none() {
            // This one, wherever it is, rather than a search.
            std::env::set_var("ZAPPER_SERVER_BIN", std::env::current_exe()?);
        }
        service::install(false)?;
    } else {
        service::remove()?;
    }
    let s = service::status();
    println!(
        "{}: {}{}",
        s.kind.as_deref().unwrap_or("the recording service"),
        if s.installed { "installed" } else { "not installed" },
        s.state.map(|st| format!(", {st}")).unwrap_or_default()
    );
    Ok(())
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
    zapper_lib::logfile::init("zapper-server", console);

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
    let exit = rt.block_on(async move {
        if args.sync {
            tokio::spawn(sync_engine::periodic(LogSink));
        }
        tokio::spawn(Arc::clone(&server).run_scheduler());
        // On 127.0.0.1, and on the network too when Settings says so; a
        // change there replaces the listeners while this keeps running.
        server.listen().await?;
        let exe = std::env::current_exe().ok();
        let at_start = exe.as_deref().and_then(lifecycle::file_id);
        let exit = supervise(&server, exe.as_deref(), at_start).await;
        // What is recording closes its part and finishes it into a playable
        // file, resumed in a new part by the next start while it is on.
        server.stop_captures(Duration::from_secs(120)).await;
        tracing::info!("stopped");
        Ok::<_, anyhow::Error>((exit, exe))
    });
    let (exit, exe) = exit?;
    rt.shutdown_timeout(Duration::from_secs(5));
    match (exit, exe) {
        (Exit::Restart, Some(exe)) => restart(&exe),
        _ => Ok(()),
    }
}

/// Waits until the service should stop or restart (lifecycle::decide):
/// asked through the API, signalled, or its program replaced on disk.
async fn supervise(server: &Arc<Server>, exe: Option<&Path>, at_start: Option<FileId>) -> Exit {
    let signal = signalled();
    tokio::pin!(signal);
    let mut signalled = false;
    loop {
        let replaced = exe.is_some_and(|p| lifecycle::replaced(at_start, lifecycle::file_id(p)));
        let asked = server.requested_shutdown();
        // Reading the schedule only when it matters.
        let idle = (asked.is_some() || replaced) && server.busy().idle;
        if let Some(e) = lifecycle::decide(asked, signalled, replaced, idle) {
            let why = if signalled {
                "signalled"
            } else if asked.is_some() {
                "asked"
            } else {
                "a newer version was installed"
            };
            tracing::info!("{} ({why})", if e == Exit::Restart { "restarting" } else { "stopping" });
            return e;
        }
        tokio::select! {
            _ = &mut signal => signalled = true,
            _ = server.shutdown_asked.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(20)) => {}
        }
    }
}

/// Runs the program at `exe` again with the same arguments: in this process
/// on Unix, so a systemd unit keeps its process; as a new one on Windows.
fn restart(exe: &Path) -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = std::process::Command::new(exe).args(&args).exec();
        Err(anyhow::anyhow!("restarting {}: {e}", exe.display()))
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new(exe).args(&args).spawn()?;
        Ok(())
    }
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

async fn signalled() {
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
