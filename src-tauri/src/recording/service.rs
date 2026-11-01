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
//! Windows: a Scheduled Task, `zapper-server-<user>`, that starts
//! `zapper-server.exe` when this user signs in, as this user, with no
//! elevation and no stored password (Task Scheduler's *interactive token*),
//! registered, started, stopped and deleted through Task Scheduler's COM
//! interface. Unprivileged users may do all of that for their own account.
//! The task runs while its user is signed in and no longer: running with
//! nobody signed in needs the account's password stored with the task, or
//! the right to log on as a batch job, so there is no "keep running when
//! logged out" switch there. Nor does it wake the computer to record: that
//! would be one more task per recording, with a wake timer.
//!
//! macOS (a LaunchAgent) is not done yet; it reports so rather than
//! pretending.
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
    /// What Install sets up, in words: "a systemd user service".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub installed: bool,
    /// systemd's word for it (`active`, `inactive`, `failed`, ...), or Task
    /// Scheduler's in the same spirit (`running`, `stopped`, `disabled`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Keeps running after logging out.
    pub linger: bool,
    /// Whether keeping it running after logging out is offered here.
    pub can_linger: bool,
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
///
/// `ConditionPathExists`: the unit lives in the user's home, which removing
/// the package that carried `zapper-server` never touches (a root maintainer
/// script has no business in people's homes). Left behind, it is skipped
/// at each start rather than failing and being retried.
///
/// `home`, as for the Windows task ([`task_xml`]): the app's own
/// `ZAPPER_HOME`, handed on as `--home`.
pub fn unit_text(exe: &Path, home: Option<&Path>) -> String {
    let path = exe.to_string_lossy();
    let mut exec = systemd_quote(&path);
    if let Some(h) = home {
        exec = format!("{exec} --home {}", systemd_quote(&h.to_string_lossy()));
    }
    format!(
        "# Installed by zapper (Settings > Recording). Remove it there.\n\
         [Unit]\n\
         Description=zapper recording service\n\
         ConditionPathExists={}\n\
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
        path.replace('%', "%%"),
        exec
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
    if cfg!(any(target_os = "linux", windows)) {
        None
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
    if cfg!(windows) {
        s.kind = Some("a scheduled task that starts it when you sign in".into());
        if let Some(user) = windows_user() {
            match scheduler::state(&task_name(&user)) {
                Ok(state) => {
                    s.installed = state.is_some();
                    s.state = state;
                }
                Err(e) => tracing::warn!("recording service: Task Scheduler: {e:#}"),
            }
        }
        return s;
    }
    s.kind = Some("a systemd user service".into());
    s.can_linger = true;
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
/// logging out. On Windows, registers the task and starts it; `linger` is
/// not offered there and is ignored.
pub fn install(linger: bool) -> Result<()> {
    if let Some(why) = unsupported() {
        bail!(why);
    }
    let exe = find_binary().ok_or_else(|| {
        anyhow!("zapper-server was not found beside zapper or on PATH; build it with `cargo build --no-default-features --features server --bin zapper-server`")
    })?;
    // A home of the app's own (ZAPPER_HOME) goes to the service too.
    let home = std::env::var_os(crate::paths::HOME_OVERRIDE)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    if cfg!(windows) {
        let user = windows_user().context("no USERNAME in the environment")?;
        return scheduler::install(&task_name(&user), |account| {
            task_xml(&exe, account, home.as_deref())
        });
    }
    let path = unit_path().ok_or_else(|| anyhow!("no home directory to install the service in"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&path, unit_text(&exe, home.as_deref()))
        .with_context(|| format!("writing {}", path.display()))?;
    run("systemctl", &["--user", "daemon-reload"])?;
    // `restart` rather than `start`, so reinstalling over a running service
    // (a new version) runs the new one.
    run("systemctl", &["--user", "enable", UNIT_NAME])?;
    run("systemctl", &["--user", "restart", UNIT_NAME])?;
    set_linger(linger)
}

/// Turns lingering on or off for this user. Nothing to do where it is not
/// offered.
pub fn set_linger(on: bool) -> Result<()> {
    if unsupported().is_some() || cfg!(windows) {
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

/// Stops and removes the unit, or the task. Recordings and settings stay
/// where they are.
pub fn remove() -> Result<()> {
    if let Some(why) = unsupported() {
        bail!(why);
    }
    if cfg!(windows) {
        let Some(user) = windows_user() else {
            return Ok(());
        };
        return scheduler::remove(&task_name(&user));
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

/// Stops the service so that an installer can replace its program: on
/// Windows a running `zapper-server.exe` cannot be. It starts again at the
/// next sign-in, or when zapper next opens ([`resume`]). Elsewhere there is
/// nothing to do: a running program's file can be replaced, and the service
/// runs the old one until it is installed again.
pub fn stop_for_update() {
    if !cfg!(windows) {
        return;
    }
    if let Some(user) = windows_user() {
        if let Err(e) = scheduler::stop_task(&task_name(&user)) {
            tracing::warn!("recording service: stopping it for the update: {e:#}");
        }
    }
}

/// Starts the installed service when it is not running, as zapper opens:
/// after an update stopped it ([`stop_for_update`]), it would otherwise
/// miss what it was to record until the next sign-in. Windows only; systemd
/// keeps the unit running by itself.
pub fn resume() {
    if !cfg!(windows) {
        return;
    }
    if let Some(user) = windows_user() {
        match scheduler::run_if_stopped(&task_name(&user)) {
            Ok(true) => tracing::info!("recording service: started its task"),
            Ok(false) => {}
            Err(e) => tracing::warn!("recording service: starting its task: {e:#}"),
        }
    }
}

/// This user's name, for naming the task. The task's own account comes
/// from Task Scheduler (`scheduler::install`): `USERDOMAIN` is not always
/// the account's domain (an SSH login has `WORKGROUP` there).
fn windows_user() -> Option<String> {
    std::env::var("USERNAME").ok().filter(|u| !u.is_empty())
}

/// The task's name. One per user: every account on a computer shares one
/// list of tasks, and may not touch another's.
pub fn task_name(user: &str) -> String {
    let name = user.rsplit('\\').next().unwrap_or(user);
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("zapper-server-{name}")
}

/// The task: at `user`'s sign-in, run `exe` as that user, unelevated, with
/// no stored password, for as long as they stay signed in.
///
/// - `ExecutionTimeLimit` `PT0S`: Task Scheduler stops a task after three
///   days unless told otherwise, and this one is meant to run for weeks.
/// - Batteries do not stop it or keep it from starting: a laptop records
///   on battery too.
/// - `MultipleInstancesPolicy` `IgnoreNew`: a second start while it runs
///   does nothing, rather than a second service fighting for the port.
/// - `RestartOnFailure`: a start that fails is tried again, three times a
///   minute apart, as systemd's `Restart=on-failure` would.
/// - `Priority` 5, normal: Task Scheduler's default (7) runs the process,
///   and its disk reads and writes, below normal, which a recording should
///   not be.
///
/// `home`, when this app runs on a home other than the account's
/// (`ZAPPER_HOME`), is handed on (`--home`): a task does not inherit the
/// environment of whoever registered it.
pub fn task_xml(exe: &Path, user: &str, home: Option<&Path>) -> String {
    // By hand rather than Path::parent, so the tests read a Windows path the
    // same on every platform.
    let exe = exe.to_string_lossy();
    let dir = exe.rfind(['\\', '/']).map_or("", |i| &exe[..i]);
    let args = home
        .map(|h| format!("--home {}", windows_arg(&h.to_string_lossy())))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>zapper</Author>
    <Description>zapper's recording service, installed from Settings &gt; Recording. Remove it there.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{args}</Arguments>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        user = xml_escape(user),
        command = xml_escape(&exe),
        args = xml_escape(&args),
        dir = xml_escape(dir),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// One word of a Windows command line, as the C runtime splits them
/// (`CommandLineToArgvW`): quoted when it has a space or a quote, with
/// backslashes doubled only where they come before a quote.
fn windows_arg(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        return s.to_owned();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in s.chars() {
        if c == '\\' {
            backslashes += 1;
            continue;
        }
        let n = if c == '"' { backslashes * 2 + 1 } else { backslashes };
        out.push_str(&"\\".repeat(n));
        backslashes = 0;
        out.push(c);
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Task Scheduler, through its COM interface (the `windows` crate). Each
/// call opens its own connection on the calling thread, which the callers
/// make a blocking one.
#[cfg(windows)]
mod scheduler {
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result};
    use windows::core::BSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::TaskScheduler::{
        IRegisteredTask, ITaskFolder, ITaskService, TaskScheduler, TASK_CREATE_OR_UPDATE,
        TASK_LOGON_INTERACTIVE_TOKEN, TASK_STATE_DISABLED, TASK_STATE_QUEUED, TASK_STATE_READY,
        TASK_STATE_RUNNING,
    };
    use windows::Win32::System::Variant::VARIANT;

    /// COM on this thread for as long as it lives. Declared before any COM
    /// object, so it is dropped after them.
    struct Com(bool);

    impl Com {
        fn init() -> Self {
            // S_FALSE (already initialised) counts as success and is
            // balanced like it; a thread already in another mode can still
            // make these calls, but must not uninitialise.
            Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    /// Task Scheduler, connected as this user, and its root folder.
    fn connect() -> Result<(ITaskService, ITaskFolder)> {
        let empty = VARIANT::default();
        unsafe {
            let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
                .context("opening Task Scheduler")?;
            service
                .Connect(&empty, &empty, &empty, &empty)
                .context("connecting to Task Scheduler")?;
            let folder = service.GetFolder(&BSTR::from("\\"))?;
            Ok((service, folder))
        }
    }

    fn root() -> Result<ITaskFolder> {
        Ok(connect()?.1)
    }

    fn find(folder: &ITaskFolder, name: &str) -> Option<IRegisteredTask> {
        unsafe { folder.GetTask(&BSTR::from(name)) }.ok()
    }

    fn running(task: &IRegisteredTask) -> bool {
        unsafe { task.State() }.is_ok_and(|s| s == TASK_STATE_RUNNING)
    }

    /// Stops it, and waits (a few seconds at most) until it has stopped.
    fn stop(task: &IRegisteredTask) {
        if !running(task) {
            return;
        }
        if let Err(e) = unsafe { task.Stop(0) } {
            tracing::warn!("recording service: stopping the task: {e}");
        }
        let until = Instant::now() + Duration::from_secs(10);
        while running(task) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The task's state in a word, or None when there is no such task.
    pub fn state(name: &str) -> Result<Option<String>> {
        let _com = Com::init();
        let Some(task) = find(&root()?, name) else {
            return Ok(None);
        };
        let state = unsafe { task.State() }.context("reading the task's state")?;
        Ok(Some(
            match state {
                TASK_STATE_RUNNING => "running",
                TASK_STATE_READY => "stopped",
                TASK_STATE_QUEUED => "starting",
                TASK_STATE_DISABLED => "disabled",
                _ => "unknown",
            }
            .to_owned(),
        ))
    }

    /// Registers the task `xml` makes for this account, `DOMAIN\user` as
    /// Task Scheduler knows it (replacing a task of the same name), and
    /// starts it now, stopping the one that ran so that a new version takes
    /// over.
    pub fn install(name: &str, xml: impl FnOnce(&str) -> String) -> Result<()> {
        let _com = Com::init();
        let (service, folder) = connect()?;
        let account = unsafe {
            let domain = service.ConnectedDomain().context("reading this account's domain")?;
            let user = service.ConnectedUser().context("reading this account's name")?;
            format!("{domain}\\{user}")
        };
        let xml = xml(&account);
        if let Some(old) = find(&folder, name) {
            stop(&old);
        }
        let empty = VARIANT::default();
        let task = unsafe {
            folder.RegisterTask(
                &BSTR::from(name),
                &BSTR::from(xml.as_str()),
                TASK_CREATE_OR_UPDATE.0,
                &empty,
                &empty,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &empty,
            )
        }
        .with_context(|| format!("Task Scheduler would not register the task {name}"))?;
        unsafe { task.Run(&empty) }.with_context(|| format!("starting the task {name}"))?;
        Ok(())
    }

    /// Stops the task, if there is one and it runs.
    pub fn stop_task(name: &str) -> Result<()> {
        let _com = Com::init();
        if let Some(task) = find(&root()?, name) {
            stop(&task);
        }
        Ok(())
    }

    /// Starts the task when it exists, is enabled and is not running.
    /// Whether it did.
    pub fn run_if_stopped(name: &str) -> Result<bool> {
        let _com = Com::init();
        let Some(task) = find(&root()?, name) else {
            return Ok(false);
        };
        if unsafe { task.State() }? != TASK_STATE_READY {
            return Ok(false);
        }
        unsafe { task.Run(&VARIANT::default()) }.with_context(|| format!("starting the task {name}"))?;
        Ok(true)
    }

    /// Stops the task and deletes it; nothing to do when there is none.
    pub fn remove(name: &str) -> Result<()> {
        let _com = Com::init();
        let folder = root()?;
        let Some(task) = find(&folder, name) else {
            return Ok(());
        };
        stop(&task);
        unsafe { folder.DeleteTask(&BSTR::from(name), 0) }
            .with_context(|| format!("deleting the task {name}"))?;
        Ok(())
    }
}

/// Elsewhere, never called: [`unsupported`] and the platform checks above
/// come first.
#[cfg(not(windows))]
mod scheduler {
    use anyhow::{bail, Result};

    pub fn state(_: &str) -> Result<Option<String>> {
        Ok(None)
    }

    pub fn install(_: &str, _: impl FnOnce(&str) -> String) -> Result<()> {
        bail!("Task Scheduler is Windows'")
    }

    pub fn remove(_: &str) -> Result<()> {
        Ok(())
    }

    pub fn stop_task(_: &str) -> Result<()> {
        Ok(())
    }

    pub fn run_if_stopped(_: &str) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_the_binary_and_restarts_it_on_failure_only() {
        let u = unit_text(Path::new("/usr/bin/zapper-server"), None);
        assert!(u.contains("\nExecStart=/usr/bin/zapper-server\n"));
        let u = unit_text(Path::new("/usr/bin/zapper-server"), Some(Path::new("/tmp/a b")));
        assert!(u.contains("\nExecStart=/usr/bin/zapper-server --home \"/tmp/a b\"\n"));
        assert!(u.contains("\nRestart=on-failure\n"));
        assert!(u.contains("\nWantedBy=default.target\n"));
        // Once the package that carried it is removed, the unit is skipped
        // rather than failed and retried.
        assert!(u.contains("\nConditionPathExists=/usr/bin/zapper-server\n"));
        let u = unit_text(Path::new("/opt/100% z/zapper-server"), None);
        assert!(u.contains("\nConditionPathExists=/opt/100%% z/zapper-server\n"), "{u}");
    }

    #[test]
    fn the_task_runs_the_service_at_sign_in_as_this_user_for_good() {
        let x = task_xml(
            Path::new(r"C:\Program Files\zapper\zapper-server.exe"),
            r"DESKTOP\Ann & Bo",
            None,
        );
        assert!(x.starts_with("<?xml version=\"1.0\" encoding=\"UTF-16\"?>"));
        assert!(x.contains("<LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>DESKTOP\\Ann &amp; Bo</UserId>"));
        assert!(x.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(x.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(x.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(x.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(x.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(x.contains("<WakeToRun>false</WakeToRun>"));
        assert!(x.contains("<Command>C:\\Program Files\\zapper\\zapper-server.exe</Command>"));
        assert!(x.contains("<Arguments></Arguments>"));
        assert!(x.contains("<WorkingDirectory>C:\\Program Files\\zapper</WorkingDirectory>"));
        // No password anywhere: the interactive token needs none.
        assert!(!x.contains("Password"));
    }

    #[test]
    fn a_home_of_its_own_is_handed_to_the_task() {
        let x = task_xml(
            Path::new(r"C:\z\zapper-server.exe"),
            "u",
            Some(Path::new(r"C:\Users\u\scratch home\")),
        );
        assert!(
            x.contains(r#"<Arguments>--home &quot;C:\Users\u\scratch home\\&quot;</Arguments>"#),
            "{x}"
        );
    }

    #[test]
    fn windows_arguments_survive_the_c_runtime() {
        assert_eq!(windows_arg(r"C:\plain\path"), r"C:\plain\path");
        assert_eq!(windows_arg(r"C:\with space"), r#""C:\with space""#);
        // A trailing backslash would otherwise escape the closing quote.
        assert_eq!(windows_arg(r"C:\with space\"), r#""C:\with space\\""#);
        assert_eq!(windows_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(windows_arg(""), r#""""#);
    }

    #[test]
    fn each_user_has_a_task_of_their_own() {
        assert_eq!(task_name(r"DESKTOP\ann"), "zapper-server-ann");
        assert_eq!(task_name("bo"), "zapper-server-bo");
        assert_eq!(task_name(r"D\a:b*c"), "zapper-server-a_b_c");
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
