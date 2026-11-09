//! Installs the recording service the way Settings › Recording does, checks
//! that it runs and answers, then removes it and checks that it is gone.
//!
//! ```text
//! cargo build -p zapper-server --features harness --bin zapper-server --bin service_check
//! target/debug/service_check
//! ```
//!
//! On a scratch `ZAPPER_HOME` (never the account on this machine), which the
//! installed service is handed as well, on a free port. It installs a real
//! service for this user for the few seconds the check takes: on Windows the
//! sign-in task, on macOS the LaunchAgent, on Linux the systemd user unit,
//! under their usual names, so it refuses to run while one is already
//! installed rather than replace it.
//! The `zapper-server` it installs is the one beside this executable.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use zapper_core::recording::client::Client;
use zapper_core::recording::{config, service};

fn main() {
    if std::env::var_os("ZAPPER_HOME").is_none_or(|v| v.is_empty()) {
        let dir = std::env::temp_dir().join(format!("zapper-service-check-{}", std::process::id()));
        std::env::set_var("ZAPPER_HOME", &dir);
    }
    std::env::remove_var(zapper_core::harness::ALLOW_REAL_ACCOUNT);
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    match rt.block_on(check()) {
        Ok(()) => println!("service_check: PASS"),
        Err(e) => {
            eprintln!("service_check: FAIL: {e}");
            std::process::exit(1);
        }
    }
}

async fn check() -> Result<(), String> {
    let home = PathBuf::from(std::env::var_os("ZAPPER_HOME").unwrap());
    std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
    println!("scratch home: {}", home.display());

    let exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(format!("zapper-server{}", std::env::consts::EXE_SUFFIX));
    if !exe.is_file() {
        return Err(format!("{} is missing; build it with --bin zapper-server", exe.display()));
    }
    std::env::set_var("ZAPPER_SERVER_BIN", &exe);

    let before = service::status();
    if !before.supported {
        return Err(before.unsupported_reason.unwrap_or_default());
    }
    if before.installed {
        return Err("a recording service is already installed for this user; not replacing it".into());
    }

    // A port of its own, written where the service and the client below
    // both read it.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map_err(|e| e.to_string())?
        .port();
    let cfg = config::ServerConfig {
        port,
        ..Default::default()
    };
    config::save_to(&config::config_path(), &cfg).map_err(|e| format!("{e:#}"))?;

    println!("installing {} ({})", exe.display(), before.kind.unwrap_or_default());
    service::install(false).map_err(|e| format!("install: {e:#}"))?;
    let outcome = installed(port).await;
    let removed = service::remove().map_err(|e| format!("remove: {e:#}"));
    outcome?;
    removed?;

    let after = service::status();
    if after.installed {
        return Err(format!("still installed after Remove ({:?})", after.state));
    }
    let gone = wait(Duration::from_secs(15), || async {
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
    })
    .await;
    if !gone {
        return Err(format!("something still answers on port {port} after Remove"));
    }
    println!("removed; nothing answers on {port}");
    Ok(())
}

/// What must hold while it is installed: the status says so, and the
/// service answers on the scratch home's port with the scratch home's
/// token, which it can only have if it was handed that home.
async fn installed(port: u16) -> Result<(), String> {
    let s = service::status();
    println!("status: installed {} state {:?}", s.installed, s.state);
    if !s.installed {
        return Err("Install returned, but the status says it is not installed".into());
    }
    let answered = wait(Duration::from_secs(30), || async {
        let Ok(c) = Client::local() else { return false };
        c.version().await.is_ok()
    })
    .await;
    if !answered {
        return Err(format!("the service did not answer on port {port}"));
    }
    let token = config::read_token().ok_or("no token in the scratch home")?;
    let c = Client::new(&format!("http://127.0.0.1:{port}"), Some(token)).map_err(|e| e.to_string())?;
    c.settings()
        .await
        .map_err(|e| format!("the service did not take the scratch home's token: {e}"))?;
    let v = c.version().await.map_err(|e| e.to_string())?;
    let s = service::status();
    println!("answering: version {} on {port}; state {:?}", v.version, s.state);
    Ok(())
}

async fn wait<F, Fut>(limit: Duration, mut done: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if done().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}
