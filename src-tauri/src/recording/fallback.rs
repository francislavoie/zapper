//! When a recording moves between its main and backup feeds.
//!
//! The rules, in order:
//!
//! - **Only a failure moves it.** A feed that is delivering is kept, however
//!   the other one might be doing: sticky, never back "home".
//! - **A connection-limit refusal is the account's, not the feed's.** The
//!   provider said the account has no connection to spare (a 509 or 429, a
//!   refusal that says so in words, or a `200` carrying a line of text
//!   instead of video), so the backup — on the same account — would be told
//!   the same. Nothing switches; the same feed is asked again after a long
//!   and growing wait (one minute, doubling to ten), never in a tight loop.
//!   A recording does take one of the account's connections while it runs;
//!   it does not fight for one.
//! - **Anything else switches,** to the other feed if there is one: no data
//!   for ten seconds, a refusal, the stream ending, the server unreachable.
//! - **Flap guard.** After three switches in ten minutes a fourth is not
//!   made: the recording goes back to the last feed that delivered and stays
//!   on it, retrying with back-off, until the ten minutes have passed.
//! - **Back-off** for a feed that keeps failing: 5 s, doubling to a minute.
//!   It starts over once a connection has delivered for 30 s.
//!
//! Pure, with the clock passed in, so every rule is a unit test.

use std::collections::VecDeque;

/// How a connection to a feed ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Connected, then nothing for [`STALL_SECS`].
    Stalled,
    /// The response finished, as when the provider drops a stream.
    Ended,
    /// An HTTP status other than success. `limit`: the provider said in
    /// words it was a connection limit.
    Refused { status: u16, limit: bool },
    /// A success status with something other than a transport stream.
    NotVideo,
    /// The server could not be reached, or the connection broke.
    Unreachable,
    /// The channel is not on the air: the provider answered with a
    /// placeholder clip this many seconds long (`hub`'s "Off air").
    OffAir { clip_secs: u32 },
}

impl Failure {
    /// The account has no connection to spare: see the module's rules.
    pub fn is_limit(self) -> bool {
        match self {
            Failure::Refused { status, limit } => limit || matches!(status, 429 | 458 | 509),
            Failure::NotVideo => true,
            _ => false,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Failure::Stalled => format!("no data for {STALL_SECS} s"),
            Failure::Ended => "the provider ended the stream".into(),
            Failure::Refused { status, .. } if self.is_limit() => {
                format!("refused ({status}): the account's connection limit")
            }
            Failure::Refused { status, .. } => format!("refused ({status})"),
            Failure::NotVideo => "the provider answered with something other than video".into(),
            Failure::Unreachable => "the server could not be reached".into(),
            Failure::OffAir { .. } => {
                "the channel is off air: the provider sent a placeholder instead".into()
            }
        }
    }
}

/// How long a recording whose channel is off air waits before asking again,
/// so that it picks the broadcast up if it starts. Each try costs one of the
/// account's connections for a few seconds; one every three minutes misses
/// at most that much of a start.
pub const OFF_AIR_RETRY_SECS: i64 = 180;

/// Seconds without data before a connection counts as failed.
pub const STALL_SECS: i64 = 10;
/// A connection that delivered this long has proved the feed works.
pub const PROVEN_SECS: i64 = 30;
/// The flap guard's window and the switches allowed in it.
pub const FLAP_WINDOW_SECS: i64 = 10 * 60;
pub const FLAP_MAX_SWITCHES: usize = 3;

/// What to do next: connect to `feed` (an index into the feeds) at `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Next {
    pub feed: usize,
    pub at: i64,
    /// Worth keeping on the recording: a switch, a limit, the flap guard.
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Fallback {
    feeds: usize,
    current: usize,
    last_good: Option<usize>,
    /// When each recent switch happened, oldest first.
    switches: VecDeque<i64>,
    /// Consecutive failures per feed, for its back-off, and when each feed
    /// last failed: a failure older than the flap window is forgotten.
    failures: [u32; 2],
    last_failed: [i64; 2],
    /// Consecutive connection-limit refusals, for theirs.
    limits: u32,
    /// Whether the flap guard's note has been given for the current spell.
    guarded: bool,
    /// When each feed was last found off air, until it delivers again.
    off_air: [Option<i64>; 2],
    /// How long to wait before asking an off-air feed again.
    off_air_retry: i64,
}

impl Fallback {
    /// `feeds` is 1 (no backup) or 2 (main, then backup).
    pub fn new(feeds: usize) -> Self {
        assert!(
            (1..=2).contains(&feeds),
            "a main feed and at most one backup"
        );
        Self {
            feeds,
            current: 0,
            last_good: None,
            switches: VecDeque::new(),
            failures: [0; 2],
            last_failed: [i64::MIN; 2],
            limits: 0,
            guarded: false,
            off_air: [None; 2],
            off_air_retry: OFF_AIR_RETRY_SECS,
        }
    }

    /// Asks an off-air feed again after `secs` rather than
    /// [`OFF_AIR_RETRY_SECS`]: for a harness that cannot wait minutes.
    pub fn with_off_air_retry(mut self, secs: i64) -> Self {
        self.off_air_retry = secs.max(1);
        self
    }

    pub fn current(&self) -> usize {
        self.current
    }

    /// The backup feed was changed, added or taken away while recording.
    /// The new one starts with a clean slate (its failures were another
    /// channel's), and with no backup left the main feed is the only one.
    /// What the recording does next is decided as before: this changes
    /// which channel "the backup" is, not whether to switch to it.
    pub fn backup_changed(&mut self, present: bool) {
        self.feeds = if present { 2 } else { 1 };
        if self.current >= self.feeds {
            self.current = 0;
        }
        if self.last_good == Some(1) {
            self.last_good = None;
        }
        self.failures[1] = 0;
        self.last_failed[1] = i64::MIN;
        self.off_air[1] = None;
    }

    /// Rides a connection the service already has open on `feed` (another
    /// recording's): it is the current feed from now on. Nothing failed, so
    /// it is not a switch and the flap guard does not count it.
    pub fn ride(&mut self, feed: usize) {
        if feed < self.feeds {
            self.current = feed;
        }
    }

    /// The connection to the current feed ended at `now` after delivering
    /// data for `delivered_secs` (0: none at all).
    pub fn failed(&mut self, now: i64, why: Failure, delivered_secs: i64) -> Next {
        if delivered_secs > 0 {
            self.last_good = Some(self.current);
            self.limits = 0;
            self.off_air[self.current] = None;
        }
        if delivered_secs >= PROVEN_SECS {
            self.failures[self.current] = 0;
        }

        if let Failure::OffAir { .. } = why {
            return self.off_air(now, why);
        }

        if why.is_limit() {
            self.limits += 1;
            let wait = limit_backoff(self.limits);
            return Next {
                feed: self.current,
                at: now + wait,
                note: Some(format!(
                    "{}; asking again in {}",
                    why.describe(),
                    words(wait)
                )),
            };
        }

        for feed in 0..self.feeds {
            if self.last_failed[feed] <= now - FLAP_WINDOW_SECS {
                self.failures[feed] = 0;
            }
        }
        self.failures[self.current] = self.failures[self.current].saturating_add(1);
        self.last_failed[self.current] = now;
        while self
            .switches
            .front()
            .is_some_and(|&t| t <= now - FLAP_WINDOW_SECS)
        {
            self.switches.pop_front();
        }
        if self.switches.is_empty() {
            self.guarded = false;
        }

        if self.feeds == 1 {
            let wait = backoff(self.failures[0]);
            return Next {
                feed: 0,
                at: now + wait,
                note: Some(format!(
                    "{}; reconnecting in {}",
                    why.describe(),
                    words(wait)
                )),
            };
        }

        if self.switches.len() >= FLAP_MAX_SWITCHES {
            // Flapping: settle on the feed that last worked and wait it out.
            let stay = self.last_good.unwrap_or(self.current);
            self.current = stay;
            let wait = backoff(self.failures[stay].max(1));
            let note = (!self.guarded).then(|| {
                format!(
                    "the feeds kept failing ({} switches in {} minutes); staying on the {} feed and retrying with back-off",
                    self.switches.len(),
                    FLAP_WINDOW_SECS / 60,
                    name(stay)
                )
            });
            self.guarded = true;
            return Next {
                feed: stay,
                at: now + wait,
                note,
            };
        }

        let from = self.current;
        let to = 1 - from;
        self.current = to;
        self.switches.push_back(now);
        // A feed that failed recently waits out its own back-off first.
        let wait = if self.failures[to] == 0 {
            0
        } else {
            backoff(self.failures[to])
        };
        Next {
            feed: to,
            at: now + wait,
            note: Some(format!(
                "switched from the {} feed to the {}: {}",
                name(from),
                name(to),
                why.describe()
            )),
        }
    }
}

impl Fallback {
    /// The current feed is off air. That is no fault of the feed's: nothing
    /// is counted against it and the flap guard does not see it. The other
    /// feed is tried at once, unless it too was found off air within the
    /// retry's wait; with nothing else to try, the feed found off air
    /// longest ago is asked again after the wait ([`OFF_AIR_RETRY_SECS`]).
    /// The caller stops asking at its programme's end.
    fn off_air(&mut self, now: i64, why: Failure) -> Next {
        let from = self.current;
        self.off_air[from] = Some(now);
        if self.feeds == 2 {
            let to = 1 - from;
            if self.off_air[to].is_none_or(|t| t <= now - self.off_air_retry) {
                self.current = to;
                return Next {
                    feed: to,
                    at: now,
                    note: Some(format!(
                        "switched from the {} feed to the {}: {}",
                        name(from),
                        name(to),
                        why.describe()
                    )),
                };
            }
            self.current = if self.off_air[0] <= self.off_air[1] { 0 } else { 1 };
        }
        Next {
            feed: self.current,
            at: now + self.off_air_retry,
            note: None,
        }
    }
}

fn name(feed: usize) -> &'static str {
    if feed == 0 {
        "main"
    } else {
        "backup"
    }
}

/// 5 s after the first failure, doubling to a minute.
pub fn backoff(failures: u32) -> i64 {
    let shift = failures.saturating_sub(1).min(8);
    (5i64 << shift).min(60)
}

/// A minute after the first limit refusal, doubling to ten.
pub fn limit_backoff(refusals: u32) -> i64 {
    let shift = refusals.saturating_sub(1).min(8);
    (60i64 << shift).min(600)
}

fn words(secs: i64) -> String {
    if secs >= 60 && secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFUSED: Failure = Failure::Refused {
        status: 503,
        limit: false,
    };

    #[test]
    fn a_failure_switches_to_the_backup_at_once() {
        let mut f = Fallback::new(2);
        let n = f.failed(100, Failure::Stalled, 40);
        assert_eq!((n.feed, n.at), (1, 100));
        assert!(n
            .note
            .unwrap()
            .contains("switched from the main feed to the backup"));
    }

    #[test]
    fn a_working_feed_is_kept_and_never_switched_back() {
        let mut f = Fallback::new(2);
        f.failed(100, Failure::Ended, 40);
        assert_eq!(f.current(), 1);
        // Nothing but a failure moves it: there is no "main is back" event to
        // react to, by construction. The next failure goes to main because it
        // is the other feed, not because it is home.
        let n = f.failed(5000, Failure::Ended, 4000);
        assert_eq!((n.feed, n.at), (0, 5000));
    }

    #[test]
    fn a_connection_limit_never_switches_and_waits_longer_each_time() {
        let mut f = Fallback::new(2);
        let limit = Failure::Refused {
            status: 509,
            limit: false,
        };
        let n = f.failed(0, limit, 0);
        assert_eq!((n.feed, n.at), (0, 60));
        assert!(n
            .note
            .unwrap()
            .contains("connection limit; asking again in 1 min"));
        assert_eq!(f.failed(60, limit, 0).at, 60 + 120);
        assert_eq!(f.failed(180, Failure::NotVideo, 0).at, 180 + 240);
        assert_eq!(
            f.failed(
                420,
                Failure::Refused {
                    status: 403,
                    limit: true
                },
                0
            )
            .at,
            420 + 480
        );
        assert_eq!(
            f.failed(900, limit, 0).at,
            900 + 600,
            "capped at ten minutes"
        );
        assert_eq!(f.current(), 0, "the backup is on the same account");
        // Delivering again starts the limit's back-off over.
        f.failed(5000, Failure::Ended, 100);
        assert_eq!(f.failed(5001, limit, 0).at, 5001 + 60);
    }

    #[test]
    fn a_plain_refusal_is_a_feed_failure() {
        let mut f = Fallback::new(2);
        assert_eq!(f.failed(0, REFUSED, 0).feed, 1);
        assert!(!REFUSED.is_limit());
        assert!(Failure::Refused {
            status: 429,
            limit: false
        }
        .is_limit());
    }

    #[test]
    fn without_a_backup_the_feed_is_retried_with_back_off() {
        let mut f = Fallback::new(1);
        let waits: Vec<i64> = (0..6)
            .map(|i| f.failed(i * 100, Failure::Unreachable, 0).at - i * 100)
            .collect();
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
        // A connection that proves itself resets the back-off.
        assert_eq!(f.failed(9000, Failure::Ended, PROVEN_SECS).at, 9005);
    }

    #[test]
    fn the_flap_guard_stops_switching_and_stays_on_the_last_good_feed() {
        let mut f = Fallback::new(2);
        // main delivered, then failed: switch 1 (to backup).
        assert_eq!(f.failed(0, Failure::Ended, 20).feed, 1);
        // backup never delivers: switch 2 (to main).
        assert_eq!(f.failed(10, REFUSED, 0).feed, 0);
        // main delivers briefly: switch 3 (to backup).
        assert_eq!(f.failed(60, Failure::Stalled, 15).feed, 1);
        // A fourth within ten minutes is not made: back to main, the last
        // feed that delivered, after a back-off, with one note.
        let n = f.failed(70, REFUSED, 0);
        assert_eq!(n.feed, 0);
        assert!(n.at > 70);
        assert!(n.note.unwrap().contains("staying on the main feed"));
        // Still guarded: no more switches, no more notes.
        let n = f.failed(200, Failure::Ended, 5);
        assert_eq!(n.feed, 0);
        assert_eq!(n.note, None);
        // Ten minutes after the first switches, switching is allowed again,
        // and the backup's old failures are forgotten.
        let n = f.failed(700, Failure::Ended, 5);
        assert_eq!((n.feed, n.at), (1, 700));
    }

    #[test]
    fn riding_another_recordings_connection_is_not_a_switch() {
        let mut f = Fallback::new(2);
        f.ride(1);
        assert_eq!(f.current(), 1);
        // Three real switches are still allowed before the flap guard.
        assert_eq!(f.failed(0, Failure::Ended, 20).feed, 0);
        assert_eq!(f.failed(10, REFUSED, 0).feed, 1);
        assert_eq!(f.failed(20, REFUSED, 0).feed, 0);
        f.ride(5);
        assert_eq!(f.current(), 0, "no such feed: nothing changes");
    }

    #[test]
    fn a_backup_chosen_while_recording_is_used_at_the_next_failure() {
        // No backup at first: the main feed is retried.
        let mut f = Fallback::new(1);
        assert_eq!(f.failed(0, REFUSED, 0).feed, 0);
        f.backup_changed(true);
        let n = f.failed(10, REFUSED, 0);
        assert_eq!((n.feed, n.at), (1, 10), "switched to the new backup at once");

        // The backup it is on is taken away: back to the main feed alone.
        f.backup_changed(false);
        assert_eq!(f.current(), 0);
        let n = f.failed(20, Failure::Ended, 40);
        assert_eq!(n.feed, 0);

        // A replaced backup carries none of the old one's failures.
        let mut f = Fallback::new(2);
        f.failed(0, REFUSED, 0); // main → backup
        f.failed(1, REFUSED, 0); // backup → main
        f.backup_changed(true);
        let n = f.failed(2, REFUSED, 0);
        assert_eq!((n.feed, n.at), (1, 2), "no back-off inherited");
    }

    #[test]
    fn switching_to_a_feed_that_just_failed_waits_its_back_off() {
        let mut f = Fallback::new(2);
        f.failed(0, REFUSED, 0); // main failed once → backup
        let n = f.failed(1, REFUSED, 0); // backup failed once → main, which failed once
        assert_eq!((n.feed, n.at), (0, 1 + 5));
    }

    const OFF_AIR: Failure = Failure::OffAir { clip_secs: 600 };

    #[test]
    fn a_feed_off_air_is_asked_again_every_few_minutes_without_a_back_off() {
        let mut f = Fallback::new(1);
        for i in 0..10 {
            let now = 1000 + i * OFF_AIR_RETRY_SECS;
            let n = f.failed(now, OFF_AIR, 0);
            assert_eq!((n.feed, n.at), (0, now + OFF_AIR_RETRY_SECS));
            assert_eq!(n.note, None, "the recording says so itself, once");
        }
        // Then it broadcasts, and a failure after that is an ordinary one.
        let n = f.failed(5000, Failure::Ended, 40);
        assert_eq!(n.at, 5000 + 5);
        assert_eq!(
            Fallback::new(1).with_off_air_retry(3).failed(10, OFF_AIR, 0).at,
            13
        );
    }

    #[test]
    fn off_air_tries_the_backup_at_once_then_both_every_few_minutes() {
        let mut f = Fallback::new(2);
        let n = f.failed(0, OFF_AIR, 0);
        assert_eq!((n.feed, n.at), (1, 0), "the backup at once");
        assert!(n.note.unwrap().contains("off air"));
        // The backup is off air too: the main one again, in a while.
        let n = f.failed(2, OFF_AIR, 0);
        assert_eq!((n.feed, n.at), (0, 2 + OFF_AIR_RETRY_SECS));
        // Still off air: the backup at once (it was asked long enough ago),
        // and so on, never the flap guard's.
        let mut t = 2 + OFF_AIR_RETRY_SECS;
        let n = f.failed(t, OFF_AIR, 0);
        assert_eq!((n.feed, n.at), (1, t));
        for i in 0..10 {
            let n = f.failed(t, OFF_AIR, 0);
            assert_eq!(n.at, t + OFF_AIR_RETRY_SECS, "{i}: both asked, so a wait");
            t = n.at;
            let n = f.failed(t, OFF_AIR, 0);
            assert_eq!(n.at, t, "{i}: the other one straight after");
            assert!(n.note.unwrap().contains("switched"), "never the flap guard");
        }
    }
}
