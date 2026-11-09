//! Recording what the app's player is watching (docs/recording.md,
//! "Recording what you watch"): the decisions, apart from mpv and the
//! service's API, so they are tested on their own.
//!
//! The app's player connects to the provider by itself, and the service
//! never ends a viewer's connection for a recording. On an account with one
//! connection, Record on the channel being watched would leave the recording
//! waiting until the player stopped. Instead, when the service runs on this
//! computer, the player lets its connection go and takes a place on the
//! service's connection to the channel (`super::watch`), which the recording
//! rides too: one connection for both ([`plan`]).
//!
//! And what the player has already received of the programme is not lost:
//! mpv keeps the last minutes of the stream for rewinding, and the part of
//! it from the programme's padded start ([`keep`]) is written out and handed
//! to the recording as its first part.

/// What Record on a programme does with the app's player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Nothing: the service connects for itself (or rides a connection it
    /// has), and the player carries on as it is.
    Alone,
    /// The player moves onto the service's connection to the channel.
    Handover,
    /// The player holds the account's last connection and nothing on this
    /// computer can share it: say so rather than leave the recording
    /// waiting.
    NoService,
}

/// What the decision needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    /// The player shows the programme's channel live, on a connection of
    /// its own (not a catch-up programme, not already through the service).
    pub watching_direct: bool,
    /// The recording's window is open: it starts now, not later.
    pub starts_now: bool,
    /// The account has a connection beyond the player's
    /// (`connections::may_open_another`).
    pub room: bool,
    /// The service already has a connection to the channel (it records
    /// something else on it).
    pub captured: bool,
    /// The service on this computer answers.
    pub service: bool,
}

/// What Record does with the player.
///
/// Only a recording that starts now, of the channel the player shows on its
/// own connection, concerns the player at all. Then it moves onto the
/// service's connection when the account has none to spare (the recording
/// would otherwise wait for the player), or when the service is on that
/// channel already (one connection does for both, and the player's own is
/// given back). On an account with room, nothing changes: the service opens
/// its own.
pub fn plan(f: Facts) -> Plan {
    if !f.watching_direct || !f.starts_now {
        return Plan::Alone;
    }
    if f.room && !f.captured {
        return Plan::Alone;
    }
    if f.service {
        Plan::Handover
    } else {
        Plan::NoService
    }
}

/// Words for [`Plan::NoService`].
pub const NO_SERVICE: &str =
    "Can't record while you watch on a 1-stream account unless the recording service runs on this computer.";

/// The span of the player's buffer to hand over: mpv's cache from `start`
/// to `end` (the stream's own time), whose first packet arrived at
/// `first_ms` (Unix milliseconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keep {
    pub start: f64,
    pub end: f64,
    pub first_ms: i64,
}

/// Less than this is not worth a part of its own.
pub const KEEP_AT_LEAST_SECS: f64 = 3.0;

/// What of the player's buffer belongs to a recording whose padded start is
/// `from` (Unix seconds): from that moment, or from the oldest the buffer
/// holds when it does not reach back that far, to `until` (Unix seconds:
/// where the recording's own parts begin, when it has some) or else the
/// newest packet.
///
/// `cache` is what mpv holds, `(oldest, newest)` in the stream's time; the
/// newest packet arrived a moment ago, at `now_ms`, and the stream's time
/// runs with the wall clock, so each moment in it is placed by its distance
/// from the newest.
pub fn keep(cache: (f64, f64), now_ms: i64, from: i64, until: Option<i64>) -> Option<Keep> {
    let (oldest, newest) = cache;
    if !(oldest.is_finite() && newest.is_finite()) || newest <= oldest {
        return None;
    }
    // The stream's time at a wall-clock moment.
    let at = |unix_ms: i64| newest - (now_ms - unix_ms) as f64 / 1000.0;
    let start = oldest.max(at(from.saturating_mul(1000)));
    let end = until.map_or(newest, |u| newest.min(at(u.saturating_mul(1000))));
    if end - start < KEEP_AT_LEAST_SECS {
        return None;
    }
    Some(Keep {
        start,
        end,
        first_ms: now_ms - ((newest - start) * 1000.0).round() as i64,
    })
}

/// The channels whose connection the service shares at no cost, from its
/// recordings: those capturing now (not waiting for a connection), on the
/// feed their last part came from.
pub fn captured_feeds(recs: &[super::api::RecordingDto]) -> Vec<i64> {
    let mut out: Vec<i64> = recs
        .iter()
        .filter(|r| r.state == super::api::RecordingState::Recording && r.waiting.is_none())
        .map(|r| r.parts.last().map_or(r.stream_id, |p| p.stream_id))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            watching_direct: true,
            starts_now: true,
            room: false,
            captured: false,
            service: true,
        }
    }

    #[test]
    fn a_full_account_moves_the_player_onto_the_services_connection() {
        assert_eq!(plan(facts()), Plan::Handover);
    }

    #[test]
    fn an_account_with_room_changes_nothing() {
        let f = Facts { room: true, ..facts() };
        assert_eq!(plan(f), Plan::Alone);
        // Nor without a service: scheduling says it is not running.
        assert_eq!(plan(Facts { service: false, ..f }), Plan::Alone);
    }

    #[test]
    fn a_full_account_with_no_service_here_says_so() {
        assert_eq!(plan(Facts { service: false, ..facts() }), Plan::NoService);
    }

    #[test]
    fn a_channel_the_service_already_records_is_joined_whatever_the_room() {
        let f = Facts {
            room: true,
            captured: true,
            ..facts()
        };
        assert_eq!(plan(f), Plan::Handover);
    }

    #[test]
    fn only_a_recording_of_what_plays_now_concerns_the_player() {
        // Another channel, a catch-up programme, or the player already on
        // the service's connection.
        let other = Facts {
            watching_direct: false,
            ..facts()
        };
        assert_eq!(plan(other), Plan::Alone);
        assert_eq!(plan(Facts { service: false, ..other }), Plan::Alone);
        // A programme later on: the player is left alone now.
        let later = Facts {
            starts_now: false,
            ..facts()
        };
        assert_eq!(plan(later), Plan::Alone);
    }

    const NOW: i64 = 1_793_736_000_000;

    #[test]
    fn the_buffer_is_kept_from_the_padded_start() {
        // Ten minutes buffered (stream time 100..700, the newest arriving
        // now); the programme's padded start was four minutes ago.
        let k = keep((100.0, 700.0), NOW, NOW / 1000 - 240, None).unwrap();
        assert!((k.start - 460.0).abs() < 1e-6, "{k:?}");
        assert_eq!(k.end, 700.0);
        assert_eq!(k.first_ms, NOW - 240_000);
    }

    #[test]
    fn a_buffer_that_does_not_reach_back_far_enough_is_kept_whole() {
        let k = keep((100.0, 160.0), NOW, NOW / 1000 - 3600, None).unwrap();
        assert_eq!(k.start, 100.0);
        assert_eq!(k.first_ms, NOW - 60_000);
    }

    #[test]
    fn it_stops_where_the_recording_already_has_parts() {
        // The recording began capturing a minute ago.
        let k = keep((100.0, 700.0), NOW, NOW / 1000 - 240, Some(NOW / 1000 - 60)).unwrap();
        assert!((k.end - 640.0).abs() < 1e-6, "{k:?}");
    }

    #[test]
    fn a_few_seconds_or_nothing_are_not_kept() {
        // The programme starts in the future, or just now.
        assert_eq!(keep((100.0, 700.0), NOW, NOW / 1000 + 60, None), None);
        assert_eq!(keep((100.0, 700.0), NOW, NOW / 1000 - 1, None), None);
        // The recording has captured all the buffer covers.
        assert_eq!(
            keep((100.0, 160.0), NOW, NOW / 1000 - 3600, Some(NOW / 1000 - 120)),
            None
        );
        assert_eq!(keep((f64::NAN, 160.0), NOW, 0, None), None);
    }

    fn rec(id: i64, state: super::super::api::RecordingState) -> super::super::api::RecordingDto {
        serde_json::from_value(serde_json::json!({
            "id": id, "streamId": 10 + id, "channelName": "C", "title": "T",
            "start": 0, "stop": 1, "padBefore": 0, "padAfter": 0,
            "state": state.as_str(), "parts": [], "notes": [], "folder": "/x",
        }))
        .unwrap()
    }

    #[test]
    fn the_feeds_shared_for_free_are_those_being_captured() {
        use super::super::api::{PartDto, RecordingState};
        let mut on_backup = rec(1, RecordingState::Recording);
        on_backup.parts.push(PartDto {
            path: "/x/a.ts".into(),
            stream_id: 99,
            bytes: 1,
            started: 0,
            ended: None,
            handed_over: false,
        });
        let mut waiting = rec(2, RecordingState::Recording);
        waiting.waiting = Some("waiting for a free connection".into());
        let recs = [
            on_backup,
            waiting,
            rec(3, RecordingState::Recording),
            rec(4, RecordingState::Scheduled),
            rec(5, RecordingState::Finishing),
        ];
        assert_eq!(captured_feeds(&recs), vec![13, 99]);
    }
}
