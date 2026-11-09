//! When the recording service stops or restarts itself, so that neither
//! spoils a recording.
//!
//! Three things ask it to go:
//!
//! - `POST /api/shutdown` ([`ShutdownRequest`]): the app's updater, or an
//!   installer through `zapper-server --stop-for-setup`. `idle` waits for
//!   the first idle moment ([`super::schedule::busy`]); `now` goes at once.
//! - A signal (SIGTERM, Ctrl+C): a package being removed, the session
//!   ending. It goes at once.
//! - Its own program replaced on disk (a package upgrade on Linux, where a
//!   running program's file can be): it restarts into the new one at the
//!   first idle moment, with nobody asking.
//!
//! Going "at once" is never abrupt: the captures in progress close their
//! part and finish it into a playable file first, and pick up in a new part
//! if the service runs again while the programme is on (capture::suspend).
//!
//! Pure: the moment, the request and what is on disk are arguments.

use super::api::{ShutdownRequest, ShutdownWhen};

/// What the service does once it stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Stop,
    /// Runs its program again, with the same arguments.
    Restart,
}

/// Whether to go now, and how; None to carry on.
///
/// `signalled`: a signal came. `replaced`: the program on disk is not the
/// one running ([`replaced`]). `idle`: [`super::schedule::Busy::idle`].
pub fn decide(
    request: Option<ShutdownRequest>,
    signalled: bool,
    replaced: bool,
    idle: bool,
) -> Option<Exit> {
    let how = |restart: bool| if restart { Exit::Restart } else { Exit::Stop };
    if signalled {
        return Some(Exit::Stop);
    }
    match request {
        Some(r) if r.when == ShutdownWhen::Now => return Some(how(r.restart)),
        Some(r) if idle => return Some(how(r.restart)),
        _ => {}
    }
    (replaced && idle).then_some(Exit::Restart)
}

/// Two requests in a row: going now outranks waiting, and the later one
/// says whether to restart.
pub fn merge(old: Option<ShutdownRequest>, new: ShutdownRequest) -> ShutdownRequest {
    match old {
        Some(o) if o.when == ShutdownWhen::Now => ShutdownRequest {
            when: ShutdownWhen::Now,
            restart: new.restart,
        },
        _ => new,
    }
}

/// A file's identity: its device and inode (Unix), which a package manager
/// replacing it changes.
pub type FileId = (u64, u64);

/// Whether the program was replaced since the service started: there is a
/// file at its path again, and it is another one. A program that is gone
/// (a removal) is not replaced: there is nothing to restart into.
pub fn replaced(at_start: Option<FileId>, now: Option<FileId>) -> bool {
    matches!((at_start, now), (Some(a), Some(b)) if a != b)
}

/// The identity of the file at `path`, where the platform has one to read.
pub fn file_id(path: &std::path::Path) -> Option<FileId> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        // A running program's file cannot be replaced on Windows.
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::recording::api::RecordingState;
    use crate::recording::schedule::{busy, Slot, IDLE_MARGIN_SECS};

    const IDLE: ShutdownRequest = ShutdownRequest {
        when: ShutdownWhen::Idle,
        restart: false,
    };
    const NOW: ShutdownRequest = ShutdownRequest {
        when: ShutdownWhen::Now,
        restart: false,
    };

    #[test]
    fn idle_waits_and_now_does_not() {
        assert_eq!(decide(Some(IDLE), false, false, false), None);
        assert_eq!(decide(Some(IDLE), false, false, true), Some(Exit::Stop));
        assert_eq!(decide(Some(NOW), false, false, false), Some(Exit::Stop));
        let restart = ShutdownRequest { restart: true, ..IDLE };
        assert_eq!(decide(Some(restart), false, false, true), Some(Exit::Restart));
        assert_eq!(decide(None, false, false, true), None, "nobody asked");
    }

    #[test]
    fn a_signal_goes_at_once_and_does_not_come_back() {
        let restart = ShutdownRequest { restart: true, ..IDLE };
        assert_eq!(decide(Some(restart), true, true, false), Some(Exit::Stop));
    }

    #[test]
    fn a_replaced_program_restarts_once_idle() {
        assert_eq!(decide(None, false, true, false), None);
        assert_eq!(decide(None, false, true, true), Some(Exit::Restart));
        assert!(replaced(Some((1, 10)), Some((1, 11))));
        assert!(!replaced(Some((1, 10)), Some((1, 10))));
        assert!(!replaced(Some((1, 10)), None), "removed, not replaced");
        assert!(!replaced(None, Some((1, 10))), "never known");
    }

    #[test]
    fn now_outranks_idle_whatever_the_order() {
        let restart_idle = ShutdownRequest { restart: true, ..IDLE };
        assert_eq!(merge(Some(NOW), restart_idle), ShutdownRequest { when: ShutdownWhen::Now, restart: true });
        assert_eq!(merge(Some(IDLE), NOW), NOW);
        assert_eq!(merge(None, IDLE), IDLE);
    }

    /// The whole decision over an evening, on a clock that is only a number:
    /// a recording from 20:00 to 21:00 (padded), another at 21:03, a third at
    /// 23:00, and a request to restart once idle made at 19:59.
    #[test]
    fn an_idle_restart_waits_out_back_to_back_recordings() {
        let h = |hh: i64, mm: i64| hh * 3600 + mm * 60;
        let slots = |state1, state2| {
            vec![
                Slot { id: 1, state: state1, from: h(20, 0), until: h(21, 0) },
                Slot { id: 2, state: state2, from: h(21, 3), until: h(22, 0) },
                Slot { id: 3, state: RecordingState::Scheduled, from: h(23, 0), until: h(23, 30) },
            ]
        };
        let req = Some(ShutdownRequest { when: ShutdownWhen::Idle, restart: true });
        let at = |t: i64, s: &[Slot], active: &[i64]| {
            let b = busy(t, s, &active.iter().copied().collect::<HashSet<_>>());
            (decide(req, false, false, b.idle), b.idle_at)
        };
        use RecordingState::*;
        // 19:56: within the margin of the 20:00 start; busy until 22:00,
        // since 21:03 follows 21:00 too closely to stop between.
        assert!(h(20, 0) - h(19, 56) < IDLE_MARGIN_SECS);
        assert_eq!(at(h(19, 56), &slots(Scheduled, Scheduled), &[]), (None, Some(h(22, 0))));
        assert_eq!(at(h(20, 30), &slots(Recording, Scheduled), &[1]), (None, Some(h(22, 0))));
        assert_eq!(at(h(21, 30), &slots(Done, Recording), &[2]), (None, Some(h(22, 0))));
        // 22:00: both done, 23:00 an hour off: restart now.
        assert_eq!(at(h(22, 0), &slots(Done, Done), &[]), (Some(Exit::Restart), None));
    }
}
