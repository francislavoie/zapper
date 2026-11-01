//! Running the recording service on this computer: Settings › Recording's
//! Install and Remove.
//!
//! Linux: a systemd *user* unit, `~/.config/systemd/user/zapper-server.service`,
//! enabled and started with `systemctl --user` — no root. A user's services
//! stop when they log out unless lingering is on, which `loginctl
//! enable-linger` turns on for one's own account (polkit allows it without a
//! password on the common desktops); that is the "keep recording when logged
//! out" switch.
//!
//! Windows (a Scheduled Task at logon) and macOS (a LaunchAgent) are not
//! done yet; they report so rather than pretending.
//!
//! Nothing here runs unless a person clicks: installing a service is theirs
//! to decide.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const UNIT_NAME: &str = "zapper-server.service";

/// What Settings › Recording shows about the service on this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatusDto {
    /// Install and Remove work on this platform.
    pub supported: bool,
    /// Said instead when they do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
    pub installed: bool,
    /// systemd's word for it: `active`, `inactive`, `failed`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Keeps running after logging out.
    pub linger: bool,
    /// The `zapper-server` that Install would run, if one was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// The service answered on its API, and with which version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Its address, for the curious and for phase two's web client.
    pub address: String,
    /// The service is reachable but is not this app's version.
    pub version_mismatch: bool,
}

/// The unit: run the service, restart it if it crashes, start it with the
/// user's session (or at boot, when lingering).
///
/// `Restart=on-failure` brings back a crashed service, which then resumes
/// whatever it was recording; a clean stop (Remove, or `systemctl stop`)
/// stays stopped.
pub fn unit_text(exe: &Path) -> String {
    format!(
        "# Installed by zapper (Settings > Recording). Remove it there.\n\
         [Unit]\n\
         Description=zapper recording service\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={}\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        systemd_quote(&exe.to_string_lossy())
    )
}

/// A path as one systemd command-line word: quoted when it has a space,
/// with `\`, `"` and the specifier `%` escaped.
fn systemd_quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    if escaped.contains(char::is_whitespace) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

/// `$XDG_CONFIG_HOME/systemd/user/zapper-server.service`.
pub fn unit_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("systemd").join("user").join(UNIT_NAME))
}

/// The `zapper-server` to install: `ZAPPER_SERVER_BIN`, else the one beside
/// this executable (how the packages install them), else one on `PATH`.
pub fn find_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZAPPER_SERVER_BIN").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let name = if cfg!(windows) {
        "zapper-server.exe"
    } else {
        "zapper-server"
    };
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    if beside.is_file() {
        return Some(beside);
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running {cmd}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!(
            "{cmd} {}: {}",
            args.join(" "),
            if err.is_empty() {
                out.status.to_string()
            } else {
                err
            }
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn unsupported() -> Option<String> {
    if cfg!(target_os = "linux") {
        None
    } else if cfg!(windows) {
        Some(
            "Not yet on Windows: run zapper-server.exe yourself for now (a logon task is planned)."
                .into(),
        )
    } else {
        Some(
            "Not yet on macOS: run zapper-server yourself for now (a LaunchAgent is planned)."
                .into(),
        )
    }
}

/// What is installed and running, without the API's half (see the caller).
pub fn status() -> ServiceStatusDto {
    let cfg = super::config::load();
    let mut s = ServiceStatusDto {
        supported: unsupported().is_none(),
        unsupported_reason: unsupported(),
        binary: find_binary().map(|p| p.to_string_lossy().into_owned()),
        address: format!("http://127.0.0.1:{}", cfg.port),
        ..Default::default()
    };
    if !s.supported {
        return s;
    }
    s.installed = unit_path().is_some_and(|p| p.is_file());
    if s.installed {
        // `is-active` exits non-zero for anything but active, and still
        // prints the state, which is what is wanted.
        s.state = Command::new("systemctl")
            .args(["--user", "is-active", UNIT_NAME])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|t| !t.is_empty());
    }
    s.linger = linger();
    s
}

fn linger() -> bool {
    let Ok(user) = std::env::var("USER") else {
        return false;
    };
    run("loginctl", &["show-user", &user, "--property=Linger"])
        .is_ok_and(|o| o.trim() == "Linger=yes")
}

/// Writes the unit and starts it; with `linger`, keeps it running after
/// logging out.
pub fn install(linger: bool) -> Result<()> {
    if let Some(why) = unsupported() {
        bail!(why);
    }
    let exe = find_binary().ok_or_else(|| {
        anyhow!("zapper-server was not found beside zapper or on PATH; build it with `cargo build --no-default-features --features server --bin zapper-server`")
    })?;
    let path = unit_path().ok_or_else(|| anyhow!("no home directory to install the service in"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&path, unit_text(&exe))
        .with_context(|| format!("writing {}", path.display()))?;
    run("systemctl", &["--user", "daemon-reload"])?;
    // `restart` rather than `start`, so reinstalling over a running service
    // (a new version) runs the new one.
    run("systemctl", &["--user", "enable", UNIT_NAME])?;
    run("systemctl", &["--user", "restart", UNIT_NAME])?;
    set_linger(linger)
}

/// Turns lingering on or off for this user.
pub fn set_linger(on: bool) -> Result<()> {
    if unsupported().is_some() {
        return Ok(());
    }
    if on == self::linger() {
        return Ok(());
    }
    let user = std::env::var("USER").context("no USER in the environment")?;
    run(
        "loginctl",
        &[
            if on {
                "enable-linger"
            } else {
                "disable-linger"
            },
            &user,
        ],
    )
    .map(|_| ())
}

/// Stops and removes the unit. Recordings and settings stay where they are.
pub fn remove() -> Result<()> {
    if let Some(why) = unsupported() {
        bail!(why);
    }
    let Some(path) = unit_path() else {
        return Ok(());
    };
    if path.is_file() {
        // Stopping a unit that is not running is not a reason to keep it.
        let _ = run("systemctl", &["--user", "disable", "--now", UNIT_NAME]);
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        run("systemctl", &["--user", "daemon-reload"])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_the_binary_and_restarts_it_on_failure_only() {
        let u = unit_text(Path::new("/usr/bin/zapper-server"));
        assert!(u.contains("\nExecStart=/usr/bin/zapper-server\n"));
        assert!(u.contains("\nRestart=on-failure\n"));
        assert!(u.contains("\nWantedBy=default.target\n"));
    }

    #[test]
    fn awkward_paths_stay_one_word() {
        assert_eq!(
            systemd_quote("/opt/my apps/zapper-server"),
            "\"/opt/my apps/zapper-server\""
        );
        assert_eq!(systemd_quote("/opt/100%/z"), "/opt/100%%/z");
    }
}
