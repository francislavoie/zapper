//! What the service should do at a given moment.
//!
//! The service keeps its recordings in SQLite and, each time it wakes, asks
//! [`plan`] what is due: captures to start (a scheduled recording whose
//! padded start has come; one that was mid-capture when the service stopped;
//! a start that was missed while the service was down, as long as the
//! programme is still on), captures to give up on (their whole window passed
//! with the service off), and when to wake next. Captures stop themselves at
//! the end of their window, so stopping is not planned here.
//!
//! Pure: the clock is an argument.

use std::collections::HashSet;

use super::api::RecordingState;

/// What [`plan`] needs to know about a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub id: i64,
    pub state: RecordingState,
    /// Unix seconds, padding included: `[from, until)`.
    pub from: i64,
    pub until: i64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Begin capturing these now.
    pub start: Vec<i64>,
    /// Their window passed without a capture: the service was not running.
    pub missed: Vec<i64>,
    /// Captured while the service ran, whose window has since passed with
    /// no capture running (it stopped mid-recording): finish them with what
    /// they have.
    pub finish: Vec<i64>,
    /// When the next start is due, if any is scheduled.
    pub next: Option<i64>,
}

/// The longest the service sleeps without looking again.
///
/// Its timers run on a monotonic clock, which a suspended machine does not
/// advance, and the wall clock may be set; looking at least this often keeps
/// a recording from starting minutes late after either.
pub const MAX_SLEEP_SECS: i64 = 30;

/// What is due at `now`, given which recordings are being captured.
pub fn plan(now: i64, slots: &[Slot], active: &HashSet<i64>) -> Plan {
    let mut p = Plan::default();
    for s in slots {
        if active.contains(&s.id) {
            continue;
        }
        match s.state {
            RecordingState::Scheduled if now >= s.until => p.missed.push(s.id),
            RecordingState::Scheduled if now >= s.from => p.start.push(s.id),
            RecordingState::Scheduled => {
                p.next = Some(p.next.map_or(s.from, |n: i64| n.min(s.from)));
            }
            // Mid-capture when the service stopped: carry on in a new part
            // while there is still something to record.
            RecordingState::Recording if now < s.until => p.start.push(s.id),
            RecordingState::Recording => p.finish.push(s.id),
            // Being finished when the service stopped: finish it now. It
            // was stopped, or its window had passed, so it never resumes.
            RecordingState::Finishing => p.finish.push(s.id),
            RecordingState::Done | RecordingState::Failed => {}
        }
    }
    p
}

/// How long to sleep before looking again.
pub fn sleep_for(now: i64, next: Option<i64>) -> i64 {
    next.map_or(MAX_SLEEP_SECS, |n| (n - now).clamp(0, MAX_SLEEP_SECS))
}

/// How close a recording's padded start may be for the service still to
/// count as idle: enough for a restart, or an installer, to be done first.
pub const IDLE_MARGIN_SECS: i64 = 5 * 60;

/// Whether the service can be stopped without spoiling a recording, and if
/// not, when it can.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Busy {
    /// Nothing is being recorded, and nothing starts within
    /// [`IDLE_MARGIN_SECS`].
    pub idle: bool,
    /// When it next will be: the end of what is recording, and of whatever
    /// starts too soon after that to wait for. None when idle now.
    pub idle_at: Option<i64>,
    /// The next padded start of a recording not yet begun.
    pub next_start: Option<i64>,
}

/// What [`Busy`] is at `now`. A recording counts as being recorded when it
/// is captured or finished (`active`) — finishing into its files outlasts
/// its window, and stopping then would leave no playable file — when it is
/// finishing, and when it was mid-capture and its window is still open: the
/// service resumes those at once.
pub fn busy(now: i64, slots: &[Slot], active: &HashSet<i64>) -> Busy {
    let live = |s: &Slot| {
        active.contains(&s.id)
            || s.state == RecordingState::Finishing
            || (s.until > now && s.state == RecordingState::Recording && s.from <= now)
    };
    let pending = |s: &Slot| s.state == RecordingState::Scheduled && !active.contains(&s.id) && s.until > now;
    let next_start = slots.iter().filter(|s| pending(s)).map(|s| s.from).min();
    // From the end of what records now, take in every recording that
    // starts within the margin of that end, until one leaves a gap.
    // A finish past its window ends at no time anyone can say: as soon as
    // it can, `now`.
    let mut at = slots.iter().filter(|s| live(s)).map(|s| s.until.max(now)).max();
    let mut t = at.unwrap_or(now);
    loop {
        let soon = slots
            .iter()
            .filter(|s| pending(s) && s.from <= t + IDLE_MARGIN_SECS && s.until > t)
            .map(|s| s.until)
            .max();
        match soon {
            Some(u) => {
                t = u;
                at = Some(u);
            }
            None => break,
        }
    }
    Busy {
        idle: at.is_none(),
        idle_at: at,
        next_start,
    }
}

/// A recording's window, padding included.
pub fn window(start: i64, stop: i64, pad_before: i64, pad_after: i64) -> (i64, i64) {
    (start - pad_before.max(0), stop + pad_after.max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use RecordingState::*;

    fn slot(id: i64, state: RecordingState, from: i64, until: i64) -> Slot {
        Slot {
            id,
            state,
            from,
            until,
        }
    }

    #[test]
    fn a_recording_starts_when_its_padded_start_comes() {
        let slots = [slot(1, Scheduled, 1000, 2000)];
        let none = HashSet::new();
        assert_eq!(
            plan(999, &slots, &none),
            Plan {
                next: Some(1000),
                ..Default::default()
            }
        );
        assert_eq!(plan(1000, &slots, &none).start, [1]);
    }

    #[test]
    fn a_missed_start_begins_at_once_while_the_programme_is_on() {
        let slots = [slot(1, Scheduled, 1000, 2000)];
        assert_eq!(plan(1500, &slots, &HashSet::new()).start, [1]);
    }

    #[test]
    fn a_window_that_passed_entirely_is_missed() {
        let slots = [slot(1, Scheduled, 1000, 2000)];
        let p = plan(2000, &slots, &HashSet::new());
        assert_eq!((p.start.len(), p.missed.as_slice()), (0, &[1][..]));
    }

    #[test]
    fn a_capture_interrupted_by_a_restart_resumes_or_finishes() {
        let slots = [slot(1, Recording, 1000, 2000), slot(2, Recording, 0, 500)];
        let p = plan(1200, &slots, &HashSet::new());
        assert_eq!(p.start, [1]);
        assert_eq!(p.finish, [2]);
    }

    #[test]
    fn running_captures_and_finished_recordings_are_left_alone() {
        let slots = [
            slot(1, Recording, 1000, 2000),
            slot(2, Done, 0, 10),
            slot(3, Failed, 0, 10),
            slot(4, Scheduled, 3000, 4000),
            slot(5, Scheduled, 2500, 4000),
        ];
        let p = plan(1200, &slots, &HashSet::from([1]));
        assert_eq!(
            p,
            Plan {
                next: Some(2500),
                ..Default::default()
            }
        );
    }

    #[test]
    fn idle_is_nothing_recording_and_nothing_about_to() {
        let none = HashSet::new();
        let slots = [slot(1, Scheduled, 10_000, 12_000)];
        let b = busy(1_000, &slots, &none);
        assert_eq!((b.idle, b.idle_at, b.next_start), (true, None, Some(10_000)));
        // Within the margin of its start, it is as good as recording.
        let b = busy(10_000 - IDLE_MARGIN_SECS, &slots, &none);
        assert_eq!((b.idle, b.idle_at), (false, Some(12_000)));
        assert!(busy(9_000 - IDLE_MARGIN_SECS, &slots, &none).idle);
        // Recording: busy until its padded end, then idle.
        let active = HashSet::from([1]);
        assert_eq!(busy(11_000, &slots, &active).idle_at, Some(12_000));
        assert!(busy(12_000, &[slot(1, Done, 10_000, 12_000)], &none).idle);
    }

    #[test]
    fn a_capture_that_will_resume_counts_as_recording() {
        // Mid-capture when the service stopped, window still open: it starts
        // again at the next look, so stopping now would spoil it.
        let b = busy(1_500, &[slot(1, Recording, 1_000, 2_000)], &HashSet::new());
        assert_eq!((b.idle, b.idle_at), (false, Some(2_000)));
        // Its window over, it is only finished, which a restart also does.
        assert!(busy(2_000, &[slot(1, Recording, 1_000, 2_000)], &HashSet::new()).idle);
    }

    #[test]
    fn a_recording_being_finished_is_busy_and_is_finished_after_a_restart() {
        // Its window over and its capture closed, it is still being turned
        // into its files: busy, idle "now" at the earliest.
        let finishing = [slot(1, Finishing, 1_000, 2_000)];
        let b = busy(2_500, &finishing, &HashSet::from([1]));
        assert_eq!((b.idle, b.idle_at), (false, Some(2_500)));
        // Without the capture task (a restart in between), likewise.
        assert!(!busy(2_500, &finishing, &HashSet::new()).idle);
        // Stopped early, inside its window: busy until it would have ended,
        // or until it is finished, whichever the service sees first.
        let b = busy(1_500, &finishing, &HashSet::from([1]));
        assert_eq!((b.idle, b.idle_at), (false, Some(2_000)));
        // A capture still registered past its window is being finished too.
        let b = busy(2_500, &[slot(1, Recording, 1_000, 2_000)], &HashSet::from([1]));
        assert!(!b.idle);

        // Not running when the service starts: finished, never resumed.
        let p = plan(1_500, &finishing, &HashSet::new());
        assert_eq!((p.start.len(), p.finish.as_slice()), (0, &[1][..]));
        assert_eq!(plan(1_500, &finishing, &HashSet::from([1])), Plan::default());
    }

    #[test]
    fn back_to_back_recordings_are_waited_out_together() {
        let active = HashSet::from([1]);
        let slots = [
            slot(1, Recording, 1_000, 2_000),
            // Starts within the margin of 1's end: no time to stop between.
            slot(2, Scheduled, 2_000 + IDLE_MARGIN_SECS - 1, 4_000),
            // Starts well after 2's end: a gap to stop in.
            slot(3, Scheduled, 4_000 + IDLE_MARGIN_SECS + 1, 9_000),
        ];
        let b = busy(1_500, &slots, &active);
        assert_eq!((b.idle, b.idle_at), (false, Some(4_000)));
        assert_eq!(b.next_start, Some(2_000 + IDLE_MARGIN_SECS - 1));
    }

    #[test]
    fn sleeps_are_short_enough_to_notice_a_suspend() {
        assert_eq!(sleep_for(0, None), MAX_SLEEP_SECS);
        assert_eq!(sleep_for(0, Some(10)), 10);
        assert_eq!(sleep_for(0, Some(10_000)), MAX_SLEEP_SECS);
        assert_eq!(sleep_for(50, Some(10)), 0);
        assert_eq!(window(1000, 2000, 60, 300), (940, 2300));
        assert_eq!(window(1000, 2000, -5, -5), (1000, 2000));
    }
}
