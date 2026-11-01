//! The whole flow with a fake GitHub and a runner that only records: check,
//! download, the SHA256SUMS verdicts, install and its failure modes, restart.

use super::release::{release, GhAsset};
use super::*;
use std::sync::atomic::AtomicUsize;

/// sha256("hello")
const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

struct FakeSource {
    releases: Mutex<Result<Vec<GhRelease>, String>>,
    sums: Mutex<String>,
    body: Vec<u8>,
    calls: AtomicUsize,
    /// When set, the next releases() call waits for it: a check held open.
    releases_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// When set, the next fetch() call waits for it: a download held between
    /// its package and its SHA256SUMS.
    fetch_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl UpdateSource for FakeSource {
    fn releases(&self) -> BoxFuture<'_, Vec<GhRelease>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let r = self.releases.lock().unwrap().clone();
        let gate = self.releases_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some(g) = gate {
                g.notified().await;
            }
            r
        })
    }
    fn fetch<'a>(&'a self, _url: &'a str, _cancel: Arc<AtomicBool>) -> BoxFuture<'a, Vec<u8>> {
        let s = self.sums.lock().unwrap().clone().into_bytes();
        let gate = self.fetch_gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some(g) = gate {
                g.notified().await;
            }
            Ok(s)
        })
    }
    fn download<'a>(&'a self, _url: &'a str, dest: &'a Path, progress: Progress, _cancel: Arc<AtomicBool>) -> BoxFuture<'a, u64> {
        Box::pin(async move {
            std::fs::write(dest, &self.body).map_err(|e| e.to_string())?;
            progress(self.body.len() as u64);
            Ok(self.body.len() as u64)
        })
    }
}

#[derive(Default)]
struct FakeRunner {
    ran: Mutex<Vec<InstallCommand>>,
    spawned: Mutex<Vec<InstallCommand>>,
    quits: AtomicUsize,
    /// What `run` answers: an exit code, or `None` for "no such program".
    code: Mutex<Option<i32>>,
}

impl UpdateRunner for FakeRunner {
    fn run<'a>(&'a self, cmd: &'a InstallCommand) -> RunFuture<'a> {
        self.ran.lock().unwrap().push(cmd.clone());
        let code = *self.code.lock().unwrap();
        Box::pin(async move {
            match code {
                Some(c) => Ok(RunOutput { code: Some(c), output: "E: something broke\n".into() }),
                None => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no pkexec")),
            }
        })
    }
    fn spawn(&self, cmd: &InstallCommand) -> std::io::Result<()> {
        self.spawned.lock().unwrap().push(cmd.clone());
        Ok(())
    }
    fn quit(&self) {
        self.quits.fetch_add(1, Ordering::SeqCst);
    }
}

fn deb_name() -> String {
    asset_name(InstallKind::Deb, &Version::parse("0.3.0").unwrap()).unwrap()
}

fn a_release_with_the_deb() -> GhRelease {
    let mut r = release("v0.3.0", false, false);
    r.body = Some("Faster zapping.".into());
    r.html_url = "https://github.com/Someone/zapper/releases/tag/v0.3.0".into();
    let base = "https://github.com/Someone/zapper/releases/download/v0.3.0";
    r.assets = vec![
        GhAsset { name: deb_name(), size: 5, browser_download_url: format!("{base}/{}", deb_name()) },
        GhAsset { name: "SHA256SUMS".into(), size: 90, browser_download_url: format!("{base}/SHA256SUMS") },
    ];
    r
}

struct Rig {
    updates: Arc<Updates>,
    source: Arc<FakeSource>,
    runner: Arc<FakeRunner>,
    emitted: Arc<Mutex<Vec<UpdateStatusDto>>>,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(kind: InstallKind, configured: bool, current: &str) -> Rig {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "zapper-updates-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let source = Arc::new(FakeSource {
        releases: Mutex::new(Ok(vec![release("v0.2.0", false, false), a_release_with_the_deb()])),
        sums: Mutex::new(format!("{HELLO}  {}\n", deb_name())),
        body: b"hello".to_vec(),
        calls: AtomicUsize::new(0),
        releases_gate: Mutex::new(None),
        fetch_gate: Mutex::new(None),
    });
    let runner = Arc::new(FakeRunner { code: Mutex::new(Some(0)), ..Default::default() });
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let log = emitted.clone();
    let updates = Arc::new(Updates::new(
        UpdateConfig {
            source: source.clone(),
            runner: runner.clone(),
            dir: dir.join("updates"),
            kind,
            exe: Some(PathBuf::from("/usr/bin/zapper")),
            configured,
            releases_page: "https://github.com/Someone/zapper/releases".into(),
            emit: Arc::new(move |s| log.lock().unwrap().push(s)),
        },
        current,
    ));
    Rig { updates, source, runner, emitted, dir }
}

async fn settle(u: &Updates) -> UpdateStateDto {
    for _ in 0..200 {
        let s = u.status().state;
        if !matches!(s, UpdateStateDto::Downloading { .. }) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the download never finished");
}

#[test]
fn the_placeholder_repo_is_not_configured() {
    // Holds until the real repository is set, and stays true after.
    assert_eq!(repo_configured(RELEASES_REPO), !RELEASES_REPO.starts_with("OWNER/"));
    assert!(!repo_configured("OWNER/zapper"));
    assert!(!repo_configured(""));
    assert!(!repo_configured("zapper"));
    assert!(!repo_configured("a/b/c"));
    assert!(!repo_configured("someone/"));
    assert!(!repo_configured("some one/zapper"));
    assert!(repo_configured("francislavoie/zapper"));
    assert!(repo_configured("Some-Org/zapper.app"));
}

#[tokio::test]
async fn not_configured_never_touches_the_network() {
    let r = rig(InstallKind::Deb, false, "0.2.0");
    assert_eq!(r.updates.status().state, UpdateStateDto::NotConfigured);
    let s = r.updates.check(1000).await.unwrap();
    assert_eq!(s.state, UpdateStateDto::NotConfigured);
    assert_eq!(r.source.calls.load(Ordering::SeqCst), 0, "no request at all");
    assert_eq!(r.updates.release_page(), None);
    assert!(r.updates.start_download().is_err());
}

#[tokio::test]
async fn a_check_offers_the_newest_release_with_this_installs_package() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    assert!(r.updates.due(1000));
    let s = r.updates.check(1000).await.unwrap();
    let UpdateStateDto::Available { release } = s.state else { panic!("{s:?}") };
    assert_eq!(release.version, "0.3.0");
    assert_eq!(release.notes, "Faster zapping.");
    assert_eq!(release.asset.unwrap().name, deb_name());
    assert_eq!((s.current.as_str(), s.kind.as_str(), s.last_checked), ("0.2.0", "deb", Some(1000)));
    assert!(!r.updates.due(1000 + 3600), "checked an hour ago");
    assert!(r.updates.due(1000 + CHECK_EVERY_SECS));
    let states: Vec<_> = r.emitted.lock().unwrap().iter().map(|s| s.state.clone()).collect();
    assert!(matches!(states[..], [UpdateStateDto::Checking, UpdateStateDto::Available { .. }]), "{states:?}");
    assert_eq!(
        r.updates.release_page().as_deref(),
        Some("https://github.com/Someone/zapper/releases/tag/v0.3.0")
    );
}

#[tokio::test]
async fn a_re_check_keeps_the_offered_update_on_screen() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    r.updates.check(1).await.unwrap();
    r.emitted.lock().unwrap().clear();
    let s = r.updates.check(2 + CHECK_EVERY_SECS).await.unwrap();
    assert!(matches!(s.state, UpdateStateDto::Available { .. }));
    let states: Vec<_> = r.emitted.lock().unwrap().iter().map(|s| s.state.clone()).collect();
    assert!(
        states.iter().all(|s| matches!(s, UpdateStateDto::Available { .. })),
        "never Checking while an update is on offer: {states:?}"
    );
    // A failed re-check leaves it there too.
    *r.source.releases.lock().unwrap() = Err("offline".into());
    assert!(r.updates.check(3 + 2 * CHECK_EVERY_SECS).await.is_err());
    assert!(matches!(r.updates.status().state, UpdateStateDto::Available { .. }));
}

/// A re-check whose answer arrives after a download has started must not
/// replace what the download is fetching, nor the record of its file.
#[tokio::test]
async fn a_check_answering_mid_download_leaves_the_download_alone() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    r.updates.check(1).await.unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    *r.source.releases_gate.lock().unwrap() = Some(gate.clone());
    let u = r.updates.clone();
    let late = tokio::spawn(async move { u.check(2 + CHECK_EVERY_SECS).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(r.updates.check(3).await.is_ok(), "a second check while one runs just reports");
    r.updates.start_download().unwrap();
    assert!(matches!(settle(&r.updates).await, UpdateStateDto::Ready { .. }));
    gate.notify_one();
    late.await.unwrap().unwrap();
    assert!(matches!(r.updates.status().state, UpdateStateDto::Ready { .. }), "still Ready");
    assert_eq!(r.updates.install().await.unwrap(), InstallOutcomeDto::Installed, "and still knows its file");
}

#[tokio::test]
async fn a_release_page_outside_the_repo_is_not_passed_on() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    let mut rel = a_release_with_the_deb();
    rel.html_url = "https://evil.example/zapper".into();
    *r.source.releases.lock().unwrap() = Ok(vec![rel]);
    r.updates.check(1).await.unwrap();
    assert_eq!(r.updates.release_page().as_deref(), Some("https://github.com/Someone/zapper/releases"));
}

#[tokio::test]
async fn up_to_date_and_a_failed_check_leaves_the_state_it_found() {
    let r = rig(InstallKind::Deb, true, "0.3.0");
    assert_eq!(r.updates.check(1).await.unwrap().state, UpdateStateDto::UpToDate);
    *r.source.releases.lock().unwrap() = Err("GitHub answered HTTP 500".into());
    assert_eq!(r.updates.check(2).await.unwrap_err(), "GitHub answered HTTP 500");
    assert_eq!(r.updates.status().state, UpdateStateDto::UpToDate);
    assert_eq!(r.updates.status().last_checked, Some(1), "only an answered check counts");
}

#[tokio::test]
async fn a_build_from_source_is_offered_only_the_page() {
    let r = rig(InstallKind::Unpackaged, true, "0.2.0");
    let s = r.updates.check(1).await.unwrap();
    let UpdateStateDto::Available { release } = s.state else { panic!() };
    assert_eq!(release.asset, None);
    assert!(r.updates.start_download().unwrap_err().contains("release page"));
}

#[tokio::test]
async fn download_verify_install_restart() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    r.updates.check(1).await.unwrap();
    r.updates.start_download().unwrap();
    let state = settle(&r.updates).await;
    assert!(matches!(state, UpdateStateDto::Ready { .. }), "{state:?}");
    let file = r.dir.join("updates").join(deb_name());
    assert_eq!(std::fs::read(&file).unwrap(), b"hello");

    assert_eq!(r.updates.install().await.unwrap(), InstallOutcomeDto::Installed);
    let ran = r.runner.ran.lock().unwrap().clone();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].program, "pkexec");
    assert_eq!(ran[0].args[..3], ["/bin/sh".to_string(), "-c".into(), install::ROOT_INSTALL.into()]);
    assert_eq!(
        ran[0].args[4..],
        [&file.display().to_string(), HELLO, "deb", "apt", "install", "-y"],
        "root copies the verified file, checks it against the verified digest, and installs the copy"
    );
    assert!(matches!(r.updates.status().state, UpdateStateDto::Installed { .. }));

    r.updates.restart().unwrap();
    let spawned = r.runner.spawned.lock().unwrap().clone();
    assert_eq!(spawned[0].args.last().map(String::as_str), Some("/usr/bin/zapper"));
    assert_eq!(r.runner.quits.load(Ordering::SeqCst), 1);
    // A second press while the first relaunch waits for this process to exit.
    assert!(r.updates.restart().unwrap_err().contains("already restarting"));
    assert_eq!(r.runner.spawned.lock().unwrap().len(), 1, "one relaunch, not two");
}

#[tokio::test]
async fn two_presses_of_install_run_the_package_manager_once() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    r.updates.check(1).await.unwrap();
    r.updates.start_download().unwrap();
    settle(&r.updates).await;
    let (a, b) = tokio::join!(r.updates.install(), r.updates.install());
    assert_eq!(
        [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(),
        1,
        "one install, one refusal: {a:?} {b:?}"
    );
    assert_eq!(r.runner.ran.lock().unwrap().len(), 1);
}

/// A download cancelled while it waits for SHA256SUMS, replaced by a second
/// that finishes first: the first, finishing late, must leave the second's
/// verified file and its Ready state alone.
#[tokio::test]
async fn a_cancelled_download_finishing_late_leaves_the_next_one_alone() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    r.updates.check(1).await.unwrap();
    let gate = Arc::new(tokio::sync::Notify::new());
    *r.source.fetch_gate.lock().unwrap() = Some(gate.clone());
    r.updates.start_download().unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    r.updates.cancel_download();
    r.updates.start_download().unwrap();
    assert!(matches!(settle(&r.updates).await, UpdateStateDto::Ready { .. }));
    gate.notify_one();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(matches!(r.updates.status().state, UpdateStateDto::Ready { .. }));
    assert_eq!(std::fs::read(r.dir.join("updates").join(deb_name())).unwrap(), b"hello", "the second's file is still there");
    assert_eq!(r.updates.install().await.unwrap(), InstallOutcomeDto::Installed);
}

#[tokio::test]
async fn a_download_that_does_not_match_its_sum_is_deleted_and_never_installed() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    *r.source.sums.lock().unwrap() = format!("{}  {}\n", "0".repeat(64), deb_name());
    r.updates.check(1).await.unwrap();
    r.updates.start_download().unwrap();
    let UpdateStateDto::Failed { message, release } = settle(&r.updates).await else { panic!() };
    assert!(message.contains("doesn't match its checksum"), "{message}");
    assert!(release.is_some(), "the update is still on offer, to try again");
    assert_eq!(std::fs::read_dir(r.dir.join("updates")).unwrap().count(), 0, "nothing left behind");
    assert!(r.updates.install().await.is_err());
    assert!(r.runner.ran.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_package_the_sums_do_not_list_is_refused() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    *r.source.sums.lock().unwrap() = format!("{HELLO}  something-else.deb\n");
    r.updates.check(1).await.unwrap();
    r.updates.start_download().unwrap();
    let UpdateStateDto::Failed { message, .. } = settle(&r.updates).await else { panic!() };
    assert!(message.contains("doesn't list"), "{message}");
}

#[tokio::test]
async fn a_file_changed_after_the_check_is_not_installed() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    r.updates.check(1).await.unwrap();
    r.updates.start_download().unwrap();
    settle(&r.updates).await;
    std::fs::write(r.dir.join("updates").join(deb_name()), b"tampered").unwrap();
    assert!(r.updates.install().await.unwrap_err().contains("changed since it was checked"));
    assert!(r.runner.ran.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_cancelled_password_prompt_or_missing_pkexec_gives_the_command_to_run() {
    for (code, says) in [(Some(126), "permission"), (None, "pkexec isn't installed"), (Some(100), "exit 100")] {
        let r = rig(InstallKind::Deb, true, "0.2.0");
        *r.runner.code.lock().unwrap() = code;
        r.updates.check(1).await.unwrap();
        r.updates.start_download().unwrap();
        settle(&r.updates).await;
        let InstallOutcomeDto::Manual { command, reason, output } = r.updates.install().await.unwrap() else { panic!() };
        // The verified file by its absolute path, quoted for a shell if it
        // needs it (a Windows temp folder does; this test runs there too).
        let file = r.dir.join("updates").join(deb_name());
        assert_eq!(command, format!("sudo apt install {}", install::quote(&file.display().to_string())));
        assert!(reason.contains(says), "{reason}");
        assert_eq!(output.is_some(), code == Some(100), "output only when the package manager ran and failed");
        assert!(matches!(r.updates.status().state, UpdateStateDto::Ready { .. }), "still installable");
    }
}

#[tokio::test]
async fn windows_installers_start_and_zapper_quits() {
    let r = rig(InstallKind::Nsis, true, "0.2.0");
    let name = asset_name(InstallKind::Nsis, &Version::parse("0.3.0").unwrap()).unwrap();
    let mut rel = a_release_with_the_deb();
    rel.assets[0].name = name.clone();
    *r.source.releases.lock().unwrap() = Ok(vec![rel]);
    *r.source.sums.lock().unwrap() = format!("{HELLO}  {name}\n");
    r.updates.check(1).await.unwrap();
    r.updates.start_download().unwrap();
    settle(&r.updates).await;
    assert_eq!(r.updates.install().await.unwrap(), InstallOutcomeDto::Quitting);
    // Started from a fresh copy in a folder of its own, checked again just
    // before, not from the download itself.
    let program = PathBuf::from(&r.runner.spawned.lock().unwrap()[0].program);
    assert_eq!(program.file_name().unwrap().to_string_lossy(), name);
    let folder = program.parent().unwrap();
    assert_eq!(folder.parent().unwrap(), r.dir.join("updates"));
    assert!(folder.file_name().unwrap().to_string_lossy().starts_with("launch-"), "{}", folder.display());
    assert_eq!(std::fs::read(&program).unwrap(), b"hello");
    assert_eq!(r.runner.quits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_staged_installer_that_does_not_match_is_not_started() {
    let dir = std::env::temp_dir().join(format!("zapper-stage-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("zapper_0.3.0_x64-setup.exe");
    std::fs::write(&file, b"hello").unwrap();
    let staged = stage_for_launch(&dir, &file, HELLO).unwrap();
    assert_ne!(staged, file);
    assert_eq!(std::fs::read(&staged).unwrap(), b"hello");
    std::fs::write(&file, b"swapped").unwrap();
    assert!(stage_for_launch(&dir, &file, HELLO).is_err());
    // The next download's preparation clears every staged copy.
    prepare_dir(&dir).unwrap();
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn cancelling_offers_the_update_again() {
    let r = rig(InstallKind::Deb, true, "0.2.0");
    r.updates.check(1).await.unwrap();
    r.updates.start_download().unwrap();
    let s = r.updates.cancel_download();
    assert!(matches!(s.state, UpdateStateDto::Available { .. }), "{:?}", s.state);
}

#[test]
fn the_wire_shape_is_what_src_types_ts_expects() {
    let s = UpdateStatusDto {
        current: "0.2.0".into(),
        kind: "deb".into(),
        last_checked: None,
        state: UpdateStateDto::Downloading {
            release: UpdateReleaseDto {
                version: "0.3.0".into(),
                name: "zapper 0.3.0".into(),
                notes: String::new(),
                url: "u".into(),
                prerelease: false,
                published_at: None,
                asset: Some(UpdateAssetDto { name: "a".into(), size: 3 }),
            },
            received: 1,
            total: 3,
        },
    };
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(v["lastChecked"], serde_json::Value::Null);
    assert_eq!(v["state"]["state"], "downloading");
    assert_eq!(v["state"]["release"]["publishedAt"], serde_json::Value::Null);
    assert_eq!(v["state"]["received"], 1);
    let nc = serde_json::to_value(UpdateStateDto::NotConfigured).unwrap();
    assert_eq!(nc["state"], "notConfigured");
    let m = serde_json::to_value(InstallOutcomeDto::Manual { command: "c".into(), reason: "r".into(), output: None }).unwrap();
    assert_eq!(m["outcome"], "manual");
}
