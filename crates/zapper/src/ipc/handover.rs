//! Recording what the player is watching (docs/recording.md, "Recording
//! what you watch"), the app's side: Record on the channel the player shows
//! moves the player onto the recording service's connection when the
//! account has none to spare, and hands the service what the player had
//! already received of the programme. The decisions are
//! `recording::handover`'s; this does them, with mpv and the service's API.
//!
//! It also remembers which channels the service is recording, from the
//! list the interface fetches anyway, so that tuning to one of them joins
//! the service's connection ([`captured`]) without asking the service on
//! every channel change; and when a recording of the channel being watched
//! starts later and waits for the connection the player holds, it moves the
//! player then ([`noticed`]).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::connections;
use crate::player::controller::{self, NowPlaying};
use crate::recording::api::{RecordingDto, RecordingState, ScheduleRequest};
use crate::recording::client::{Client, NOT_RUNNING};
use crate::recording::handover::{self, Facts, Plan};
use crate::recording::schedule::window;
use crate::recording::{now, now_ms};

/// How long the recordings last fetched say what the service records: the
/// interface fetches them every few seconds while anything records, and
/// every half minute otherwise.
const FRESH: Duration = Duration::from_secs(45);

/// The channels the service shares a connection to, and when that was
/// learnt.
static CAPTURED: Mutex<Option<(Instant, Vec<i64>)>> = Mutex::new(None);

/// Recordings the player was already moved for, or tried to be: once each.
static MOVED_FOR: Mutex<Option<HashSet<i64>>> = Mutex::new(None);

/// Whether the service on this computer is recording `stream_id` now, by
/// the last list fetched.
pub fn captured(stream_id: i64) -> bool {
    CAPTURED
        .lock()
        .ok()
        .and_then(|c| c.as_ref().map(|(at, feeds)| at.elapsed() < FRESH && feeds.contains(&stream_id)))
        .unwrap_or(false)
}

/// What the service's list of recordings says, as it is fetched.
pub fn noticed(list: &[RecordingDto]) {
    if let Ok(mut c) = CAPTURED.lock() {
        *c = Some((Instant::now(), handover::captured_feeds(list)));
    }
    // A recording of the channel being watched that waits for the
    // connection the player holds: it would wait until the player stops.
    let Some(p) = controller::now_playing().filter(|p| p.live && !p.through_service) else {
        return;
    };
    let Some(rec) = list
        .iter()
        .find(|r| {
            r.state == RecordingState::Recording
                && r.waiting.is_some()
                && (r.stream_id == p.stream_id || r.backup_stream_id == Some(p.stream_id))
        })
        .cloned()
    else {
        return;
    };
    let first = MOVED_FOR
        .lock()
        .map(|mut m| m.get_or_insert_with(HashSet::new).insert(rec.id))
        .unwrap_or(false);
    if !first {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let Ok(client) = Client::local() else { return };
        let facts = Facts {
            watching_direct: true,
            starts_now: true,
            room: room().await,
            captured: false,
            service: true,
        };
        if handover::plan(facts) != Plan::Handover {
            return;
        }
        tracing::info!("recording {} waits for the player's connection: moving the player", rec.id);
        let feeds: Vec<i64> = std::iter::once(rec.stream_id).chain(rec.backup_stream_id).collect();
        let (from, _) = window(rec.start, rec.stop, rec.pad_before, rec.pad_after);
        let until = rec.parts.iter().find(|part| part.bytes > 0).map(|part| part.started);
        let kept = keep_buffer(p, &feeds, from, until).await;
        if let Err(e) = controller::move_to_service(&client, p.stream_id).await {
            tracing::warn!("recording {}: {e}", rec.id);
        }
        if let Some(k) = kept {
            hand_buffer_over(client, rec.id, k);
        }
    });
}

/// Whether the account has a connection beyond the player's, asked afresh.
async fn room() -> bool {
    let declared = crate::catalog::settings::load_or_default().simultaneous_streams;
    let limit = match controller::source() {
        Ok(src) => connections::look(src, declared).await.limit(),
        Err(_) => connections::Limit::Unknown,
    };
    connections::may_open_another(limit, connections::held())
}

/// Schedules a programme, and when it is on now on the channel the player
/// shows: keeps what the player has of it, and moves the player onto the
/// service's connection when the account has none to spare.
///
/// The player moves before the recording is scheduled, so the recording
/// finds the service's connection open and rides it from its first moment:
/// scheduled first, it could connect for itself where the service cannot
/// count the player's connection (a playlist's "Simultaneous streams"), and
/// the provider would end one of the two.
pub async fn record(req: ScheduleRequest) -> Result<RecordingDto, String> {
    let client = Client::local().map_err(|e| format!("{e:#}"))?;
    let t = now();
    let here = controller::now_playing().filter(|p| p.stream_id == req.stream_id && p.live);
    let direct = here.is_some_and(|p| !p.through_service);
    // When the recording starts, its padding included: the service's
    // setting unless the request says.
    let (pad_before, service) = if here.is_some() {
        match client.settings().await {
            Ok(s) => (req.pad_before.unwrap_or(s.pad_before), true),
            Err(e) => (req.pad_before.unwrap_or(0), format!("{e:#}") != NOT_RUNNING),
        }
    } else {
        (0, true)
    };
    let from = req.start - pad_before.max(0);
    let starts_now = from <= t && t < req.stop;
    let room = if direct && starts_now { room().await } else { true };
    let plan = handover::plan(Facts {
        watching_direct: direct,
        starts_now,
        room,
        captured: captured(req.stream_id),
        service,
    });
    if plan == Plan::NoService {
        return Err(handover::NO_SERVICE.to_string());
    }
    // What the player has of it first: moving the player empties its buffer.
    let kept = match here {
        Some(p) if starts_now && service => keep_buffer(p, &[req.stream_id], from, None).await,
        _ => None,
    };
    let moved = if plan == Plan::Handover {
        controller::move_to_service(&client, req.stream_id).await
    } else {
        Ok(())
    };
    let rec = match client.schedule(&req).await {
        Ok(r) => r,
        Err(e) => {
            if let Some(k) = kept {
                let _ = std::fs::remove_file(&k.path);
            }
            return Err(format!("{e:#}"));
        }
    };
    if let Ok(mut m) = MOVED_FOR.lock() {
        m.get_or_insert_with(HashSet::new).insert(rec.id);
    }
    if let Some(k) = kept {
        // Already being recorded from before what the buffer holds (Record
        // on a programme that was): nothing to add.
        let covered = rec
            .parts
            .first()
            .is_some_and(|part| part.started * 1000 <= k.first_ms + 3000);
        if covered || !matches!(rec.state, RecordingState::Scheduled | RecordingState::Recording) {
            let _ = std::fs::remove_file(&k.path);
        } else {
            hand_buffer_over(client, rec.id, k);
        }
    }
    match moved {
        Ok(()) => Ok(rec),
        Err(e) => Err(format!(
            "“{}” is scheduled, but the player could not share the recording service's connection ({e}), so it waits until you stop watching",
            rec.title
        )),
    }
}

/// The player's buffer of a programme, written out to a file.
struct Kept {
    path: PathBuf,
    first_ms: i64,
}

/// Writes out what mpv's cache holds of a programme recorded from `from`
/// (Unix seconds, its padded start; or as far back as the cache reaches) up
/// to `until` (where the recording's own parts begin, if they have), while
/// `p` plays one of the recording's channels (`feeds`). `None` when the
/// cache has nothing worth keeping, or mpv could not write it.
async fn keep_buffer(p: NowPlaying, feeds: &[i64], from: i64, until: Option<i64>) -> Option<Kept> {
    if !feeds.contains(&p.stream_id) {
        return None;
    }
    let mpv = crate::surface::mpv::handle()?;
    let span = crate::surface::mpv::cache_span(mpv)?;
    let k = handover::keep(span, now_ms(), from, until)?;
    let dir = crate::paths::cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("record-buffer-{}-{}.ts", now_ms(), std::process::id()));
    let out = path.clone();
    let written = tokio::task::spawn_blocking(move || {
        crate::surface::mpv::dump_cache(mpv, k.start, k.end, &out)
    })
    .await;
    match written {
        Ok(Ok(())) if std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) => {
            tracing::info!("kept {:.0} s of the player's buffer", k.end - k.start);
            Some(Kept {
                path,
                first_ms: k.first_ms,
            })
        }
        other => {
            if let Ok(Err(e)) = other {
                tracing::warn!("could not write out the player's buffer: {e:#}");
            }
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// Hands the buffer to the service as the recording's first part, in the
/// background (it is read through once there), and deletes it here.
fn hand_buffer_over(client: Client, id: i64, k: Kept) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = client.prelude(id, &k.path, k.first_ms).await {
            tracing::warn!("recording {id}: the player's buffer was not taken: {e:#}");
        }
        let _ = tokio::fs::remove_file(&k.path).await;
    });
}
