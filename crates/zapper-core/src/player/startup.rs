//! What a front end does once, as it opens, after it has installed its
//! player ([`super::install`]): the saved settings applied to the player,
//! the controller started on the front end's runtime, the catalogue kept in
//! sync (and told when the recording service or a browser changes the
//! account or the catalogue), and the recording service started again if an
//! update stopped it. One function, so that every front end opens the same
//! way.

use std::sync::atomic::{AtomicBool, Ordering};

/// What [`startup`] does beyond the player itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Startup {
    /// Sync the catalogue when it is due and on the interval
    /// (`catalog::sync_engine::spawn_periodic`), and hear of the account or
    /// the catalogue changing elsewhere
    /// (`catalog::sync_engine::spawn_watch_elsewhere`). Off for a check
    /// that brings its own catalogue.
    pub sync: bool,
    /// Start the installed recording service when an update stopped it
    /// (`recording::service::resume`; Windows, a no-op elsewhere). Off for
    /// an unattended run, whose scratch home is not the service's.
    pub resume_service: bool,
}

impl Startup {
    /// Everything: an app opening for a person.
    pub const APP: Startup = Startup { sync: true, resume_service: true };
    /// The player alone: a harness.
    pub const PLAYER_ONLY: Startup = Startup { sync: false, resume_service: false };
}

/// Opens the core for a front end, on `rt`. Once per process: returns
/// whether this call did it, and a later one does nothing.
pub fn startup(rt: &tokio::runtime::Handle, what: Startup) -> bool {
    static STARTED: AtomicBool = AtomicBool::new(false);
    run(&STARTED, &Real(rt), what)
}

/// The steps, apart from their order and the once-only, so that a test can
/// watch them.
trait Steps {
    fn apply_saved_settings(&self);
    fn start_player(&self);
    fn spawn_sync(&self);
    fn spawn_watch_elsewhere(&self);
    fn resume_service(&self);
}

fn run(started: &AtomicBool, steps: &dyn Steps, what: Startup) -> bool {
    if started.swap(true, Ordering::AcqRel) {
        return false;
    }
    // Settings that only take effect when changed would silently reset
    // every launch.
    steps.apply_saved_settings();
    // The controller's tasks run on the front end's runtime, and its
    // heartbeat starts.
    steps.start_player();
    if what.sync {
        steps.spawn_sync();
        // The recording service on this computer shares the account file
        // and the catalogue, and a browser signed in to it can change
        // either: the front end hears of both from here.
        steps.spawn_watch_elsewhere();
    }
    if what.resume_service {
        steps.resume_service();
    }
    true
}

struct Real<'a>(&'a tokio::runtime::Handle);

impl Steps for Real<'_> {
    fn apply_saved_settings(&self) {
        super::controller::apply_saved_settings();
    }

    fn start_player(&self) {
        super::start(self.0);
    }

    fn spawn_sync(&self) {
        let _on = self.0.enter();
        crate::catalog::sync_engine::spawn_periodic();
    }

    fn spawn_watch_elsewhere(&self) {
        let _on = self.0.enter();
        crate::catalog::sync_engine::spawn_watch_elsewhere(super::controller::forget_source);
    }

    fn resume_service(&self) {
        // It waits on the task scheduler and the service manager: off the
        // front end's thread.
        std::thread::spawn(crate::recording::service::resume);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Log(Mutex<Vec<&'static str>>);

    impl Log {
        fn push(&self, step: &'static str) {
            self.0.lock().unwrap().push(step);
        }
        fn taken(&self) -> Vec<&'static str> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Steps for Log {
        fn apply_saved_settings(&self) {
            self.push("settings");
        }
        fn start_player(&self) {
            self.push("player");
        }
        fn spawn_sync(&self) {
            self.push("sync");
        }
        fn spawn_watch_elsewhere(&self) {
            self.push("elsewhere");
        }
        fn resume_service(&self) {
            self.push("service");
        }
    }

    #[test]
    fn startup_runs_once_and_in_order() {
        let (started, log) = (AtomicBool::new(false), Log::default());
        assert!(run(&started, &log, Startup::APP));
        assert_eq!(log.taken(), ["settings", "player", "sync", "elsewhere", "service"]);
        // A second front end, or the same one twice: nothing again.
        assert!(!run(&started, &log, Startup::APP));
        assert!(!run(&started, &log, Startup::PLAYER_ONLY));
        assert!(log.taken().is_empty());
    }

    #[test]
    fn startup_leaves_out_what_it_is_not_asked_for() {
        let (started, log) = (AtomicBool::new(false), Log::default());
        assert!(run(&started, &log, Startup::PLAYER_ONLY));
        assert_eq!(log.taken(), ["settings", "player"]);
        let (started, log) = (AtomicBool::new(false), Log::default());
        assert!(run(&started, &log, Startup { sync: false, resume_service: true }));
        assert_eq!(log.taken(), ["settings", "player", "service"]);
    }
}
