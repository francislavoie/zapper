//! Which channels the recording service on this computer shares a
//! connection to, as its list of recordings last said: tuning to one of them
//! joins that connection (the controller asks [`captured`]) rather than
//! opening one of the player's own. [`super::handover`] keeps it current as
//! the list is fetched ([`remember`]).
//!
//! On its own so that the controller and the handover each depend on it,
//! and not on each other.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the recordings last fetched say what the service records: the
/// interface fetches them every few seconds while anything records, and
/// every half minute otherwise.
const FRESH: Duration = Duration::from_secs(45);

/// The channels the service shares a connection to, and when that was
/// learnt.
static CAPTURED: Mutex<Option<(Instant, Vec<i64>)>> = Mutex::new(None);

/// Whether the service on this computer is recording `stream_id` now, by
/// the last list fetched.
pub fn captured(stream_id: i64) -> bool {
    CAPTURED
        .lock()
        .ok()
        .and_then(|c| c.as_ref().map(|(at, feeds)| at.elapsed() < FRESH && feeds.contains(&stream_id)))
        .unwrap_or(false)
}

/// The channels the service's list, fetched just now, says it shares a
/// connection to.
pub fn remember(feeds: Vec<i64>) {
    if let Ok(mut c) = CAPTURED.lock() {
        *c = Some((Instant::now(), feeds));
    }
}
