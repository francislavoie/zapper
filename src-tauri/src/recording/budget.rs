//! Whether the account has a connection to spare, asked before every
//! connection a capture makes.
//!
//! Most providers allow a few streams at once per account, often one, and
//! end the oldest when another device opens one. A recording that connected
//! regardless would take the stream from whoever is watching, and if their
//! player reconnected too the two would trade the connection back and forth.
//! So the capture asks here first, and when the account is full it does not
//! connect: the recording waits ("waiting for a free connection"), asking
//! again with back-off.
//!
//! How full the account is comes from [`crate::connections`], the one
//! connection budget the app's player asks too, so the two can never
//! disagree about an account:
//!
//! - **Xtream**: `player_api.php` reports `active_cons` and
//!   `max_connections` — every device's streams, this service's included.
//!   An API call, not a stream, so asking costs no connection.
//! - **HDHomeRun**: its tuner count and how many are busy.
//! - **M3U**: no API, so the service counts its own captures against the
//!   app's "Simultaneous streams" (Settings → Account, 1 unless set), read
//!   from the catalogue the two share. It cannot see another device; the
//!   provider will refuse or drop, and the fallback rules treat a limit
//!   refusal as the account's, never something to retry quickly.
//! - **Tvheadend**: its server allots tuners itself and refuses when none
//!   is free, so a capture is allowed and the server answers.
//!
//! This is deliberately one function, [`check`], over a small [`Budget`]
//! seam, so the capture never needs to know where the numbers came from.

use std::future::Future;
use std::pin::Pin;

use crate::connections::{self, Limit};
use crate::source::Source;

/// The account's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slots {
    Free,
    /// `active` of `max` in use.
    Full {
        active: i64,
        max: i64,
    },
    /// Nobody can say; go ahead.
    Unknown,
}

impl Slots {
    pub fn is_full(self) -> bool {
        matches!(self, Slots::Full { .. })
    }
}

/// Asked before each connection. `own_active` is how many streams this
/// service is holding open right now, not counting the one about to open.
pub trait Budget: Send + Sync {
    fn check(&self, own_active: usize) -> Pin<Box<dyn Future<Output = Slots> + Send + '_>>;
}

/// The decision itself, apart from how the numbers were learnt: the
/// account's [`Limit`] and how many streams this service holds now, not
/// counting the one about to open.
///
/// A counted limit already includes this service's own streams in
/// `active`; a declared one knows nothing of other devices, so it is only
/// ever a ceiling on this service.
pub fn decide(limit: Limit, own_active: usize) -> Slots {
    match limit {
        Limit::Counted(s) if s.free() => Slots::Free,
        Limit::Counted(s) => Slots::Full {
            active: i64::from(s.active),
            max: i64::from(s.max),
        },
        Limit::Declared(max) => {
            let max = i64::from(max.max(1));
            let active = own_active as i64;
            if active >= max {
                Slots::Full { active, max }
            } else {
                Slots::Free
            }
        }
        Limit::Unknown => Slots::Unknown,
    }
}

/// The budget as the account's own source reports it, through
/// [`crate::connections::look`].
pub struct SourceBudget;

impl Budget for SourceBudget {
    fn check(&self, own_active: usize) -> Pin<Box<dyn Future<Output = Slots> + Send + '_>> {
        Box::pin(async move {
            let source = match Source::load() {
                Ok(s) => s,
                Err(_) => return Slots::Unknown,
            };
            if matches!(source, Source::Tvheadend(_)) {
                return Slots::Unknown;
            }
            // Read each time: the setting is the app's, and can change while
            // the service runs.
            let declared = crate::catalog::settings::load_or_default().simultaneous_streams;
            decide(connections::look(&source, declared).await.limit(), own_active)
        })
    }
}

/// How long to wait before asking again, the `n`th time in a row (1-based):
/// 30 s doubling to five minutes, spread by `jitter` in `[0, 1)` across
/// ±20% so two services (or a service and an app) never ask in step.
pub fn wait_for_slot(n: u32, jitter: f64) -> i64 {
    let base = (30i64 << n.saturating_sub(1).min(8)).min(300) as f64;
    let spread = 0.8 + 0.4 * jitter.clamp(0.0, 1.0);
    (base * spread).round() as i64
}

/// A jitter in `[0, 1)` from the clock's nanoseconds: nothing here needs
/// more randomness than "not in step".
pub fn jitter() -> f64 {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(n % 1000) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::connections::{Slots as Count, Status};

    #[test]
    fn an_account_that_counts_is_full_when_its_connections_are_all_in_use() {
        let counted = |active, max| Limit::Counted(Count { max, active });
        assert_eq!(decide(counted(1, 1), 0), Slots::Full { active: 1, max: 1 });
        assert_eq!(decide(counted(1, 2), 0), Slots::Free);
        // The provider's count already has this service's own streams in it.
        assert_eq!(decide(counted(1, 2), 1), Slots::Free);
    }

    #[test]
    fn xtreams_unlimited_and_silent_answers_do_not_hold_a_recording_back() {
        // `max_connections` of 0 is how some panels say "unlimited".
        assert_eq!(decide(connections::xtream_status(Some(0), Some(5), None).limit(), 0), Slots::Unknown);
        assert_eq!(decide(Status::default().limit(), 0), Slots::Unknown);
    }

    #[test]
    fn a_playlist_counts_the_services_own_streams_against_the_setting() {
        assert_eq!(decide(Limit::Declared(1), 0), Slots::Free);
        assert_eq!(decide(Limit::Declared(1), 1), Slots::Full { active: 1, max: 1 });
        assert_eq!(decide(Limit::Declared(2), 1), Slots::Free);
        assert_eq!(decide(Limit::Declared(0), 0), Slots::Free, "0 is read as 1");
    }

    #[test]
    fn waiting_backs_off_with_jitter() {
        assert_eq!(wait_for_slot(1, 0.5), 30);
        assert_eq!(wait_for_slot(2, 0.5), 60);
        assert_eq!(wait_for_slot(9, 0.5), 300);
        assert_eq!(wait_for_slot(1, 0.0), 24);
        assert_eq!(wait_for_slot(1, 1.0), 36);
    }
}
