//! macOS: a LaunchAgent, `~/Library/LaunchAgents/dev.lavoie.zapper.server.plist`,
//! loaded into this user's login session with `launchctl bootstrap` and
//! unloaded with `launchctl bootout`: no administrator. launchd starts it
//! when the user logs in (`RunAtLoad`), and again after a crash, but not
//! after a clean stop (`KeepAlive` → `SuccessfulExit` false), as systemd's
//! `Restart=on-failure` does on Linux. An agent runs while its user is
//! logged in; running with nobody logged in would be a LaunchDaemon, which
//! needs an administrator and runs as root, so it is not offered.
//!
//! zapper-server writes its own log, `zapper-server.log` in the data folder,
//! and is told not to copy it to standard error (`ZAPPER_LOG_STDERR=0`):
//! what reaches standard error and output then (a panic, the reason it
//! would not start) goes to `zapper-server.launchd.log` beside it.
//!
//! The text is built and tested on every platform; `launchctl` only runs on
//! a Mac.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use super::{run, xml_escape};

/// The agent's label: zapper's bundle identifier, and the service's part.
pub const LABEL: &str = "dev.lavoie.zapper.server";

/// `~/Library/LaunchAgents/<label>.plist`.
pub fn plist_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
    )
}

/// Where launchd puts what the service writes to standard error and output.
pub fn launchd_log(data_dir: &Path) -> PathBuf {
    data_dir.join("zapper-server.launchd.log")
}

/// The agent: run `exe` (with `--home` when the app has a home of its own,
/// which an agent does not inherit) at login and after a crash.
///
/// - `KeepAlive` `SuccessfulExit` false: restarted when it exits with an
///   error or a signal; a clean stop (Remove, an installer, `bootout`)
///   stays stopped.
/// - `ThrottleInterval` 10: no faster than every ten seconds, as the
///   systemd unit's `RestartSec`.
/// - `ExitTimeOut` 150: what `bootout` waits after SIGTERM before SIGKILL;
///   launchd's 20 seconds would cut off finishing a recording into a
///   playable file, which can take a couple of minutes.
/// - `ProcessType` Standard: launchd's Background would throttle its disk
///   and CPU, which a recording must not be.
pub fn plist_text(exe: &Path, home: Option<&Path>, log: &Path) -> String {
    let mut args = vec![exe.to_string_lossy().into_owned()];
    if let Some(h) = home {
        args.push("--home".into());
        args.push(h.to_string_lossy().into_owned());
    }
    let args: String = args
        .iter()
        .map(|a| format!("\n        <string>{}</string>", xml_escape(a)))
        .collect();
    let log = xml_escape(&log.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Installed by zapper (Settings > Recording). Remove it there. -->
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>{args}
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>ZAPPER_LOG_STDERR</key>
        <string>0</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ExitTimeOut</key>
    <integer>150</integer>
    <key>ProcessType</key>
    <string>Standard</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
    )
}

/// An executable that will not be there tomorrow: zapper run from its disk
/// image, or from the read-only copy Gatekeeper runs a downloaded app from
/// until it is moved (App Translocation).
pub fn transient(exe: &Path) -> bool {
    let p = exe.to_string_lossy();
    p.starts_with("/Volumes/") || p.contains("/AppTranslocation/")
}

/// The domain the agent goes in: this user's login session (`gui/<uid>`),
/// or, where there is none (logged in over SSH only), the user's
/// background one (`user/<uid>`), which runs it all the same.
fn domain() -> Result<String> {
    let uid = run("id", &["-u"])?;
    let gui = format!("gui/{uid}");
    if run("launchctl", &["print", &gui]).is_ok() {
        Ok(gui)
    } else {
        Ok(format!("user/{uid}"))
    }
}

/// What `launchctl print` says of the agent, in the words Settings shows:
/// `running`, `stopped`, or `not loaded` when the plist is there but
/// launchd does not have it.
pub fn state() -> Option<String> {
    let path = plist_path()?;
    if !path.is_file() {
        return None;
    }
    let target = domain().ok().map(|d| format!("{d}/{LABEL}"))?;
    let Ok(out) = run("launchctl", &["print", &target]) else {
        return Some("not loaded".into());
    };
    Some(state_from_print(&out).to_string())
}

/// `state = running` (or `not running`, `spawn scheduled`, ...) in
/// `launchctl print`'s output.
pub fn state_from_print(out: &str) -> &'static str {
    let state = out
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("state = "))
        .unwrap_or("");
    match state {
        "running" => "running",
        "spawn scheduled" | "spawning" => "starting",
        _ => "stopped",
    }
}

/// Writes the plist and loads it, which starts it; over an agent already
/// loaded, that one is unloaded first (stopping it cleanly), so that a new
/// version runs.
pub fn install(exe: &Path, home: Option<&Path>) -> Result<()> {
    if transient(exe) {
        bail!(
            "zapper is running from {}, which will not stay there: move zapper to Applications, open it from there, and install the service again",
            exe.display()
        );
    }
    let path = plist_path().context("no home folder to install the service in")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let data = crate::paths::data_dir();
    std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
    let domain = domain()?;
    let target = format!("{domain}/{LABEL}");
    let _ = run("launchctl", &["bootout", &target]);
    std::fs::write(&path, plist_text(exe, home, &launchd_log(&data)))
        .with_context(|| format!("writing {}", path.display()))?;
    let plist = path.to_string_lossy();
    // An agent just booted out can take a moment to be gone, and launchd
    // refuses to load it again until it is (an I/O error).
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match run("launchctl", &["bootstrap", &domain, &plist]) {
            Ok(_) => return Ok(()),
            Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => return Err(e),
        }
    }
}

/// Unloads the agent, which stops it cleanly, and deletes its plist.
pub fn remove() -> Result<()> {
    let Some(path) = plist_path() else {
        return Ok(());
    };
    if let Ok(domain) = domain() {
        // Not loaded is not a reason to keep the file.
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
    }
    if path.is_file() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_runs_the_binary_at_login_and_after_a_crash_only() {
        let p = plist_text(
            Path::new("/Applications/zapper.app/Contents/MacOS/zapper-server"),
            None,
            Path::new("/Users/u/Library/Application Support/zapper/zapper-server.launchd.log"),
        );
        assert!(p.contains("<key>Label</key>\n    <string>dev.lavoie.zapper.server</string>"));
        assert!(p.contains(
            "<array>\n        <string>/Applications/zapper.app/Contents/MacOS/zapper-server</string>\n    </array>"
        ));
        assert!(p.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(p.contains("<key>SuccessfulExit</key>\n        <false/>"));
        assert!(p.contains("<key>ExitTimeOut</key>\n    <integer>150</integer>"));
        assert!(p.contains(
            "<key>StandardErrorPath</key>\n    <string>/Users/u/Library/Application Support/zapper/zapper-server.launchd.log</string>"
        ));
        assert!(p.contains("<key>ZAPPER_LOG_STDERR</key>\n        <string>0</string>"));
    }

    #[test]
    fn a_home_of_its_own_is_handed_to_the_agent_and_escaped() {
        let p = plist_text(
            Path::new("/opt/z&z/zapper-server"),
            Some(Path::new("/tmp/a <b>")),
            Path::new("/tmp/log"),
        );
        assert!(p.contains(
            "<string>/opt/z&amp;z/zapper-server</string>\n        <string>--home</string>\n        <string>/tmp/a &lt;b&gt;</string>"
        ), "{p}");
    }

    #[test]
    fn launchctl_print_is_read_for_the_state() {
        let out = "gui/501/dev.lavoie.zapper.server = {\n\tactive count = 1\n\tpath = /x.plist\n\ttype = LaunchAgent\n\tstate = running\n\n\tprogram = /x\n}";
        assert_eq!(state_from_print(out), "running");
        assert_eq!(state_from_print("\tstate = not running\n"), "stopped");
        assert_eq!(state_from_print("\tstate = spawn scheduled\n"), "starting");
        assert_eq!(state_from_print(""), "stopped");
    }

    #[test]
    fn an_app_on_its_disk_image_or_translocated_is_not_installed_from() {
        assert!(transient(Path::new("/Volumes/zapper/zapper.app/Contents/MacOS/zapper-server")));
        assert!(transient(Path::new(
            "/private/var/folders/x/T/AppTranslocation/5F2E/d/zapper.app/Contents/MacOS/zapper-server"
        )));
        assert!(!transient(Path::new("/Applications/zapper.app/Contents/MacOS/zapper-server")));
    }
}
