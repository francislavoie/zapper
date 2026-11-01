//! The account's connection budget: how many streams it may hold, how many
//! are open, and whether this app may open another.
//!
//! Many accounts allow one stream at a time, and a provider settles a second
//! by ending one of the two. Two copies of zapper on one account would then
//! take the connection from each other forever if either reconnected by
//! itself after losing it — so nothing does, unless the account has a
//! connection to spare. This is where that is decided: for the player's
//! automatic reconnect, for the recording service before each capture
//! (`recording::budget`, which reads it through [`look`] and the same
//! "Simultaneous streams"), and for anything later that wants a second
//! stream, such as a warm standby for the previous channel: ask
//! [`may_open_another`] first.
//!
//! What is known depends on the account:
//!
//! - **Xtream**: `player_api.php` answers `max_connections` and
//!   `active_cons` — every device's streams, this one's included. An API
//!   call, not a stream, so asking costs no connection.
//! - **HDHomeRun**: `discover.json`'s tuner count, and `status.json`, which
//!   lists what each tuner is doing.
//! - **M3U playlists and Tvheadend**: nothing to ask. The person says how
//!   many streams the account allows (Settings → Account, "Simultaneous
//!   streams", default one), and how many are in use elsewhere is unknown.
//!
//! The decisions are pure and tested; [`look`] does the asking.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::source::Source;

/// How many streams an account allows, and how many it has open — on every
/// device, this one's included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slots {
    pub max: u32,
    pub active: u32,
}

impl Slots {
    pub fn free(&self) -> bool {
        self.active < self.max
    }
}

/// What can be known about the account's connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The provider counted them.
    Counted(Slots),
    /// Nobody can count them; the person said how many the account allows.
    Declared(u32),
    /// The provider could not be asked, or did not answer.
    Unknown,
}

// ---------------------------------------------------------------------------
// What this app holds
// ---------------------------------------------------------------------------

/// What a stream this app holds is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// What is on screen.
    Player,
}

const USES: usize = 1;

/// Streams held, by use. A count per use rather than a set, so that a
/// future use holding several (recordings) fits.
static HELD: Mutex<[u32; USES]> = Mutex::new([0; USES]);

/// Records whether `what` holds a stream now. The player calls this as
/// streams open and end.
pub fn set_held(what: Use, holding: bool) {
    let mut h = HELD.lock().unwrap_or_else(|e| e.into_inner());
    h[what as usize] = u32::from(holding);
}

/// Streams this app holds, of every use.
pub fn held() -> u32 {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).iter().sum()
}

/// Whether this app may open one more stream, holding `ours` already.
///
/// A counted limit already includes ours in `active`. A declared one does
/// not know about other devices at all, so it is only ever a ceiling on
/// this app; an unknown one allows a first stream and nothing more.
pub fn may_open_another(limit: Limit, ours: u32) -> bool {
    match limit {
        Limit::Counted(s) => s.free(),
        Limit::Declared(max) => ours < max.max(1),
        Limit::Unknown => ours == 0,
    }
}

// ---------------------------------------------------------------------------
// Reconnecting
// ---------------------------------------------------------------------------

/// The automatic reconnects allowed after a network failure, and the wait
/// before each. Then it gives up and says why.
pub const BACKOFF: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// How long to wait before asking the provider a second time when the
/// first answer said the account was full. Servers are slow to let go of a
/// connection that died, and the dead one may be this app's own.
pub const SECOND_LOOK: Duration = Duration::from_secs(4);

/// The wait before automatic reconnect number `attempt` (from 1), or `None`
/// once they are used up. `unit` is a random number in `0..1`: the wait is
/// spread a quarter either side, so that two copies of zapper that lost the
/// same network blip do not come back in the same instant.
pub fn backoff(attempt: u32, unit: f64) -> Option<Duration> {
    let base = BACKOFF.get(usize::try_from(attempt.checked_sub(1)?).ok()?)?;
    let unit = if unit.is_finite() { unit.clamp(0.0, 1.0) } else { 0.5 };
    Some(base.mul_f64(0.75 + unit * 0.5))
}

/// A random number in `0..1`, for [`backoff`]. Not cryptographic and not
/// meant to be: std's hasher keys are random per instance, which is enough
/// to keep two machines apart.
pub fn jitter() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}

/// What to do about reconnecting, once the wait is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Reconnect.
    Go,
    /// The account looked full; look again after [`SECOND_LOOK`].
    LookAgain,
    /// Still full on the second look: another device has the connection.
    /// Stop, and say so.
    InUse(Slots),
    /// Nothing could be asked, so whether a connection is free is not known.
    /// Do not connect now; count the attempt and wait for the next.
    NotYet,
    /// One stream allowed and no way to see who has it: stop. Reconnecting
    /// could take it from another device.
    Unsafe,
}

/// Whether to reconnect after a network failure, from what the account
/// says about its connections.
///
/// `unreached` is that the failure proves no request ever reached the
/// provider (the name did not resolve, nothing answered): such a stream
/// held no connection and took nobody's, and the next attempt either fails
/// the same way or finds the network back. It is the only reconnect allowed
/// on an account of one stream that cannot be counted. `second` is whether
/// this is the second look.
pub fn reconnect_verdict(limit: Limit, unreached: bool, second: bool) -> Verdict {
    match limit {
        Limit::Counted(s) if s.free() => Verdict::Go,
        Limit::Counted(_) if !second => Verdict::LookAgain,
        Limit::Counted(s) => Verdict::InUse(s),
        Limit::Declared(max) if max > 1 || unreached => Verdict::Go,
        Limit::Declared(_) => Verdict::Unsafe,
        Limit::Unknown if unreached => Verdict::Go,
        Limit::Unknown => Verdict::NotYet,
    }
}

// ---------------------------------------------------------------------------
// Asking
// ---------------------------------------------------------------------------

/// What the account says about itself, for Settings → Account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// Counted by the provider, or declared by the person.
    pub max: Option<u32>,
    /// Counted by the provider; `None` where nobody can count.
    pub active: Option<u32>,
    /// When the account expires, Unix seconds, if the provider says.
    pub expires: Option<i64>,
}

impl Status {
    pub fn limit(&self) -> Limit {
        match (self.max, self.active) {
            (Some(max), Some(active)) => Limit::Counted(Slots { max, active }),
            (Some(max), None) => Limit::Declared(max),
            _ => Limit::Unknown,
        }
    }
}

/// An Xtream server's `user_info` as connection figures. Servers send
/// `max_connections` as a number or a string, and some send `0` or nothing
/// for "unlimited" — which can't be counted against, so it is unknown.
pub fn xtream_status(max: Option<i64>, active: Option<i64>, expires: Option<i64>) -> Status {
    let max = max.and_then(|m| u32::try_from(m).ok()).filter(|m| *m > 0);
    Status {
        max,
        active: max.and(active.and_then(|a| u32::try_from(a).ok())),
        expires: expires.filter(|e| *e > 0),
    }
}

/// How many of an HDHomeRun's tuners are busy, from its `status.json`: a
/// tuner watching or recording names its channel (`VctNumber`) or where it
/// is sending (`TargetIP`); an idle one has neither.
pub fn tuners_in_use(status: &serde_json::Value) -> u32 {
    status.as_array().map_or(0, |tuners| {
        tuners
            .iter()
            .filter(|t| t.get("VctNumber").is_some() || t.get("TargetIP").is_some())
            .count() as u32
    })
}

/// The last answer, and when it came, so that Settings opening twice does
/// not ask twice.
static LAST: Mutex<Option<(Instant, Status)>> = Mutex::new(None);

/// How long an answer is good for, for showing. A reconnect always asks
/// afresh.
const FRESH: Duration = Duration::from_secs(20);

/// Asks the account about its connections. `declared` is the person's
/// "Simultaneous streams", for accounts that cannot be asked.
pub async fn look(source: &Source, declared: u32) -> Status {
    let declared = Status {
        max: Some(declared.max(1)),
        ..Status::default()
    };
    let status = match source {
        Source::Xtream(c) => match c.login().await {
            Ok(u) => xtream_status(u.max_connections, u.active_cons, u.exp_date),
            Err(e) => {
                tracing::info!(
                    "could not ask the server about connections: {}",
                    crate::catalog::sync_engine::scrub(&format!("{e:#}"))
                );
                Status::default()
            }
        },
        Source::HdHomeRun(h) => h.tuner_status().await.unwrap_or_default(),
        Source::M3u(_) | Source::Tvheadend(_) => declared,
    };
    if let Ok(mut last) = LAST.lock() {
        *last = Some((Instant::now(), status));
    }
    status
}

/// [`look`], unless it was asked a moment ago.
pub async fn look_cached(source: &Source, declared: u32) -> Status {
    if let Ok(last) = LAST.lock() {
        if let Some((at, s)) = *last {
            if at.elapsed() < FRESH {
                return s;
            }
        }
    }
    look(source, declared).await
}

/// Forgets the last answer: the account changed.
pub fn forget() {
    if let Ok(mut last) = LAST.lock() {
        *last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_OF_ONE: Slots = Slots { max: 1, active: 1 };

    #[test]
    fn a_free_connection_is_reconnected_to() {
        let free = Limit::Counted(Slots { max: 2, active: 1 });
        assert_eq!(reconnect_verdict(free, false, false), Verdict::Go);
        let empty = Limit::Counted(Slots { max: 1, active: 0 });
        assert_eq!(reconnect_verdict(empty, false, false), Verdict::Go);
    }

    #[test]
    fn a_full_account_is_looked_at_twice_and_then_left_alone() {
        // The first look may still count this app's own dead connection.
        let full = Limit::Counted(ONE_OF_ONE);
        assert_eq!(reconnect_verdict(full, false, false), Verdict::LookAgain);
        assert_eq!(reconnect_verdict(full, false, true), Verdict::InUse(ONE_OF_ONE));
        // Even when the failure never reached the provider: the account says
        // somebody else has it, and connecting would take it from them.
        assert_eq!(reconnect_verdict(full, true, true), Verdict::InUse(ONE_OF_ONE));
    }

    #[test]
    fn one_stream_that_cannot_be_counted_is_only_retried_when_nothing_connected() {
        let one = Limit::Declared(1);
        assert_eq!(reconnect_verdict(one, false, false), Verdict::Unsafe);
        assert_eq!(reconnect_verdict(one, true, false), Verdict::Go);
        // More than one: there is a spare for this app.
        assert_eq!(reconnect_verdict(Limit::Declared(2), false, false), Verdict::Go);
    }

    #[test]
    fn a_provider_that_cannot_be_asked_is_waited_on() {
        assert_eq!(reconnect_verdict(Limit::Unknown, false, false), Verdict::NotYet);
        assert_eq!(reconnect_verdict(Limit::Unknown, true, false), Verdict::Go);
    }

    #[test]
    fn the_backoff_runs_two_five_ten_thirty_and_stops() {
        let mid: Vec<_> = (1..=4).map(|n| backoff(n, 0.5).unwrap().as_secs_f64()).collect();
        assert_eq!(mid, vec![2.0, 5.0, 10.0, 30.0]);
        assert_eq!(backoff(5, 0.5), None);
        assert_eq!(backoff(0, 0.5), None);
    }

    #[test]
    fn the_backoff_is_spread_so_two_clients_do_not_return_together() {
        assert_eq!(backoff(2, 0.0).unwrap().as_secs_f64(), 3.75);
        assert_eq!(backoff(2, 1.0).unwrap().as_secs_f64(), 6.25);
        assert_eq!(backoff(2, f64::NAN).unwrap().as_secs_f64(), 5.0);
        let draws: Vec<f64> = (0..64).map(|_| jitter()).collect();
        assert!(draws.iter().all(|u| (0.0..1.0).contains(u)));
        let distinct = draws.iter().map(|u| (u * 1e6) as u64).collect::<std::collections::HashSet<_>>();
        assert!(distinct.len() > 32, "jitter is not jittering: {draws:?}");
    }

    #[test]
    fn another_stream_needs_a_free_connection() {
        let full = Limit::Counted(ONE_OF_ONE);
        assert!(!may_open_another(full, 1));
        assert!(may_open_another(Limit::Counted(Slots { max: 2, active: 1 }), 1));
        assert!(may_open_another(Limit::Declared(2), 1));
        assert!(!may_open_another(Limit::Declared(1), 1));
        assert!(may_open_another(Limit::Declared(1), 0));
        assert!(may_open_another(Limit::Unknown, 0));
        assert!(!may_open_another(Limit::Unknown, 1));
    }

    #[test]
    fn xtreams_user_info_reads_as_connections() {
        let s = xtream_status(Some(1), Some(1), Some(1_800_000_000));
        assert_eq!(s.limit(), Limit::Counted(ONE_OF_ONE));
        assert_eq!(s.expires, Some(1_800_000_000));
        // "0" is how some servers say unlimited: nothing to count against.
        let s = xtream_status(Some(0), Some(3), None);
        assert_eq!(s.limit(), Limit::Unknown);
        // No active count: known ceiling, unknown use.
        assert_eq!(xtream_status(Some(2), None, None).limit(), Limit::Declared(2));
        assert_eq!(xtream_status(Some(2), Some(-1), Some(0)).expires, None);
    }

    #[test]
    fn an_hdhomeruns_busy_tuners_are_counted() {
        let status = serde_json::json!([
            {"Resource": "tuner0", "VctNumber": "5.1", "TargetIP": "192.168.1.20"},
            {"Resource": "tuner1"},
            {"Resource": "tuner2", "TargetIP": "192.168.1.30"}
        ]);
        assert_eq!(tuners_in_use(&status), 2);
        assert_eq!(tuners_in_use(&serde_json::json!({})), 0);
    }

    #[test]
    fn what_is_held_is_counted() {
        set_held(Use::Player, true);
        assert_eq!(held(), 1);
        set_held(Use::Player, true);
        assert_eq!(held(), 1, "holding is a state, not a counter to drift");
        set_held(Use::Player, false);
        assert_eq!(held(), 0);
    }
}
