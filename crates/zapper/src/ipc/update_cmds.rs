//! The updater's commands. Every step is a request to `crate::updates`; the
//! webview never talks to GitHub and never names a URL or a file to act on,
//! so nothing it sends can point an install or the browser somewhere else.

use std::sync::Arc;

use tauri::Manager;

use crate::ipc::types::{InstallOutcomeDto, InstallWhen, UpdateStatusDto};
use crate::updates::{self, install::detect_install_kind, Updates};

/// Builds the updater, makes it the app's state, and starts the automatic
/// check. Infallible: an updater that cannot be built (no HTTP client) leaves
/// the app running with updates reported as not configured.
pub fn install(app: &tauri::AppHandle) {
    let emit: updates::Emit = Arc::new(|status: UpdateStatusDto| {
        crate::events::bus().emit(crate::events::Event::Update(status));
    });
    let quitter = app.clone();
    let runner = Arc::new(updates::SystemRunner {
        quit: Arc::new(move || quitter.exit(0)),
    });
    let exe = std::env::current_exe().ok();
    let kind = detect_install_kind(exe.as_deref());
    let source = updates::github::GitHubReleases::github(updates::RELEASES_REPO);
    let configured = updates::repo_configured(updates::RELEASES_REPO) && source.is_ok();
    if let Err(e) = &source {
        tracing::error!("updates disabled: {e}");
    }
    let source: Arc<dyn updates::UpdateSource> = match source {
        Ok(s) => Arc::new(s),
        Err(_) => Arc::new(Unavailable),
    };
    tracing::info!(
        "updates: {} install, releases from {}{}",
        kind.as_str(),
        updates::RELEASES_REPO,
        if configured { "" } else { " (not configured)" }
    );
    let u = Arc::new(Updates::new(
        updates::UpdateConfig {
            source,
            runner,
            dir: updates::cache_dir().join("updates"),
            kind,
            exe,
            configured,
            releases_page: format!("{}/{}/releases", updates::github::GITHUB_WEB, updates::RELEASES_REPO),
            emit,
        },
        updates::APP_VERSION,
    ));
    app.manage(u.clone());
    updates::spawn_periodic(u);
}

/// Stands in when no HTTP client could be built. Never called: the updater
/// is marked not configured in that case and makes no request.
struct Unavailable;

impl updates::UpdateSource for Unavailable {
    fn releases(&self) -> updates::BoxFuture<'_, Vec<updates::release::GhRelease>> {
        Box::pin(async { Err("updates are unavailable".to_string()) })
    }
    fn fetch<'a>(&'a self, _url: &'a str, _cancel: Arc<std::sync::atomic::AtomicBool>) -> updates::BoxFuture<'a, Vec<u8>> {
        Box::pin(async { Err("updates are unavailable".to_string()) })
    }
    fn download<'a>(
        &'a self,
        _url: &'a str,
        _dest: &'a std::path::Path,
        _progress: updates::Progress,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> updates::BoxFuture<'a, u64> {
        Box::pin(async { Err("updates are unavailable".to_string()) })
    }
}

type State<'a> = tauri::State<'a, Arc<Updates>>;

#[tauri::command]
pub async fn update_status(u: State<'_>) -> Result<UpdateStatusDto, String> {
    Ok(u.status())
}

/// Check for updates, now, whatever the setting says: asking is consent.
#[tauri::command]
pub async fn update_check(u: State<'_>) -> Result<UpdateStatusDto, String> {
    u.check(updates::now()).await
}

#[tauri::command]
pub async fn update_download(u: State<'_>) -> Result<UpdateStatusDto, String> {
    u.inner().start_download()
}

#[tauri::command]
pub async fn update_cancel(u: State<'_>) -> Result<UpdateStatusDto, String> {
    Ok(u.cancel_download())
}

/// `when`: what to do about a recording in progress (Windows); asked about
/// first when not given.
#[tauri::command]
pub async fn update_install(u: State<'_>, when: Option<InstallWhen>) -> Result<InstallOutcomeDto, String> {
    u.install_when(when.unwrap_or_default()).await
}

#[tauri::command]
pub async fn update_restart(u: State<'_>) -> Result<(), String> {
    u.restart()
}

/// Opens the offered release's page, or the list of releases, in the
/// browser. The URL comes from the updater's own state, never from the
/// webview, and always starts with the configured repository's address.
#[tauri::command]
pub async fn update_open_page(u: State<'_>) -> Result<(), String> {
    let url = u
        .release_page()
        .ok_or("Updates aren't configured for this build, so there's no release page to open.")?;
    open_in_browser(&url).map_err(|e| format!("Couldn't open {url}: {e}"))
}

/// Opens OpenSubtitles' sign-up page in the browser. The address is fixed
/// here, never taken from the webview.
#[tauri::command]
pub async fn open_opensubtitles_sign_up() -> Result<(), String> {
    const URL: &str = "https://www.opensubtitles.com/en/users/sign_up";
    open_in_browser(URL).map_err(|e| format!("Couldn't open {URL}: {e}"))
}

pub(crate) fn open_in_browser(url: &str) -> std::io::Result<()> {
    let mut c = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        // Not `cmd /c start`, which would read `&` in a query string as a
        // command separator.
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}
