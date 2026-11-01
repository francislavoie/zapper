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
//! How full the account is depends on the kind:
//!
//! - **Xtream**: `player_api.php` with no action reports `active_cons` and
//!   `max_connections`. That request is an API call, not a stream, so asking
//!   costs no connection.
//! - **M3U**: no API, so the service counts its own captures against the
//!   "simultaneous streams" setting (1 unless set). It cannot see another
//!   device; the provider will refuse or drop, and the fallback rules treat a
//!   limit refusal as the account's, never something to retry quickly.
//! - **HDHomeRun, Tvheadend**: their own servers allot tuners and refuse when
//!   none is free; unknown here, so allowed.
//!
//! This is deliberately one function, [`check`], over a small [`Budget`]
//! seam: a shared connection-budget module for the whole app (playback
//! included) can replace [`SourceBudget`] without the capture changing.

use std::future::Future;
use std::pin::Pin;

use crate::catalog::credentials::Kind;
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

/// The decision itself, apart from how the numbers were learnt.
///
/// `reported` is the provider's `(active, max)` when it has an API for it;
/// `limit` is the configured number of simultaneous streams for kinds that
/// do not.
pub fn decide(kind: Kind, reported: Option<(i64, i64)>, own_active: usize, limit: u32) -> Slots {
    match kind {
        Kind::Xtream => match reported {
            // `max_connections` of 0 is how some panels say "unlimited".
            Some((active, max)) if max > 0 && active >= max => Slots::Full { active, max },
            Some(_) => Slots::Free,
            None => Slots::Unknown,
        },
        Kind::M3u => {
            let max = i64::from(limit.max(1));
            let active = own_active as i64;
            if active >= max {
                Slots::Full { active, max }
            } else {
                Slots::Free
            }
        }
        Kind::HdHomeRun | Kind::Tvheadend => Slots::Unknown,
    }
}

/// The budget as the account's own source reports it.
pub struct SourceBudget {
    /// Simultaneous streams for accounts with no API to ask.
    pub limit: u32,
}

impl Budget for SourceBudget {
    fn check(&self, own_active: usize) -> Pin<Box<dyn Future<Output = Slots> + Send + '_>> {
        Box::pin(async move {
            let source = match Source::load() {
                Ok(s) => s,
                Err(_) => return Slots::Unknown,
            };
            let reported = match &source {
                Source::Xtream(c) => match c.login().await {
                    Ok(info) => info.active_cons.zip(info.max_connections),
                    Err(e) => {
                        tracing::debug!(
                            "could not ask the provider for its connection count: {}",
                            crate::catalog::sync_engine::scrub(&format!("{e:#}"))
                        );
                        None
                    }
                },
                _ => None,
            };
            decide(source.kind(), reported, own_active, self.limit)
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

    #[test]
    fn an_xtream_account_is_full_when_its_connections_are_all_in_use() {
        assert_eq!(
            decide(Kind::Xtream, Some((1, 1)), 0, 1),
            Slots::Full { active: 1, max: 1 }
        );
        assert_eq!(decide(Kind::Xtream, Some((1, 2)), 0, 1), Slots::Free);
        assert_eq!(
            decide(Kind::Xtream, Some((5, 0)), 0, 1),
            Slots::Free,
            "0 means unlimited"
        );
        assert_eq!(decide(Kind::Xtream, None, 0, 1), Slots::Unknown);
    }

    #[test]
    fn a_playlist_counts_the_services_own_streams_against_the_setting() {
        assert_eq!(decide(Kind::M3u, None, 0, 1), Slots::Free);
        assert_eq!(
            decide(Kind::M3u, None, 1, 1),
            Slots::Full { active: 1, max: 1 }
        );
        assert_eq!(decide(Kind::M3u, None, 1, 2), Slots::Free);
        assert_eq!(decide(Kind::M3u, None, 0, 0), Slots::Free, "0 is read as 1");
    }

    #[test]
    fn tuners_allot_themselves() {
        assert_eq!(decide(Kind::HdHomeRun, None, 9, 1), Slots::Unknown);
        assert_eq!(decide(Kind::Tvheadend, None, 9, 1), Slots::Unknown);
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
