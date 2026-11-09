//! HDHomeRun network tuners: the third way to describe a catalogue.
//!
//! A SiliconDust HDHomeRun is a box on the local network with an aerial (or
//! a cable) going in and HTTP coming out. It describes itself at
//! `/discover.json`, lists what it can tune at the `LineupURL` that names
//! (`/lineup.json`: one entry per channel, each with the URL that streams
//! it), and streams MPEG-TS from port 5004 — one tuner per stream, and it
//! says `503` when every tuner is busy.
//!
//! Its channels go into the mirror through the playlist writer
//! (`catalog::playlist`), because a lineup is a playlist in every way the
//! mirror cares about: a stored stream URL per channel, no catch-up, a guide
//! from somewhere else. What differs is where the ids and the guide come
//! from (see [`device`] and [`guide`]).
//!
//! Nothing here writes to the database except [`guide::land`], and
//! `catalog::sync_devices` decides when.

pub mod account;
pub mod device;
pub mod guide;
pub mod source;

pub use account::HdhrAccount;
pub use source::HdhrSource;

/// A JSON value read as a yes/no: the lineup writes `"HD":1`, older
/// firmware `"HD":"1"`, and a hand-made fixture `true`. Absent is no.
pub(crate) fn flag(v: &Option<serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        Some(serde_json::Value::String(s)) => matches!(s.trim(), "1" | "true" | "yes"),
        _ => false,
    }
}

/// A JSON value read as text, whether it was written as a string or a
/// number: `"GuideNumber":"5.1"` usually, `"GuideNumber":702` sometimes.
pub(crate) fn text(v: &Option<serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.trim().to_owned(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}
