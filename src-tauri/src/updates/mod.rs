//! Updates from GitHub Releases: the check, the download and its SHA256SUMS
//! check, and the install. No updater plugin: what a release contains and how
//! each package installs is decided here.
//!
//! - **Check:** `GET /repos/<RELEASES_REPO>/releases`, unauthenticated, a
//!   minute after startup and once a day after that (Settings → Updates turns
//!   that off), and on Check for updates. The newest release above this
//!   version is offered (`release.rs`).
//! - **Download:** this install's package (`install.rs` says which kind it
//!   is; `release.rs` names the asset) into `~/.cache/zapper/updates`, with
//!   progress, cancellable. It is kept only if the same release's SHA256SUMS
//!   lists it with the same digest; otherwise it is deleted and never run.
//! - **Install:** the kind's command, run by the `UpdateRunner`, then Restart
//!   starts the new copy. The download lives in a folder this user owns, so
//!   anything else running as this user could replace it between a check and
//!   the install. What each platform guarantees about that:
//!   - **Linux (deb, rpm):** the guarantee is complete. pkexec runs
//!     `install::ROOT_INSTALL` as root, which copies the file into a fresh
//!     root-owned directory, checks *the copy* against the verified digest,
//!     and hands only that copy to apt, dnf, zypper or rpm. A swap at any
//!     moment, the long wait at the password prompt included, is refused
//!     rather than installed as root.
//!   - **Windows (NSIS, MSI; not built yet):** narrowed, not closed. The
//!     package is copied into a folder made for that one attempt and checked
//!     again immediately before the installer starts, but the installer opens
//!     its own file by path afterwards, and a same-user process fast enough
//!     to replace it in that instant would win. Such a process can already
//!     rewrite a per-user install directly; the MSI's elevation (UAC) is the
//!     one place this matters, and code signing is the real answer there.
//!   - **macOS (dmg; not built yet):** checked once more, then installed as
//!     this user. There is no privilege boundary to defend: the process
//!     doing the swap could equally rewrite the `.app` it is replacing.
//!
//! A build from source has no package to install and is only ever pointed at
//! the release page. A build whose [`RELEASES_REPO`] is still the placeholder
//! makes no request at all and says "not configured".
//!
//! The HTTP is behind `UpdateSource` and the processes behind `UpdateRunner`,
//! so the whole flow is tested here without a network or a package manager.

pub mod github;
pub mod install;
pub mod release;
pub mod version;

use crate::ipc::types::{
    InstallOutcomeDto, UpdateAssetDto, UpdateReleaseDto, UpdateStateDto, UpdateStatusDto,
};
use install::{
    app_bundle, bundle_replaceable, install_command, manual_command, relaunch_command,
    InstallCommand, InstallKind,
};
use release::{asset_name, check_sum, newest_update, sha256_file, GhRelease, SumCheck, SUMS_ASSET};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use version::Version;

/// Where zapper's releases are published, as `owner/repository` on GitHub.
///
/// **The one place to set it.** A fork that publishes its own releases
/// changes it here; the release workflow refuses to build while it does not
/// match the repository it runs in (docs/releasing.md, "One-time setup").
/// While it is still the `OWNER/…` placeholder the updater makes no network
/// request and reports "not configured" (see [`repo_configured`]).
pub const RELEASES_REPO: &str = "francislavoie/zapper";

/// The automatic check runs this often.
pub const CHECK_EVERY_SECS: i64 = 24 * 60 * 60;
/// How often the automatic check looks at the clock. Hourly, so a laptop
/// that slept through the due time catches up within the hour.
pub const CHECK_TICK: Duration = Duration::from_secs(60 * 60);
/// After startup, the first automatic check waits this long. Startup already
/// has a catalogue sync and possibly a stream opening on one connection; a
/// release check is the least urgent thing in the process.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
/// Download progress is announced at most this often (and on every percent).
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Whether `repo` names a real repository rather than the placeholder.
pub fn repo_configured(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    owner != "OWNER" && ok(owner) && ok(name)
}

/// This build's version.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;
pub type Progress = Arc<dyn Fn(u64) + Send + Sync>;

/// Fetches releases and their files. Never with a token.
pub trait UpdateSource: Send + Sync {
    /// The repository's releases, newest first.
    fn releases(&self) -> BoxFuture<'_, Vec<GhRelease>>;
    /// A small asset (SHA256SUMS), whole, within a deadline. Stops once
    /// `cancel` is set.
    fn fetch<'a>(&'a self, url: &'a str, cancel: Arc<AtomicBool>) -> BoxFuture<'a, Vec<u8>>;
    /// Streams `url` into `dest`, reporting bytes so far. Stops once `cancel`
    /// is set; on any error or cancel, `dest` is removed.
    fn download<'a>(&'a self, url: &'a str, dest: &'a Path, progress: Progress, cancel: Arc<AtomicBool>) -> BoxFuture<'a, u64>;
}

/// A finished command: its exit code (`None`: killed by a signal) and the end
/// of its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub code: Option<i32>,
    pub output: String,
}

pub type RunFuture<'a> = Pin<Box<dyn Future<Output = std::io::Result<RunOutput>> + Send + 'a>>;

/// Runs the install commands. The app's runs them; the tests record them.
pub trait UpdateRunner: Send + Sync {
    /// Runs `cmd` and waits. An `Err` of kind `NotFound`: no such program.
    fn run<'a>(&'a self, cmd: &'a InstallCommand) -> RunFuture<'a>;
    /// Starts `cmd` on its own, to outlive zapper.
    fn spawn(&self, cmd: &InstallCommand) -> std::io::Result<()>;
    /// Quits zapper, as closing the window does.
    fn quit(&self);
}

pub type Emit = Arc<dyn Fn(UpdateStatusDto) + Send + Sync>;

/// The updater's parts.
pub struct UpdateConfig {
    pub source: Arc<dyn UpdateSource>,
    pub runner: Arc<dyn UpdateRunner>,
    /// Downloads go here.
    pub dir: PathBuf,
    pub kind: InstallKind,
    /// The binary as it was at startup. Read once, because after apt has
    /// replaced it `current_exe()` answers `/usr/bin/zapper (deleted)`.
    pub exe: Option<PathBuf>,
    /// False while [`RELEASES_REPO`] is the placeholder.
    pub configured: bool,
    /// The releases page, for a build that can only be sent there.
    pub releases_page: String,
    /// Told about every state change; the app turns it into an event.
    pub emit: Emit,
}

/// The release found, and where its files are.
#[derive(Clone)]
struct Found {
    release: UpdateReleaseDto,
    asset_url: Option<String>,
    sums_url: Option<String>,
    /// Once verified: the file and its digest.
    file: Option<(PathBuf, String)>,
}

struct Inner {
    state: UpdateStateDto,
    found: Option<Found>,
    /// The running download's id and stop flag.
    download: Option<(u64, Arc<AtomicBool>)>,
    next_download: u64,
    /// The last check that got an answer (unix seconds).
    last_check: Option<i64>,
    /// When progress was last announced, and at what percent.
    announced: (Option<Instant>, u64),
    /// A check is waiting on GitHub. Its own flag rather than a state,
    /// because a re-check leaves an offered release on screen meanwhile.
    checking: bool,
    /// Restart has been asked for and the relaunch started: a second press
    /// must not start a second copy.
    restarting: bool,
}

pub struct Updates {
    cfg: UpdateConfig,
    current: Version,
    inner: Mutex<Inner>,
}

fn release_of(state: &UpdateStateDto) -> Option<&UpdateReleaseDto> {
    match state {
        UpdateStateDto::Available { release }
        | UpdateStateDto::Downloading { release, .. }
        | UpdateStateDto::Ready { release }
        | UpdateStateDto::Installing { release }
        | UpdateStateDto::Installed { release } => Some(release),
        UpdateStateDto::Failed { release, .. } => release.as_ref(),
        UpdateStateDto::NotConfigured
        | UpdateStateDto::Idle
        | UpdateStateDto::Checking
        | UpdateStateDto::UpToDate => None,
    }
}

/// A download, a verified package waiting, or an install under way: states a
/// check must not replace.
fn busy(state: &UpdateStateDto) -> bool {
    matches!(
        state,
        UpdateStateDto::Downloading { .. }
            | UpdateStateDto::Ready { .. }
            | UpdateStateDto::Installing { .. }
            | UpdateStateDto::Installed { .. }
    )
}

impl Updates {
    pub fn new(cfg: UpdateConfig, current: &str) -> Self {
        // A version that does not parse (never, from Cargo) is older than
        // every release, which errs on the side of offering one.
        let current = Version::parse(current).unwrap_or_else(|| Version::parse("0.0.0").expect("0.0.0 parses"));
        let state = if cfg.configured {
            UpdateStateDto::Idle
        } else {
            UpdateStateDto::NotConfigured
        };
        Self {
            cfg,
            current,
            inner: Mutex::new(Inner {
                state,
                found: None,
                download: None,
                next_download: 1,
                last_check: None,
                announced: (None, 0),
                checking: false,
                restarting: false,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding it would be a bug in this file; the state it
        // guards is still coherent (each change is one assignment), so carry
        // on rather than poisoning every later call.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn snapshot(&self, inner: &Inner) -> UpdateStatusDto {
        UpdateStatusDto {
            current: self.current.without_build(),
            kind: self.cfg.kind.as_str().to_string(),
            last_checked: inner.last_check,
            state: inner.state.clone(),
        }
    }

    pub fn status(&self) -> UpdateStatusDto {
        self.snapshot(&self.lock())
    }

    /// Sets the state and announces it.
    fn set(&self, inner: &mut Inner, state: UpdateStateDto) {
        inner.state = state;
        (self.cfg.emit)(self.snapshot(inner));
    }

    pub fn configured(&self) -> bool {
        self.cfg.configured
    }

    /// The automatic check is due: never checked this run, or a day since.
    pub fn due(&self, now: i64) -> bool {
        self.lock()
            .last_check
            .is_none_or(|t| now.saturating_sub(t) >= CHECK_EVERY_SECS)
    }

    /// Asks GitHub for the releases. A download, a verified package or an
    /// install in progress is left alone, and so is a check already running.
    ///
    /// A release already on offer stays on offer while the check runs: the
    /// state only says `Checking` when there is nothing to show instead. The
    /// daily re-check of an update nobody has installed yet would otherwise
    /// blank the title bar's badge, and the dialog's button, for the length of
    /// a request, then announce the same update all over again.
    ///
    /// A failed check restores the state it found and returns the error, so a
    /// flaky network at 3am does not leave a red badge for the morning.
    pub async fn check(&self, now: i64) -> Result<UpdateStatusDto, String> {
        let before = {
            let mut inner = self.lock();
            if inner.checking || busy(&inner.state) || inner.state == UpdateStateDto::NotConfigured {
                return Ok(self.snapshot(&inner));
            }
            inner.checking = true;
            let before = inner.state.clone();
            if release_of(&before).is_none() {
                self.set(&mut inner, UpdateStateDto::Checking);
            }
            before
        };
        let listed = self.cfg.source.releases().await;
        let mut inner = self.lock();
        inner.checking = false;
        let releases = match listed {
            Ok(r) => r,
            Err(e) => {
                if inner.state == UpdateStateDto::Checking {
                    self.set(&mut inner, before);
                }
                return Err(e);
            }
        };
        inner.last_check = Some(now);
        if busy(&inner.state) {
            // A download started while the list was on its way: what it is
            // downloading is what was offered, and replacing `found` now would
            // lose track of the file it is about to verify.
            return Ok(self.snapshot(&inner));
        }
        let found = newest_update(&releases, &self.current).map(|(r, v)| self.found(r, &v));
        let state = match &found {
            Some(f) => UpdateStateDto::Available { release: f.release.clone() },
            None => UpdateStateDto::UpToDate,
        };
        inner.found = found;
        self.set(&mut inner, state);
        Ok(self.snapshot(&inner))
    }

    fn found(&self, r: &GhRelease, v: &Version) -> Found {
        let asset = asset_name(self.cfg.kind, v).and_then(|n| r.asset(&n));
        let version = v.without_build();
        Found {
            release: UpdateReleaseDto {
                name: r
                    .name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| format!("zapper {version}")),
                version,
                notes: r.body.clone().unwrap_or_default(),
                url: r.html_url.clone(),
                prerelease: r.prerelease || v.is_prerelease(),
                published_at: r.published_at.clone(),
                asset: asset.map(|a| UpdateAssetDto {
                    name: a.name.clone(),
                    size: a.size,
                }),
            },
            asset_url: asset.map(|a| a.browser_download_url.clone()),
            sums_url: r.asset(SUMS_ASSET).map(|a| a.browser_download_url.clone()),
            file: None,
        }
    }

    /// Starts downloading the offered package (again, after a failure).
    /// Progress and the outcome arrive as state changes.
    pub fn start_download(self: &Arc<Self>) -> Result<UpdateStatusDto, String> {
        let mut inner = self.lock();
        let offered = matches!(
            inner.state,
            UpdateStateDto::Available { .. } | UpdateStateDto::Failed { release: Some(_), .. }
        );
        let found = inner
            .found
            .clone()
            .filter(|_| offered)
            .ok_or("There's no update to download")?;
        let (Some(url), Some(asset)) = (found.asset_url.clone(), found.release.asset.clone()) else {
            return Err("This copy of zapper has no package to download: get the update from its release page".into());
        };
        let id = inner.next_download;
        inner.next_download += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        inner.download = Some((id, cancel.clone()));
        inner.announced = (None, 0);
        self.set(
            &mut inner,
            UpdateStateDto::Downloading {
                release: found.release.clone(),
                received: 0,
                total: asset.size,
            },
        );
        let snapshot = self.snapshot(&inner);
        drop(inner);
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.download(id, found, url, asset, cancel).await });
        Ok(snapshot)
    }

    fn is_current_download(inner: &Inner, id: u64) -> bool {
        inner.download.as_ref().is_some_and(|(d, _)| *d == id)
    }

    async fn download(self: Arc<Self>, id: u64, found: Found, url: String, asset: UpdateAssetDto, cancel: Arc<AtomicBool>) {
        let release = found.release.clone();
        let dir = self.cfg.dir.clone();
        // Its own name per attempt, so a cancelled one still unwinding never
        // deletes the next one's file.
        let part = dir.join(format!("{}.{id}.part", asset.name));
        let target = dir.join(&asset.name);
        let fail = |me: &Self, message: String| {
            let _ = std::fs::remove_file(&part);
            let mut inner = me.lock();
            if Self::is_current_download(&inner, id) {
                inner.download = None;
                me.set(&mut inner, UpdateStateDto::Failed { message, release: Some(release.clone()) });
            }
        };
        if let Err(e) = prepare_dir(&dir) {
            return fail(&self, format!("Couldn't prepare {} for the download: {e}", dir.display()));
        }
        let me = self.clone();
        let (rel, total) = (release.clone(), asset.size);
        let progress: Progress = Arc::new(move |received| me.progress(id, &rel, received, total));
        let got = self.cfg.source.download(&url, &part, progress, cancel.clone()).await;
        if cancel.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&part);
            return;
        }
        if let Err(e) = got {
            return fail(&self, format!("Couldn't download the update: {e}"));
        }
        let Some(sums_url) = found.sums_url.clone() else {
            return fail(&self, format!("The release has no {SUMS_ASSET} to check the download against, so zapper deleted it."));
        };
        let sums = match self.cfg.source.fetch(&sums_url, cancel.clone()).await {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(_) if cancel.load(Ordering::SeqCst) => {
                let _ = std::fs::remove_file(&part);
                return;
            }
            Err(e) => {
                return fail(&self, format!("Couldn't get the release's {SUMS_ASSET} ({e}), so zapper deleted the download."))
            }
        };
        let (path, name) = (part.clone(), asset.name.clone());
        let checked = tokio::task::spawn_blocking(move || check_sum(&path, &name, &sums)).await;
        if cancel.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&part);
            return;
        }
        let digest = match checked {
            Ok(Ok(SumCheck::Good(d))) => d,
            Ok(Ok(SumCheck::Missing)) => {
                return fail(&self, format!("{SUMS_ASSET} doesn't list {}, so zapper deleted the download and won't install it.", asset.name))
            }
            Ok(Ok(SumCheck::Mismatch)) => {
                return fail(&self, format!("The download doesn't match its checksum in {SUMS_ASSET}, so zapper deleted it and won't install it."))
            }
            Ok(Err(e)) => return fail(&self, format!("Couldn't read the download to check it: {e}")),
            Err(e) => return fail(&self, format!("Couldn't check the download: {e}")),
        };
        // Still the current download? Asked and acted on under one lock: a
        // cancelled attempt finishing late shares the target's name with the
        // attempt that replaced it, and must neither rename over that one's
        // verified file nor delete it.
        let mut inner = self.lock();
        if cancel.load(Ordering::SeqCst) || !Self::is_current_download(&inner, id) {
            drop(inner);
            let _ = std::fs::remove_file(&part);
            return;
        }
        if let Err(e) = std::fs::rename(&part, &target) {
            drop(inner);
            return fail(&self, format!("Couldn't keep the download: {e}"));
        }
        inner.download = None;
        if let Some(f) = inner.found.as_mut() {
            f.file = Some((target, digest));
        }
        self.set(&mut inner, UpdateStateDto::Ready { release });
    }

    fn progress(&self, id: u64, release: &UpdateReleaseDto, received: u64, total: u64) {
        let mut inner = self.lock();
        if !Self::is_current_download(&inner, id) {
            return;
        }
        let percent = received.saturating_mul(100).checked_div(total).unwrap_or(0);
        let (last, last_percent) = inner.announced;
        let state = UpdateStateDto::Downloading {
            release: release.clone(),
            received,
            total,
        };
        if last.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY) || percent != last_percent {
            inner.announced = (Some(Instant::now()), percent);
            self.set(&mut inner, state);
        } else {
            inner.state = state;
        }
    }

    /// Stops the download; the update is offered again.
    pub fn cancel_download(&self) -> UpdateStatusDto {
        let mut inner = self.lock();
        if let Some((_, cancel)) = inner.download.take() {
            cancel.store(true, Ordering::SeqCst);
            if let Some(release) = release_of(&inner.state).cloned() {
                self.set(&mut inner, UpdateStateDto::Available { release });
            }
        }
        self.snapshot(&inner)
    }

    /// Back to Ready after an install that did not happen, so it can be tried
    /// again.
    fn back_to_ready(&self, release: UpdateReleaseDto) {
        let mut inner = self.lock();
        self.set(&mut inner, UpdateStateDto::Ready { release });
    }

    /// Installs the verified package, its digest checked once more first.
    ///
    /// Ready becomes Installing under the same lock that checked it was
    /// Ready, so a second press (or a second window's) gets "no verified
    /// update" rather than a second package manager run.
    pub async fn install(&self) -> Result<InstallOutcomeDto, String> {
        let (release, file, digest) = {
            let mut inner = self.lock();
            let claimed = match (&inner.state, inner.found.as_ref().and_then(|f| f.file.clone())) {
                (UpdateStateDto::Ready { release }, Some((file, digest))) => (release.clone(), file, digest),
                _ => return Err("There's no verified update to install".into()),
            };
            self.set(&mut inner, UpdateStateDto::Installing { release: claimed.0.clone() });
            claimed
        };
        let path = file.clone();
        let again = match tokio::task::spawn_blocking(move || sha256_file(&path)).await {
            Ok(again) => again,
            Err(e) => {
                self.back_to_ready(release);
                return Err(e.to_string());
            }
        };
        if again.ok().as_deref() != Some(digest.as_str()) {
            let _ = std::fs::remove_file(&file);
            let message = "The downloaded package changed since it was checked, so zapper deleted it. Download it again.".to_string();
            let mut inner = self.lock();
            if let Some(f) = inner.found.as_mut() {
                f.file = None;
            }
            self.set(&mut inner, UpdateStateDto::Failed { message: message.clone(), release: Some(release) });
            return Err(message);
        }
        let kind = self.cfg.kind;
        let bundle = self.cfg.exe.as_deref().and_then(app_bundle);
        if kind == InstallKind::Dmg {
            let cannot = match &bundle {
                None => Some("zapper isn't running from zapper.app, so it can't replace it.".to_string()),
                Some(b) if !bundle_replaceable(b) => Some(format!(
                    "zapper can't write to {}, so it can't replace itself there.",
                    b.parent().unwrap_or(b).display()
                )),
                Some(_) => None,
            };
            if let Some(why) = cannot {
                self.back_to_ready(release);
                return Ok(InstallOutcomeDto::Manual {
                    command: manual_command(kind, &file).unwrap_or_default(),
                    reason: format!("{why} Open the disk image and drag zapper to Applications, replacing the old one."),
                    output: None,
                });
            }
        }
        if kind.quits_to_install() {
            // The installer is run by path from this user's own folder, and
            // Windows offers nothing like pkexec to copy it across a privilege
            // boundary first. So: a fresh copy in a folder made for this one
            // attempt, checked against the digest immediately before it is
            // started. That narrows the window to the moment between the
            // check and the installer opening its own file; see the module
            // comment for what that does and does not guarantee.
            let (dir, from, want) = (self.cfg.dir.clone(), file.clone(), digest.clone());
            let staged = match tokio::task::spawn_blocking(move || stage_for_launch(&dir, &from, &want)).await {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => {
                    self.back_to_ready(release);
                    return Err(format!("Couldn't prepare the installer: {e}"));
                }
                Err(e) => {
                    self.back_to_ready(release);
                    return Err(e.to_string());
                }
            };
            let Some(cmd) = install_command(kind, &staged, &digest, None) else {
                self.back_to_ready(release);
                return Err("This copy of zapper has no package to install".into());
            };
            tracing::info!("starting the installer: {}", cmd.display());
            if let Err(e) = self.cfg.runner.spawn(&cmd) {
                self.back_to_ready(release);
                return Err(format!("Couldn't start the installer: {e}"));
            }
            self.cfg.runner.quit();
            return Ok(InstallOutcomeDto::Quitting);
        }
        let Some(cmd) = install_command(kind, &file, &digest, bundle.as_deref()) else {
            self.back_to_ready(release);
            return Err("This copy of zapper has no package to install".into());
        };
        tracing::info!("installing the update: {}", cmd.display());
        let ran = self.cfg.runner.run(&cmd).await;
        let manual = manual_command(kind, &file).unwrap_or_default();
        let mut output = None;
        let pkexec = kind.uses_pkexec();
        let reason = match ran {
            Ok(out) if out.code == Some(0) => {
                let mut inner = self.lock();
                self.set(&mut inner, UpdateStateDto::Installed { release });
                return Ok(InstallOutcomeDto::Installed);
            }
            // pkexec's own codes: 126, the prompt was dismissed or refused;
            // 127, no authentication agent to ask with.
            Ok(out) if pkexec && matches!(out.code, Some(126 | 127)) => {
                "zapper couldn't get permission to install it: the password prompt was cancelled, or there's no polkit agent to ask.".to_string()
            }
            Ok(out) => {
                output = Some(install_output(&out.output)).filter(|o| !o.is_empty());
                let code = out.code.map_or("a signal".to_string(), |c| format!("exit {c}"));
                format!("The install failed ({code}).")
            }
            Err(e) if pkexec && e.kind() == std::io::ErrorKind::NotFound => {
                "pkexec isn't installed, so zapper can't ask for the password itself.".to_string()
            }
            Err(e) if pkexec => format!("Couldn't start pkexec: {e}"),
            Err(e) => format!("Couldn't run the install: {e}"),
        };
        tracing::warn!("the update didn't install: {reason} {}", output.as_deref().unwrap_or(""));
        self.back_to_ready(release);
        Ok(InstallOutcomeDto::Manual { command: manual, reason, output })
    }

    /// Starts the installed version once this one has quit, and quits. Once:
    /// the flag is taken under the lock, so a second press cannot start a
    /// second copy while the first is still waiting for this one to exit.
    pub fn restart(&self) -> Result<(), String> {
        let exe = {
            let mut inner = self.lock();
            if !matches!(inner.state, UpdateStateDto::Installed { .. }) {
                return Err("No update has been installed".into());
            }
            if inner.restarting {
                return Err("zapper is already restarting".into());
            }
            let exe = self
                .cfg
                .exe
                .clone()
                .ok_or("zapper doesn't know where its binary is: start it again yourself")?;
            inner.restarting = true;
            exe
        };
        // A bundle restarts as a bundle: `open` gives it its Dock icon and
        // menu bar; the binary inside, run directly, would get neither.
        let cmd = match app_bundle(&exe) {
            Some(bundle) => relaunch_command(std::process::id(), &[Path::new("/usr/bin/open"), &bundle]),
            None => relaunch_command(std::process::id(), &[&exe]),
        };
        if let Err(e) = self.cfg.runner.spawn(&cmd) {
            self.lock().restarting = false;
            return Err(format!("Couldn't restart zapper: {e}"));
        }
        self.cfg.runner.quit();
        Ok(())
    }

    /// Where to send someone who has to update by hand: the offered release's
    /// page, else the list of releases. `None` while not configured.
    pub fn release_page(&self) -> Option<String> {
        if !self.cfg.configured {
            return None;
        }
        let inner = self.lock();
        Some(
            release_of(&inner.state)
                .map(|r| r.url.clone())
                .filter(|u| u.starts_with(&self.cfg.releases_page))
                .unwrap_or_else(|| self.cfg.releases_page.clone()),
        )
    }
}

/// The downloads folder, emptied of older downloads: one update is kept at a
/// time, and a stale package must never be the one installed.
fn prepare_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Only this user may put a file where the installer will look.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() {
            std::fs::remove_file(entry.path())?;
        } else if kind.is_dir() && entry.file_name().to_string_lossy().starts_with(LAUNCH_PREFIX) {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

/// The per-attempt folders [`stage_for_launch`] makes.
const LAUNCH_PREFIX: &str = "launch-";

/// A copy of `file` in a folder made for this one launch, checked against
/// `sha256` after it is written: what a Windows installer is started from.
fn stage_for_launch(dir: &Path, file: &Path, sha256: &str) -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let fresh = dir.join(format!("{LAUNCH_PREFIX}{}-{nanos}", std::process::id()));
    // create_dir, not create_dir_all: a folder that already exists was not
    // made for this attempt.
    std::fs::create_dir(&fresh)?;
    let name = file.file_name().ok_or_else(|| std::io::Error::other("the package has no file name"))?;
    let staged = fresh.join(name);
    std::fs::copy(file, &staged)?;
    if sha256_file(&staged)? != sha256 {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(std::io::Error::other(
            "the package changed after zapper checked it, so it was not started",
        ));
    }
    Ok(staged)
}

/// A package manager's output for the dialog: less apt's warning that its
/// CLI is not stable (it prints it whenever stdout is not a terminal).
fn install_output(out: &str) -> String {
    out.lines()
        .filter(|l| !l.contains("apt does not have a stable CLI interface"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// `~/.cache/zapper`, or the platform's equivalent. The updates folder lives
/// under it because a downloaded package is disposable: the next check can
/// always fetch it again.
pub fn cache_dir() -> PathBuf {
    crate::paths::cache_dir()
}

/// The app's runner: real processes.
pub struct SystemRunner {
    pub quit: Arc<dyn Fn() + Send + Sync>,
}

/// How much of a failed command's output the dialog shows, from the end.
const OUTPUT_TAIL: usize = 1200;

impl UpdateRunner for SystemRunner {
    fn run<'a>(&'a self, cmd: &'a InstallCommand) -> RunFuture<'a> {
        let cmd = cmd.clone();
        Box::pin(async move {
            // Blocking, on the blocking pool: the package manager takes as
            // long as it takes, and tokio's process support is a feature this
            // crate does not otherwise need.
            let out = tokio::task::spawn_blocking(move || {
                std::process::Command::new(&cmd.program)
                    .args(&cmd.args)
                    .stdin(std::process::Stdio::null())
                    .output()
            })
            .await
            .map_err(std::io::Error::other)??;
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            let start = text.len().saturating_sub(OUTPUT_TAIL);
            let start = (start..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
            Ok(RunOutput {
                code: out.status.code(),
                output: text[start..].to_string(),
            })
        })
    }

    fn spawn(&self, cmd: &InstallCommand) -> std::io::Result<()> {
        let mut c = std::process::Command::new(&cmd.program);
        c.args(&cmd.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Its own process group, so whatever ends zapper's group (a terminal
        // closing, the session manager) does not take the relaunch with it.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        c.spawn().map(|_| ())
    }

    fn quit(&self) {
        (self.quit)();
    }
}

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The automatic check: once, [`FIRST_CHECK_DELAY`] after startup, then
/// whenever a day has passed, for as long as the setting is on. Reads the
/// setting each time round, so turning it off takes effect without a restart.
/// Does nothing at all while not configured.
pub fn spawn_periodic(updates: Arc<Updates>) {
    if !updates.configured() {
        tracing::info!("updates: {RELEASES_REPO} is a placeholder; not checking");
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let on = crate::catalog::settings::load_or_default().check_for_updates;
            if on && updates.due(now()) {
                if let Err(e) = updates.check(now()).await {
                    // Quietly: the next tick tries again, and a person who
                    // wants to know asks with Check for updates.
                    tracing::warn!("update check failed: {e}");
                }
            }
            tokio::time::sleep(CHECK_TICK).await;
        }
    });
}

#[cfg(test)]
mod tests;
