//! Which time zone an Xtream server's clock is in.
//!
//! A catch-up request names its start as wall time, `YYYY-MM-DD:HH-MM`, and
//! the server reads that in *its own* zone. That is often not the viewer's:
//! a server an hour east of UTC, asked by a viewer five hours west of it for
//! "five minutes ago" in the viewer's wall time, replays what aired six hours
//! earlier — and near the far end of the archive, asks for something it no
//! longer has.
//!
//! The server says what its clock reads in `player_api.php`'s `server_info`
//! (`time_now` beside `timestamp_now`), so every sync of an Xtream account
//! asks, works out the offset ([`crate::xtream::models::panel_utc_offset`])
//! and keeps it here, per catalogue origin, in `meta`. Asked at every sync
//! so that the server's clocks changing for summer time is picked up within
//! a day.
//!
//! When it is not known — a catalogue synced before this, or a server that
//! does not say — the machine's own offset is used, as it always was, and
//! the log says so once.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::catalog::{db, origin};
use crate::xtream::{Credentials, XtreamClient};

const KEY_PREFIX: &str = "panel_utc_offset:";

fn key(origin: &str) -> String {
    format!("{KEY_PREFIX}{origin}")
}

/// The offset last learnt for `origin`'s server, seconds east of UTC.
pub fn recorded(conn: &Connection, origin: &str) -> Result<Option<i64>> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key(origin)], |r| r.get(0))
        .optional()?;
    Ok(v.and_then(|s| s.trim().parse().ok()))
}

/// Notes `origin`'s server's offset.
pub fn record(conn: &Connection, origin: &str, offset: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key(origin), offset.to_string()],
    )?;
    Ok(())
}

/// Asks the server for its clock and keeps the offset, for a sync pass.
///
/// Never fails the pass: the catalogue does not depend on it, and an
/// offset already on record stays until a reading replaces it — the zone a
/// server ran in yesterday is a better guess than this machine's.
pub async fn learn(client: &XtreamClient, origin: &str) {
    let answer = match client.account().await {
        Ok(a) => a,
        Err(e) => {
            tracing::info!(
                "could not ask the server for its clock: {}",
                crate::catalog::sync_engine::scrub(&format!("{e:#}"))
            );
            return;
        }
    };
    let Some(offset) = answer.server_info.utc_offset() else {
        tracing::info!("the server did not say what its clock reads; catch-up uses this computer's time zone");
        return;
    };
    match db::with_write_db(|conn| record(conn, origin, offset)) {
        Ok(()) => tracing::info!("the server's clock is {}", describe(offset)),
        Err(e) => tracing::warn!("could not keep the server's time zone: {e:#}"),
    }
}

/// `UTC+01:00`, `UTC−05:00`, `UTC+05:30`.
pub fn describe(offset: i64) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let a = offset.abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
}

/// The offset to write a catch-up start for this account's server in:
/// the one on record, or this machine's at `start` when there is none.
pub fn offset_for(creds: &Credentials, start: i64) -> i64 {
    let from = origin::of_xtream(creds);
    let stored = db::with_db(|conn| recorded(conn, &from)).ok().flatten();
    choose(stored, || crate::localtime::utc_offset(start))
}

/// How long a catch-up waits for the server's clock when none is on record.
const ASK_FOR: std::time::Duration = std::time::Duration::from_secs(5);

/// Like [`offset_for`], but when nothing is on record asks the server's
/// clock right now (the account call, which is not a stream and takes no
/// connection), keeps what it says, and uses it. Only if that fails does
/// it fall back to this machine's offset.
pub async fn offset_now(client: &XtreamClient, start: i64) -> i64 {
    let from = origin::of_xtream(client.credentials());
    let stored = db::with_db(|conn| recorded(conn, &from)).ok().flatten();
    if stored.is_none() {
        match tokio::time::timeout(ASK_FOR, client.account()).await {
            Ok(Ok(a)) => match a.server_info.utc_offset() {
                Some(o) => {
                    match db::with_write_db(|conn| record(conn, &from, o)) {
                        Ok(()) => tracing::info!("the server's clock is {}", describe(o)),
                        Err(e) => tracing::warn!("could not keep the server's time zone: {e:#}"),
                    }
                    return o;
                }
                None => tracing::info!("the server did not say what its clock reads"),
            },
            Ok(Err(e)) => tracing::info!(
                "could not ask the server for its clock: {}",
                crate::catalog::sync_engine::scrub(&format!("{e:#}"))
            ),
            Err(_) => tracing::info!("the server did not give its clock in time"),
        }
    }
    choose(stored, || crate::localtime::utc_offset(start))
}

static FELL_BACK: AtomicBool = AtomicBool::new(false);

/// The server's offset when known; otherwise `local`, said once.
fn choose(stored: Option<i64>, local: impl FnOnce() -> i64) -> i64 {
    choose_noting(&FELL_BACK, stored, local)
}

fn choose_noting(said: &AtomicBool, stored: Option<i64>, local: impl FnOnce() -> i64) -> i64 {
    match stored {
        Some(o) => o,
        None => {
            let o = local();
            if !said.swap(true, Ordering::Relaxed) {
                tracing::info!(
                    "the server's time zone is not known yet; asking for catch-up in this computer's ({})",
                    describe(o)
                );
            }
            o
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;

    #[test]
    fn an_offset_is_kept_per_origin_and_replaced_by_the_next() {
        let c = open_in_memory().unwrap();
        assert_eq!(recorded(&c, "xtream:a").unwrap(), None);
        record(&c, "xtream:a", 3600).unwrap();
        record(&c, "xtream:b", -5 * 3600).unwrap();
        assert_eq!(recorded(&c, "xtream:a").unwrap(), Some(3600));
        assert_eq!(recorded(&c, "xtream:b").unwrap(), Some(-18_000));
        // The clocks went back for the winter.
        record(&c, "xtream:a", 0).unwrap();
        assert_eq!(recorded(&c, "xtream:a").unwrap(), Some(0));
    }

    #[test]
    fn the_server_offset_wins_and_the_local_one_is_the_fallback() {
        assert_eq!(choose(Some(3600), || -5 * 3600), 3600);
        assert_eq!(choose(Some(0), || -5 * 3600), 0);
        assert_eq!(choose(None, || -5 * 3600), -18_000);
        // Not asked for when it is not needed.
        assert_eq!(choose(Some(19_800), || unreachable!()), 19_800);
    }

    #[test]
    fn a_catch_up_is_stamped_in_the_offset_learnt_for_its_account() {
        db::install_test_db();
        let creds = |user: &str| Credentials {
            host: "http://panel-clock.example".into(),
            username: user.into(),
            password: "p".into(),
        };
        let east = creds("east-of-utc");
        db::with_write_db(|c| record(c, &origin::of_xtream(&east), 3600)).unwrap();
        assert_eq!(offset_for(&east, 1_794_082_337), 3600);
        // An account nothing was learnt for falls back to this computer's.
        let unknown = creds("never-synced");
        assert_eq!(
            offset_for(&unknown, 1_794_082_337),
            crate::localtime::utc_offset(1_794_082_337)
        );
    }

    fn panel(offset_hours: i64, user: &str) -> (crate::test_http::Server, XtreamClient) {
        let now = 1_794_082_637_i64;
        // The server's wall clock, written as `time_now` is.
        let t = chrono_like(now + offset_hours * 3600);
        let body = format!(
            r#"{{"user_info":{{"auth":1}},"server_info":{{"time_now":"{t}","timestamp_now":{now}}}}}"#
        );
        let s = crate::test_http::serve(move |_, _| crate::test_http::Reply::ok(body.clone()));
        let c = XtreamClient::new(Credentials {
            host: s.base.clone(),
            username: user.into(),
            password: "secret-pw".into(),
        })
        .unwrap();
        (s, c)
    }

    /// `YYYY-MM-DD HH:MM:SS` for Unix seconds read as UTC.
    fn chrono_like(secs: i64) -> String {
        let days = secs.div_euclid(86_400);
        let r = secs.rem_euclid(86_400);
        // Civil-from-days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", r / 3600, (r % 3600) / 60, r % 60)
    }

    #[tokio::test]
    async fn an_unknown_clock_is_asked_for_when_a_catch_up_needs_it_and_kept() {
        db::install_test_db();
        let (s, c) = panel(2, "asked-on-demand");
        let from = origin::of_xtream(c.credentials());
        assert_eq!(db::with_db(|k| recorded(k, &from)).unwrap(), None);
        assert_eq!(offset_now(&c, 1_794_082_337).await, 7200);
        assert_eq!(db::with_db(|k| recorded(k, &from)).unwrap(), Some(7200));
        // Known now: no second call.
        assert_eq!(offset_now(&c, 1_794_082_337).await, 7200);
        let seen = s.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].query("action"), None, "the account call, not a stream");
        assert!(seen[0].route().starts_with("/player_api.php"));
    }

    #[tokio::test]
    async fn a_clock_that_cannot_be_asked_falls_back_to_local_and_says_so_once() {
        db::install_test_db();
        let s = crate::test_http::serve(|_, _| crate::test_http::Reply::new(500, "nope"));
        let c = XtreamClient::new(Credentials {
            host: s.base.clone(),
            username: "unreachable-clock".into(),
            password: "secret-pw".into(),
        })
        .unwrap();
        let start = 1_794_082_337;
        assert_eq!(offset_now(&c, start).await, crate::localtime::utc_offset(start));
        let from = origin::of_xtream(c.credentials());
        assert_eq!(db::with_db(|k| recorded(k, &from)).unwrap(), None);
        // The notice is said once, however many catch-ups fall back.
        let said = AtomicBool::new(false);
        assert_eq!(choose_noting(&said, None, || 5), 5);
        assert!(said.load(Ordering::Relaxed));
        assert_eq!(choose_noting(&said, None, || 6), 6);
    }

    #[test]
    fn offsets_are_described_as_zones() {
        assert_eq!(describe(3600), "UTC+01:00");
        assert_eq!(describe(-5 * 3600), "UTC-05:00");
        assert_eq!(describe(5 * 3600 + 1800), "UTC+05:30");
        assert_eq!(describe(0), "UTC+00:00");
    }
}
