//! Windows: the recording service as a real Windows service, started at
//! boot whether or not anyone signs in — Settings › Recording's "Start at
//! boot", the other way to run it there besides the sign-in task.
//!
//! **Who it runs as.** Its own virtual account, `NT SERVICE\<name>`, which
//! Windows makes for the service and which has no password, no profile of
//! a person's and no rights beyond what it is granted. Not LocalSystem: the
//! service answers on the network when network access is on, and a flaw in
//! it would then be a flaw running as the most powerful account on the
//! computer. Not the person's own account either: a service runs as a
//! person only with their password stored in the Service Control Manager,
//! and a changed password would silently stop it.
//!
//! **Whose files.** The person's zapper, not one of its own: their settings,
//! API token and provider account (`%APPDATA%\zapper`), catalogue and list
//! of recordings (`%LOCALAPPDATA%\zapper`), and recordings folder. Its own
//! `%APPDATA%` is the virtual account's, so they are handed to it on its
//! command line ([`Plan::service_args`]: `--config-dir`, `--data-dir`,
//! `--cache-dir`, `--recordings-dir`), and it is granted Modify on those
//! folders and on the recordings folder in its settings, and Read & Execute
//! on the folder `zapper-server.exe` is in when that is in the person's
//! profile (the per-user setup's). Granted with `icacls` on the folders,
//! inherited by what is in them; the person keeps their own access, and the
//! files the service writes there inherit it too, so the app reads the token
//! and the catalogue the service writes, as it does today. The app-token and
//! loopback rules are unchanged: the token is still the one file in the
//! person's config folder, and only a loopback caller holding it is "the
//! app". A recordings folder chosen later in Settings is granted then, by
//! the app (the person owns their folders, so that needs no administrator).
//! A network share is out of reach of a virtual account, as of any service;
//! that is said in Settings.
//!
//! **Who may manage it.** Creating a service needs an administrator: the one
//! UAC prompt, for `zapper-server --install-boot-service`, run elevated with
//! everything decided beforehand on its command line ([`Plan`]) rather than
//! in a file a non-elevated process could change in between. It gives the
//! person who installed it (their SID) the right to start, stop, query and
//! delete it in its security descriptor ([`sddl`]), but not to reconfigure
//! it (which would let them run anything as it), so that the app, the
//! updater and the uninstaller stop, start and remove it as that person,
//! with no further prompt.
//!
//! **One or the other.** It and the sign-in task have the same name,
//! `zapper-server-<user>`, in their separate namespaces, and never both
//! run: switching stops the one cleanly before the other starts.
//!
//! **How it behaves.** Automatic start; restarted by the Service Control
//! Manager ten seconds after a crash, up to three times a day, and after an
//! exit with an error (`FailureActionsOnNonCrashFailures`), which is how it
//! restarts itself when asked to; a clean stop stays stopped. It asks for
//! the pre-shutdown notice, with a minute to finish what it is recording into
//! playable files as the computer shuts down. Its log is
//! `zapper-server.log` in the person's data folder, like the task's.
//!
//! The plan, the command lines and the security descriptor are built and
//! tested on every platform; the Service Control Manager is Windows'
//! ([`scm`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

/// The argument that starts the elevated step.
pub const INSTALL_ARG: &str = "--install-boot-service";

/// What installing the boot-time service sets up, all decided by the app as
/// its person, then carried out elevated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The service's name, `zapper-server-<user>`.
    pub name: String,
    /// The SID of the person it is for, who may start, stop and remove it.
    pub user_sid: String,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// The default recordings folder.
    pub recordings_dir: PathBuf,
    /// Folders it is granted Modify on: the four above, and the recordings
    /// folder in its settings when that is another.
    pub write: Vec<PathBuf>,
    /// Folders it is granted Read & Execute on: the program's own, when it
    /// is in the person's profile.
    pub read: Vec<PathBuf>,
}

impl Plan {
    /// The elevated step's command line, after the program's name.
    pub fn to_args(&self) -> Vec<OsString> {
        let mut a: Vec<OsString> = vec![
            INSTALL_ARG.into(),
            "--name".into(),
            self.name.clone().into(),
            "--sid".into(),
            self.user_sid.clone().into(),
            "--config-dir".into(),
            self.config_dir.clone().into(),
            "--data-dir".into(),
            self.data_dir.clone().into(),
            "--cache-dir".into(),
            self.cache_dir.clone().into(),
            "--recordings-dir".into(),
            self.recordings_dir.clone().into(),
        ];
        for w in &self.write {
            a.push("--grant-write".into());
            a.push(w.clone().into());
        }
        for r in &self.read {
            a.push("--grant-read".into());
            a.push(r.clone().into());
        }
        a
    }

    /// The plan in the elevated step's command line, from after
    /// [`INSTALL_ARG`]. Also takes `--report FILE`, returned beside it.
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<(Plan, Option<PathBuf>)> {
        let mut it = args.into_iter();
        let mut name = None;
        let mut sid = None;
        let mut dirs: [Option<PathBuf>; 4] = Default::default();
        let mut write = Vec::new();
        let mut read = Vec::new();
        let mut report = None;
        while let Some(arg) = it.next() {
            let arg = arg.to_string_lossy().into_owned();
            let mut value = || it.next().ok_or_else(|| anyhow!("{arg} needs a value"));
            match arg.as_str() {
                "--name" => name = Some(value()?.to_string_lossy().into_owned()),
                "--sid" => sid = Some(value()?.to_string_lossy().into_owned()),
                "--config-dir" => dirs[0] = Some(value()?.into()),
                "--data-dir" => dirs[1] = Some(value()?.into()),
                "--cache-dir" => dirs[2] = Some(value()?.into()),
                "--recordings-dir" => dirs[3] = Some(value()?.into()),
                "--grant-write" => write.push(value()?.into()),
                "--grant-read" => read.push(value()?.into()),
                "--report" => report = Some(value()?.into()),
                other => bail!("{INSTALL_ARG}: unknown argument {other:?}"),
            }
        }
        let [Some(config_dir), Some(data_dir), Some(cache_dir), Some(recordings_dir)] = dirs else {
            bail!("{INSTALL_ARG} needs --config-dir, --data-dir, --cache-dir and --recordings-dir");
        };
        let name = name.context("--name is needed")?;
        if !valid_name(&name) {
            bail!("{name:?} is not a zapper-server service's name");
        }
        let user_sid = sid.context("--sid is needed")?;
        if !valid_sid(&user_sid) {
            bail!("{user_sid:?} is not a SID");
        }
        for d in [&config_dir, &data_dir, &cache_dir, &recordings_dir]
            .into_iter()
            .chain(&write)
            .chain(&read)
        {
            if !d.is_absolute() && !looks_absolute_windows(d) {
                bail!("{} is not a full path", d.display());
            }
        }
        Ok((
            Plan {
                name,
                user_sid,
                config_dir,
                data_dir,
                cache_dir,
                recordings_dir,
                write,
                read,
            },
            report,
        ))
    }

    /// The service's own command line, after the program's name: run as
    /// the service, on the person's folders.
    pub fn service_args(&self) -> Vec<OsString> {
        vec![
            "--service".into(),
            self.name.clone().into(),
            "--config-dir".into(),
            self.config_dir.clone().into(),
            "--data-dir".into(),
            self.data_dir.clone().into(),
            "--cache-dir".into(),
            self.cache_dir.clone().into(),
            "--recordings-dir".into(),
            self.recordings_dir.clone().into(),
        ]
    }
}

/// `zapper-server-` and what [`super::task_name`] leaves of a user name:
/// nothing that could reach another service or make a path of the account.
pub fn valid_name(name: &str) -> bool {
    name.strip_prefix("zapper-server-").is_some_and(|u| {
        !u.is_empty() && u.len() <= 200 && !u.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
            && !u.chars().any(char::is_control)
    })
}

/// `S-1-5-21-…`: the form `ConvertSidToStringSid` writes, which is all that
/// goes into the security descriptor.
pub fn valid_sid(sid: &str) -> bool {
    let mut parts = sid.split('-');
    parts.next() == Some("S")
        && parts.clone().count() >= 3
        && parts.all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `C:\…` or `\\server\…`, read the same on every platform for the tests.
fn looks_absolute_windows(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || s.starts_with("\\\\")
}

/// The account it runs as.
pub fn account(name: &str) -> String {
    format!("NT SERVICE\\{name}")
}

/// Its security descriptor: Windows' default for a service (SYSTEM and
/// administrators manage it, interactive and service logons may look at it),
/// and its person may also start, stop, look at and delete it (CC LC SW RP WP
/// LO CR SD RC), but not change its configuration or its permissions.
pub fn sddl(user_sid: &str) -> String {
    format!(
        "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)\
         (A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)(A;;CCLCSWRPWPLOCRSDRC;;;{user_sid})"
    )
}

/// What Settings › Recording says it is.
pub const KIND: &str = "a Windows service that starts it when the computer starts";

/// The folders a plan grants, deduplicated, and without one inside another
/// already granted the same: a grant is inherited.
pub fn grants(
    dirs: &[&Path],
    output_dir: Option<&Path>,
    exe_dir: Option<&Path>,
    profile: Option<&Path>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut write: Vec<PathBuf> = Vec::new();
    for d in dirs.iter().copied().chain(output_dir) {
        if !write.iter().any(|w| inside(d, w)) {
            write.retain(|w| !inside(w, d));
            write.push(d.to_path_buf());
        }
    }
    let read = exe_dir
        .filter(|e| profile.is_some_and(|p| inside(e, p)))
        .filter(|e| !write.iter().any(|w| inside(e, w)))
        .map(|e| vec![e.to_path_buf()])
        .unwrap_or_default();
    (write, read)
}

/// `a` is `b` or inside it, comparing as Windows does: without case, and
/// with either slash.
fn inside(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let (a, b) = (norm(a), norm(b));
    a == b || a.starts_with(&format!("{b}\\"))
}

/// The Service Control Manager's side, Windows only.
#[cfg(windows)]
pub mod scm {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use anyhow::{anyhow, bail, Context, Result};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TokenUser, DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Services::{SetServiceObjectSecurity, SC_HANDLE};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
    };
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows_service::service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl,
        ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType,
        ServiceState, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    use super::{account, sddl, Plan, INSTALL_ARG};

    /// Windows' error for a service that does not exist.
    const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
    /// ... and for one that is not running, to a stop.
    const ERROR_SERVICE_NOT_ACTIVE: i32 = 1062;
    /// ... and for a UAC prompt that was declined.
    const ERROR_CANCELLED: i32 = 1223;

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access)
            .context("opening the Service Control Manager")
    }

    fn os_code(e: &windows_service::Error) -> Option<i32> {
        match e {
            windows_service::Error::Winapi(io) => io.raw_os_error(),
            _ => None,
        }
    }

    fn open(name: &str, access: ServiceAccess) -> Result<Option<windows_service::service::Service>> {
        match manager(ServiceManagerAccess::CONNECT)?.open_service(name, access) {
            Ok(s) => Ok(Some(s)),
            Err(e) if os_code(&e) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => Ok(None),
            Err(e) => Err(anyhow!(e)).with_context(|| format!("opening the service {name}")),
        }
    }

    fn word(state: ServiceState) -> &'static str {
        match state {
            ServiceState::Running => "running",
            ServiceState::Stopped => "stopped",
            ServiceState::StartPending => "starting",
            ServiceState::StopPending => "stopping",
            _ => "paused",
        }
    }

    /// Its state in a word, or None when there is no such service.
    pub fn state(name: &str) -> Result<Option<String>> {
        let Some(s) = open(name, ServiceAccess::QUERY_STATUS)? else {
            return Ok(None);
        };
        let st = s.query_status().context("reading the service's state")?;
        Ok(Some(word(st.current_state).to_owned()))
    }

    /// Whether there is a service of that name.
    pub fn exists(name: &str) -> bool {
        matches!(open(name, ServiceAccess::QUERY_STATUS), Ok(Some(_)))
    }

    fn wait_for(s: &windows_service::service::Service, want: ServiceState, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        loop {
            match s.query_status() {
                Ok(st) if st.current_state == want => return true,
                Err(_) => return false,
                _ => {}
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Starts it when it is stopped, and waits a little for it to run.
    /// Whether it was started. Nothing when there is no such service.
    pub fn start(name: &str) -> Result<bool> {
        let Some(s) = open(name, ServiceAccess::START | ServiceAccess::QUERY_STATUS)? else {
            return Ok(false);
        };
        if s.query_status()?.current_state != ServiceState::Stopped {
            return Ok(false);
        }
        s.start::<&str>(&[]).with_context(|| format!("starting the service {name}"))?;
        wait_for(&s, ServiceState::Running, Duration::from_secs(15));
        Ok(true)
    }

    /// Stops it, which it takes as Ctrl+C (finishing what it records into
    /// playable files), and waits until it has.
    pub fn stop(name: &str, limit: Duration) -> Result<()> {
        let Some(s) = open(name, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)? else {
            return Ok(());
        };
        if s.query_status()?.current_state == ServiceState::Stopped {
            return Ok(());
        }
        match s.stop() {
            Ok(_) => {}
            Err(e) if os_code(&e) == Some(ERROR_SERVICE_NOT_ACTIVE) => {}
            Err(e) => return Err(anyhow!(e)).with_context(|| format!("stopping the service {name}")),
        }
        if !wait_for(&s, ServiceState::Stopped, limit) {
            bail!("the service {name} did not stop within {}s", limit.as_secs());
        }
        Ok(())
    }

    /// Stops it and deletes it, after taking back what it was granted on
    /// `folders` (while its account still has a name to take it back from).
    pub fn remove(name: &str, folders: &[PathBuf]) -> Result<()> {
        if !exists(name) {
            return Ok(());
        }
        stop(name, Duration::from_secs(150))?;
        for f in folders {
            if f.is_dir() {
                if let Err(e) = icacls(f, &["/remove:g", &account(name)]) {
                    tracing::warn!("recording service: {e:#}");
                }
            }
        }
        let s = open(name, ServiceAccess::DELETE)?
            .ok_or_else(|| anyhow!("the service {name} went away"))?;
        s.delete().with_context(|| format!("deleting the service {name}"))?;
        Ok(())
    }

    fn icacls(dir: &Path, args: &[&str]) -> Result<()> {
        let exe = std::env::var_os("SystemRoot")
            .map(|r| PathBuf::from(r).join("System32").join("icacls.exe"))
            .unwrap_or_else(|| PathBuf::from("icacls.exe"));
        let out = Command::new(exe)
            .arg(dir)
            .args(args)
            .arg("/Q")
            .output()
            .context("running icacls")?;
        if !out.status.success() {
            bail!(
                "icacls {} {}: {}",
                dir.display(),
                args.join(" "),
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(())
    }

    /// Grants the service's account `rights` (`M`, `RX`) on `dir` and what
    /// is in it, now and later.
    pub fn grant(name: &str, dir: &Path, rights: &str) -> Result<()> {
        icacls(dir, &["/grant", &format!("{}:(OI)(CI){rights}", account(name))])
    }

    /// The elevated step: creates the service as the plan says (or brings
    /// one already there up to date), with its permissions, and grants it
    /// its folders. Does not start it: the caller stops whatever runs on its
    /// port first.
    pub fn install_elevated(plan: &Plan, exe: &Path) -> Result<()> {
        let info = ServiceInfo {
            name: plan.name.clone().into(),
            display_name: format!("zapper recording service ({})", user_part(&plan.name)).into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe.to_path_buf(),
            launch_arguments: plan.service_args(),
            dependencies: vec![],
            account_name: Some(account(&plan.name).into()),
            account_password: None,
        };
        let m = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let access = ServiceAccess::ALL_ACCESS;
        let s = match m.open_service(&plan.name, access) {
            Ok(s) => {
                s.change_config(&info)
                    .with_context(|| format!("updating the service {}", plan.name))?;
                s
            }
            Err(e) if os_code(&e) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => m
                .create_service(&info, access)
                .with_context(|| format!("creating the service {}", plan.name))?,
            Err(e) => return Err(anyhow!(e)).context("opening the service"),
        };
        s.set_description(
            "zapper's recording service, installed from Settings > Recording (Start at boot). Remove it there.",
        )?;
        let restart = ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: Duration::from_secs(10),
        };
        s.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![restart.clone(), restart.clone(), restart]),
        })?;
        s.set_failure_actions_on_non_crash_failures(true)?;
        s.set_preshutdown_timeout(Duration::from_secs(60))?;
        set_security(&s, &plan.user_sid)?;
        for d in &plan.write {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
            grant(&plan.name, d, "M")?;
        }
        for d in &plan.read {
            grant(&plan.name, d, "RX")?;
        }
        Ok(())
    }

    fn user_part(name: &str) -> &str {
        name.strip_prefix("zapper-server-").unwrap_or(name)
    }

    fn set_security(s: &windows_service::service::Service, user_sid: &str) -> Result<()> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl(user_sid)),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .context("reading the service's permissions")?;
            let set = SetServiceObjectSecurity(
                SC_HANDLE(s.raw_handle() as _),
                DACL_SECURITY_INFORMATION,
                sd,
            );
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            set.context("setting the service's permissions")?;
        }
        Ok(())
    }

    /// This process's token's user, as `S-1-5-21-…`, and whether it is
    /// elevated.
    fn token() -> Result<(String, bool)> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .context("reading this process's token")?;
            let read = || -> Result<(String, bool)> {
                let mut len = 0u32;
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
                let mut buf = vec![0u8; len as usize];
                GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len)
                    .context("reading this account's SID")?;
                let user = &*(buf.as_ptr() as *const TOKEN_USER);
                let mut text = PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut text)
                    .context("writing this account's SID")?;
                let sid = text.to_string();
                let _ = LocalFree(Some(HLOCAL(text.0.cast())));
                let mut elevation = TOKEN_ELEVATION::default();
                let mut len = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
                let elevated = GetTokenInformation(
                    token,
                    TokenElevation,
                    Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
                    len,
                    &mut len,
                )
                .is_ok()
                    && elevation.TokenIsElevated != 0;
                Ok((sid?, elevated))
            };
            let r = read();
            let _ = CloseHandle(token);
            r
        }
    }

    /// This account's SID.
    pub fn user_sid() -> Result<String> {
        Ok(token()?.0)
    }

    /// Carries out `plan` elevated: in this process when it is already
    /// (an administrator's elevated session), else `exe` with the plan on
    /// its command line, through the UAC prompt, waited for.
    pub fn install_as_administrator(plan: &Plan, exe: &Path) -> Result<()> {
        if token()?.1 {
            return install_elevated(plan, exe);
        }
        let report = std::env::temp_dir().join(format!(
            "zapper-install-service-{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&report);
        let mut args = plan.to_args();
        args.push("--report".into());
        args.push(report.clone().into());
        let params: Vec<String> = args
            .iter()
            .map(|a| super::super::windows_arg(&a.to_string_lossy()))
            .collect();
        let params = HSTRING::from(params.join(" "));
        let file = HSTRING::from(exe.as_os_str());
        let verb = HSTRING::from("runas");
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        unsafe {
            if let Err(e) = ShellExecuteExW(&mut info) {
                if e.code().0 & 0xffff == ERROR_CANCELLED {
                    bail!("an administrator's permission is needed to start the service at boot, and was not given");
                }
                return Err(anyhow!(e)).context("asking for an administrator's permission");
            }
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 0u32;
            let got = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            got.context("reading how the installation went")?;
            if code != 0 {
                let why = std::fs::read_to_string(&report).unwrap_or_default();
                let _ = std::fs::remove_file(&report);
                bail!(
                    "installing the service failed: {}",
                    if why.trim().is_empty() { format!("exit code {code}") } else { why.trim().to_owned() }
                );
            }
        }
        let _ = std::fs::remove_file(&report);
        Ok(())
    }

    /// `zapper-server --install-boot-service …`: the elevated step, its
    /// failure written to the report file for the app to show.
    pub fn run_elevated(args: impl IntoIterator<Item = OsString>) -> i32 {
        let parsed = Plan::from_args(args);
        let report = parsed.as_ref().ok().and_then(|(_, r)| r.clone());
        let result = parsed.and_then(|(plan, _)| {
            let exe = std::env::current_exe().context("finding zapper-server.exe")?;
            install_elevated(&plan, &exe)
        });
        match result {
            Ok(()) => 0,
            Err(e) => {
                if let Some(r) = report {
                    let _ = std::fs::write(r, format!("{e:#}"));
                }
                eprintln!("{INSTALL_ARG}: {e:#}");
                1
            }
        }
    }
}

/// Elsewhere there is no Service Control Manager: no such service, and
/// nothing to do.
#[cfg(not(windows))]
pub mod scm {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{bail, Result};

    use super::Plan;

    pub fn state(_: &str) -> Result<Option<String>> {
        Ok(None)
    }
    pub fn exists(_: &str) -> bool {
        false
    }
    pub fn start(_: &str) -> Result<bool> {
        Ok(false)
    }
    pub fn stop(_: &str, _: Duration) -> Result<()> {
        Ok(())
    }
    pub fn remove(_: &str, _: &[PathBuf]) -> Result<()> {
        Ok(())
    }
    pub fn grant(_: &str, _: &Path, _: &str) -> Result<()> {
        Ok(())
    }
    pub fn user_sid() -> Result<String> {
        bail!("a Windows service is Windows'")
    }
    pub fn install_as_administrator(_: &Plan, _: &Path) -> Result<()> {
        bail!("a Windows service is Windows'")
    }
    pub fn run_elevated(_: impl IntoIterator<Item = OsString>) -> i32 {
        eprintln!("{}: a Windows service is Windows'", super::INSTALL_ARG);
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            name: "zapper-server-ann".into(),
            user_sid: "S-1-5-21-1-2-3-1001".into(),
            config_dir: r"C:\Users\ann\AppData\Roaming\zapper".into(),
            data_dir: r"C:\Users\ann\AppData\Local\zapper".into(),
            cache_dir: r"C:\Users\ann\AppData\Local\zapper".into(),
            recordings_dir: r"C:\Users\ann\Videos\zapper".into(),
            write: vec![r"C:\Users\ann\AppData\Roaming\zapper".into(), r"D:\TV".into()],
            read: vec![r"C:\Users\ann\Apps\zapper".into()],
        }
    }

    #[test]
    fn the_elevated_step_reads_back_the_plan_it_was_given() {
        let p = plan();
        let mut args = p.to_args();
        assert_eq!(args.remove(0), OsString::from(INSTALL_ARG));
        args.push("--report".into());
        args.push(r"C:\Temp\r.txt".into());
        let (back, report) = Plan::from_args(args).unwrap();
        assert_eq!(back, p);
        assert_eq!(report, Some(PathBuf::from(r"C:\Temp\r.txt")));
    }

    #[test]
    fn the_elevated_step_refuses_what_is_not_a_plan() {
        let with = |name: &str, sid: &str, dir: &str| {
            let mut p = plan();
            p.name = name.into();
            p.user_sid = sid.into();
            p.config_dir = dir.into();
            let mut a = p.to_args();
            a.remove(0);
            Plan::from_args(a)
        };
        assert!(with("zapper-server-ann", "S-1-5-21-1-2-3-1001", r"C:\x").is_ok());
        assert!(with("Spooler", "S-1-5-21-1-2-3-1001", r"C:\x").is_err());
        assert!(with("zapper-server-", "S-1-5-21-1-2-3-1001", r"C:\x").is_err());
        assert!(with(r"zapper-server-a\b", "S-1-5-21-1-2-3-1001", r"C:\x").is_err());
        assert!(with("zapper-server-ann", "S-1-5-21-1)(A;;GA;;;WD", r"C:\x").is_err());
        assert!(with("zapper-server-ann", "WD", r"C:\x").is_err());
        assert!(with("zapper-server-ann", "S-1-5-21-1-2-3-1001", r"relative\dir").is_err());
        let mut a = plan().to_args();
        a.remove(0);
        a.push("--bogus".into());
        assert!(Plan::from_args(a).is_err());
    }

    #[test]
    fn the_service_runs_on_the_persons_folders() {
        let a: Vec<String> = plan()
            .service_args()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            a,
            [
                "--service",
                "zapper-server-ann",
                "--config-dir",
                r"C:\Users\ann\AppData\Roaming\zapper",
                "--data-dir",
                r"C:\Users\ann\AppData\Local\zapper",
                "--cache-dir",
                r"C:\Users\ann\AppData\Local\zapper",
                "--recordings-dir",
                r"C:\Users\ann\Videos\zapper",
            ]
        );
        assert_eq!(account("zapper-server-ann"), r"NT SERVICE\zapper-server-ann");
    }

    #[test]
    fn its_person_may_start_stop_and_remove_it_but_not_reconfigure_it() {
        let d = sddl("S-1-5-21-1-2-3-1001");
        let ace = d.split('(').find(|a| a.ends_with(";;;S-1-5-21-1-2-3-1001)")).unwrap();
        let rights = ace.split(';').nth(2).unwrap();
        let has = |r: &str| rights.as_bytes().chunks(2).any(|c| c == r.as_bytes());
        for r in ["CC", "LC", "RP", "WP", "SD", "RC"] {
            assert!(has(r), "{r} in {rights}");
        }
        // DC: change the configuration (and so the account it runs as); WD,
        // WO: its permissions and owner.
        for r in ["DC", "WD", "WO"] {
            assert!(!has(r), "{r} in {rights}");
        }
        assert!(d.starts_with("D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)"));
    }

    #[test]
    fn folders_are_granted_once_and_the_program_only_from_the_profile() {
        let p = |s: &str| PathBuf::from(s);
        let roaming = p(r"C:\Users\ann\AppData\Roaming\zapper");
        let local = p(r"C:\Users\ann\AppData\Local\zapper");
        let videos = p(r"C:\Users\ann\Videos\zapper");
        let dirs = [roaming.as_path(), local.as_path(), local.as_path(), videos.as_path()];
        let profile = p(r"C:\Users\ann");
        // The per-user setup installs into the data folder: already granted.
        let (w, r) = grants(&dirs, Some(&p(r"c:/users/ann/videos/zapper/TV")), Some(&local), Some(&profile));
        assert_eq!(w, vec![roaming.clone(), local.clone(), videos.clone()]);
        assert!(r.is_empty());
        // Somewhere else in the profile: read, not written.
        let mine = p(r"C:\Users\ann\Apps\zapper");
        let (_, r) = grants(&dirs, None, Some(&mine), Some(&profile));
        assert_eq!(r, vec![mine]);
        // Program Files is everyone's to read already.
        let (_, r) = grants(&dirs, None, Some(&p(r"C:\Program Files\zapper")), Some(&profile));
        assert!(r.is_empty());
        // A recordings folder holding the others replaces them.
        let (w, _) = grants(&[videos.as_path()], Some(&p(r"C:\Users\ann\Videos")), None, None);
        assert_eq!(w, vec![p(r"C:\Users\ann\Videos")]);
    }
}
