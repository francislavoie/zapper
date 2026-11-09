//! What a front end does as it quits, [`startup`](super::startup)'s other
//! half: where a movie or an episode got to written now, the player taken
//! out of reach of every caller, and playback stopped, in a bounded time.
//!
//! Bounded because the stop is a call into the player, which a wedged
//! decoder or network read can hold: a quit that hangs is worse than one
//! that skips the tidying (the old app's quit had a watchdog for the same
//! reason). The progress is written first, and synchronously, because it is
//! the one thing a quit must not lose.

use std::sync::mpsc;
use std::time::Duration;

/// The longest [`shutdown`] waits for the player to stop.
pub const STOP_BOUND: Duration = Duration::from_secs(2);

/// Takes the player down for quitting. Returns whether it stopped within
/// [`STOP_BOUND`]; either way the backend is no longer installed when this
/// returns, and the controller says Stopped.
///
/// In order: progress saved ([`super::controller::save_progress_now`]); the
/// title previews' worker told to end and the trickplay store cleared; the
/// backend uninstalled, so no command, heartbeat or worker reaches it from
/// here on, and the controller's state committed as stopped, so nothing reads
/// the player as playing or holding a connection; then the player, held from
/// before it was uninstalled, told to stop, on a thread of its own, waited
/// for at most [`STOP_BOUND`].
pub fn shutdown() -> bool {
    shutdown_within(STOP_BOUND)
}

fn shutdown_within(bound: Duration) -> bool {
    super::controller::save_progress_now();
    super::title_thumbs::stop();
    super::trickplay::clear();
    let player = super::backend();
    super::uninstall();
    super::controller::commit(|m| m.stopped());
    let Some(player) = player else {
        return true;
    };
    let (done, stopped) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("zapper-player-stop".into())
        .spawn(move || {
            if let Err(e) = player.command("stop", &[]) {
                tracing::debug!("stopping the player at quit: {e:#}");
            }
            let _ = done.send(());
        });
    if let Err(e) = spawned {
        tracing::warn!("could not stop the player at quit: {e}");
        return false;
    }
    match stopped.recv_timeout(bound) {
        Ok(()) => true,
        Err(_) => {
            tracing::warn!("the player did not stop within {bound:?}; quitting anyway");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::fake::FakeBackend;
    use crate::player::{self, BackendCaps, Health, PlayerBackend, Probe, Signal, Stats};
    use std::path::Path;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Instant;

    fn registry() -> std::sync::MutexGuard<'static, ()> {
        crate::player::backend::TEST_REGISTRY
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A [`FakeBackend`] that also notes, when told to stop, whether it was
    /// still reachable through `player::backend()`, and can be made to hang
    /// there until released.
    struct Watched {
        fake: FakeBackend,
        installed_at_stop: Mutex<Option<bool>>,
        hang: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl PlayerBackend for Watched {
        fn capabilities(&self) -> BackendCaps {
            self.fake.capabilities()
        }
        fn load(&self, url: &str) -> anyhow::Result<()> {
            self.fake.load(url)
        }
        fn set_property(&self, key: &str, value: &str) -> anyhow::Result<()> {
            self.fake.set_property(key, value)
        }
        fn command(&self, name: &str, args: &[&str]) -> anyhow::Result<()> {
            if name == "stop" {
                *self.installed_at_stop.lock().unwrap() = Some(player::backend().is_some());
                if let Some(release) = self.hang.lock().unwrap().take() {
                    let _ = release.recv();
                }
            }
            self.fake.command(name, args)
        }
        fn set_flag(&self, key: &str, value: bool) -> anyhow::Result<()> {
            self.fake.set_flag(key, value)
        }
        fn set_f64(&self, key: &str, value: f64) -> anyhow::Result<()> {
            self.fake.set_f64(key, value)
        }
        fn get_f64(&self, key: &str) -> Option<f64> {
            self.fake.get_f64(key)
        }
        fn get_i64(&self, key: &str) -> Option<i64> {
            self.fake.get_i64(key)
        }
        fn get_bool(&self, key: &str) -> Option<bool> {
            self.fake.get_bool(key)
        }
        fn get_string(&self, key: &str) -> Option<String> {
            self.fake.get_string(key)
        }
        fn pump(&self) -> Vec<Signal> {
            self.fake.pump()
        }
        fn stats(&self) -> Stats {
            self.fake.stats()
        }
        fn reading(&self) -> crate::catalog::store::Reading {
            self.fake.reading()
        }
        fn health(&self) -> Health {
            self.fake.health()
        }
        fn probe(&self) -> Probe {
            self.fake.probe()
        }
        fn dump_cache(&self, start: f64, end: f64, path: &Path) -> anyhow::Result<()> {
            self.fake.dump_cache(start, end, path)
        }
    }

    fn watched(hang: Option<mpsc::Receiver<()>>) -> Arc<Watched> {
        Arc::new(Watched {
            fake: FakeBackend::new(),
            installed_at_stop: Mutex::new(None),
            hang: Mutex::new(hang),
        })
    }

    #[test]
    fn shutdown_uninstalls_the_player_before_stopping_it() {
        let _registry = registry();
        crate::catalog::db::install_test_db();
        let b = watched(None);
        player::install(b.clone());
        assert!(shutdown());
        assert!(player::backend().is_none(), "uninstalled");
        assert_eq!(*b.installed_at_stop.lock().unwrap(), Some(false), "stopped after it");
        assert!(b.fake.calls().iter().any(|c| c.starts_with("command stop")), "{:?}", b.fake.calls());
        let state = super::super::controller::STATE.lock().unwrap().dto.state;
        assert_eq!(state, crate::api::types::PlaybackState::Stopped);
        // Nothing installed: nothing to stop, and nothing to wait for.
        assert!(shutdown());
    }

    #[test]
    fn shutdown_gives_up_on_a_player_that_does_not_stop() {
        let _registry = registry();
        crate::catalog::db::install_test_db();
        let (release, hang) = mpsc::channel();
        let b = watched(Some(hang));
        player::install(b.clone());
        let began = Instant::now();
        assert!(!shutdown_within(Duration::from_millis(200)));
        assert!(began.elapsed() < Duration::from_secs(2), "{:?}", began.elapsed());
        assert!(player::backend().is_none());
        let _ = release.send(());
    }
}
