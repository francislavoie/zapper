//! The service's recordings, in their own SQLite file.
//!
//! `recordings.db`, beside the catalogue rather than inside it: the service
//! is its only writer (the app asks the service over HTTP and never opens
//! it), so a recording's state changes never queue behind the catalogue's
//! long sync transactions, and the catalogue's schema is untouched.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::api::{PartDto, RecordingDto, RecordingState, ScheduleRequest};
use super::schedule::{window, Slot};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS recordings (
    id               INTEGER PRIMARY KEY,
    stream_id        INTEGER NOT NULL,
    backup_stream_id INTEGER,
    channel_name     TEXT NOT NULL,
    title            TEXT NOT NULL,
    episode_title    TEXT,
    season           INTEGER,
    episode          INTEGER,
    start            INTEGER NOT NULL,
    stop             INTEGER NOT NULL,
    pad_before       INTEGER NOT NULL,
    pad_after        INTEGER NOT NULL,
    state            TEXT NOT NULL,
    -- JSON arrays: PartDto, and notes as strings.
    parts            TEXT NOT NULL DEFAULT '[]',
    notes            TEXT NOT NULL DEFAULT '[]',
    error            TEXT,
    -- `<output>/<Show>` and the file name stem, fixed when it is scheduled
    -- so a change of output folder never splits one recording across two.
    folder           TEXT NOT NULL,
    base             TEXT NOT NULL,
    clock_track      INTEGER NOT NULL DEFAULT 1,
    created          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS recordings_by_state ON recordings(state, start);
"#;

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch(SCHEMA)
        .context("creating the recordings table")?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// What a new recording is, once the service has filled in what the request
/// left out.
pub struct NewRecording<'a> {
    pub req: &'a ScheduleRequest,
    pub backup_stream_id: Option<i64>,
    pub channel_name: &'a str,
    pub pad_before: i64,
    pub pad_after: i64,
    pub folder: &'a str,
    pub base: &'a str,
    pub clock_track: bool,
    pub now: i64,
}

pub fn insert(conn: &Connection, n: &NewRecording<'_>) -> Result<i64> {
    let r = n.req;
    conn.execute(
        "INSERT INTO recordings (stream_id, backup_stream_id, channel_name, title, episode_title,
             season, episode, start, stop, pad_before, pad_after, state, folder, base,
             clock_track, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            r.stream_id,
            n.backup_stream_id,
            n.channel_name,
            r.title,
            r.episode_title,
            r.season,
            r.episode,
            r.start,
            r.stop,
            n.pad_before,
            n.pad_after,
            RecordingState::Scheduled.as_str(),
            n.folder,
            n.base,
            n.clock_track,
            n.now,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// A recording of the same programme on the same channel, if one exists:
/// asking twice must not record twice.
pub fn find(conn: &Connection, stream_id: i64, start: i64) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM recordings WHERE stream_id = ?1 AND start = ?2",
            params![stream_id, start],
            |r| r.get(0),
        )
        .optional()?)
}

const COLUMNS: &str = "id, stream_id, backup_stream_id, channel_name, title, episode_title,
    season, episode, start, stop, pad_before, pad_after, state, parts, notes, error, folder";

fn from_row(r: &Row<'_>) -> rusqlite::Result<RecordingDto> {
    let state: String = r.get(12)?;
    let parts: String = r.get(13)?;
    let notes: String = r.get(14)?;
    Ok(RecordingDto {
        id: r.get(0)?,
        stream_id: r.get(1)?,
        backup_stream_id: r.get(2)?,
        channel_name: r.get(3)?,
        title: r.get(4)?,
        episode_title: r.get(5)?,
        season: r.get(6)?,
        episode: r.get(7)?,
        start: r.get(8)?,
        stop: r.get(9)?,
        pad_before: r.get(10)?,
        pad_after: r.get(11)?,
        state: RecordingState::parse(&state).unwrap_or(RecordingState::Failed),
        parts: serde_json::from_str(&parts).unwrap_or_default(),
        notes: serde_json::from_str(&notes).unwrap_or_default(),
        error: r.get(15)?,
        folder: r.get(16)?,
        backup_name: None,
        waiting: None,
        conflicts: Vec::new(),
    })
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<RecordingDto>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM recordings WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()?)
}

/// Every recording, soonest first.
pub fn list(conn: &Connection) -> Result<Vec<RecordingDto>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM recordings ORDER BY start, id"
    ))?;
    let rows = st
        .query_map([], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// What the scheduler needs, for the recordings that may still need it.
pub fn slots(conn: &Connection) -> Result<Vec<Slot>> {
    let mut st = conn.prepare(
        "SELECT id, state, start, stop, pad_before, pad_after FROM recordings
         WHERE state IN ('scheduled', 'recording', 'finishing')",
    )?;
    let rows = st
        .query_map([], |r| {
            let state: String = r.get(1)?;
            let (from, until) = window(r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
            Ok(Slot {
                id: r.get(0)?,
                state: RecordingState::parse(&state).unwrap_or(RecordingState::Failed),
                from,
                until,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The capture's own facts about a recording, which the API does not show.
pub struct CaptureInfo {
    pub base: String,
    pub clock_track: bool,
}

pub fn capture_info(conn: &Connection, id: i64) -> Result<CaptureInfo> {
    conn.query_row(
        "SELECT base, clock_track FROM recordings WHERE id = ?1",
        [id],
        |r| {
            Ok(CaptureInfo {
                base: r.get(0)?,
                clock_track: r.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| anyhow!("recording {id} is gone"))
}

pub fn set_state(
    conn: &Connection,
    id: i64,
    state: RecordingState,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE recordings SET state = ?2, error = ?3 WHERE id = ?1",
        params![id, state.as_str(), error],
    )?;
    Ok(())
}

/// Changes the state alone, keeping the error on record.
pub fn mark(conn: &Connection, id: i64, state: RecordingState) -> Result<()> {
    conn.execute(
        "UPDATE recordings SET state = ?2 WHERE id = ?1",
        params![id, state.as_str()],
    )?;
    Ok(())
}

/// Keeps why a recording has nothing to show, without changing its state.
pub fn set_error(conn: &Connection, id: i64, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE recordings SET error = ?2 WHERE id = ?1",
        params![id, error],
    )?;
    Ok(())
}

pub fn set_parts(conn: &Connection, id: i64, parts: &[PartDto]) -> Result<()> {
    conn.execute(
        "UPDATE recordings SET parts = ?2 WHERE id = ?1",
        params![id, serde_json::to_string(parts)?],
    )?;
    Ok(())
}

pub fn add_note(conn: &Connection, id: i64, note: &str) -> Result<()> {
    let Some(rec) = get(conn, id)? else {
        return Ok(());
    };
    let mut notes = rec.notes;
    // A recording that flaps would otherwise collect hundreds; the first
    // ones say what happened, and the count says how often.
    const MAX_NOTES: usize = 50;
    if notes.len() >= MAX_NOTES {
        return Ok(());
    }
    notes.push(note.to_string());
    conn.execute(
        "UPDATE recordings SET notes = ?2 WHERE id = ?1",
        params![id, serde_json::to_string(&notes)?],
    )?;
    Ok(())
}

/// The recording's backup feed now: read again before each connection, so
/// a change while it records is used from its next fallback decision.
pub fn backup_of(conn: &Connection, id: i64) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT backup_stream_id FROM recordings WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}

pub fn set_backup(conn: &Connection, id: i64, backup: Option<i64>) -> Result<()> {
    conn.execute(
        "UPDATE recordings SET backup_stream_id = ?2 WHERE id = ?1",
        params![id, backup],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM recordings WHERE id = ?1", [id])? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(stream_id: i64, start: i64) -> ScheduleRequest {
        ScheduleRequest {
            stream_id,
            backup_stream_id: None,
            channel_name: None,
            title: "News".into(),
            episode_title: None,
            season: Some(1),
            episode: Some(2),
            start,
            stop: start + 1800,
            pad_before: None,
            pad_after: None,
        }
    }

    fn add(c: &Connection, r: &ScheduleRequest) -> i64 {
        insert(
            c,
            &NewRecording {
                req: r,
                backup_stream_id: Some(9),
                channel_name: "One",
                pad_before: 60,
                pad_after: 300,
                folder: "/v/News",
                base: "News - S01E02 - 2026-11-03 20.00",
                clock_track: true,
                now: 1,
            },
        )
        .unwrap()
    }

    #[test]
    fn recordings_survive_as_rows_and_change_state() {
        let c = open_in_memory().unwrap();
        let id = add(&c, &req(5, 1000));
        let r = get(&c, id).unwrap().unwrap();
        assert_eq!(
            (r.state, r.backup_stream_id, r.season),
            (RecordingState::Scheduled, Some(9), Some(1))
        );
        assert_eq!(slots(&c).unwrap()[0].from, 940);
        assert_eq!(find(&c, 5, 1000).unwrap(), Some(id));

        set_backup(&c, id, Some(12)).unwrap();
        assert_eq!(backup_of(&c, id).unwrap(), Some(12));
        set_backup(&c, id, None).unwrap();
        assert_eq!(backup_of(&c, id).unwrap(), None);
        assert_eq!(backup_of(&c, id + 100).unwrap(), None);

        set_state(&c, id, RecordingState::Recording, None).unwrap();
        let part = PartDto {
            path: "/v/News/x.ts".into(),
            stream_id: 5,
            bytes: 10,
            started: 940,
            ended: None,
        };
        set_parts(&c, id, std::slice::from_ref(&part)).unwrap();
        add_note(&c, id, "switched").unwrap();
        let r = get(&c, id).unwrap().unwrap();
        assert_eq!(
            (r.parts, r.notes),
            (vec![part], vec!["switched".to_string()])
        );

        set_error(&c, id, Some("no data")).unwrap();
        mark(&c, id, RecordingState::Finishing).unwrap();
        let r = get(&c, id).unwrap().unwrap();
        assert_eq!((r.state, r.error.as_deref()), (RecordingState::Finishing, Some("no data")));
        assert_eq!(slots(&c).unwrap()[0].state, RecordingState::Finishing, "still the scheduler's");

        set_state(&c, id, RecordingState::Failed, Some("nothing")).unwrap();
        assert!(
            slots(&c).unwrap().is_empty(),
            "finished recordings are not scheduled"
        );
        assert_eq!(
            get(&c, id).unwrap().unwrap().error.as_deref(),
            Some("nothing")
        );
        assert_eq!(
            capture_info(&c, id).unwrap().base,
            "News - S01E02 - 2026-11-03 20.00"
        );

        assert!(delete(&c, id).unwrap());
        assert!(list(&c).unwrap().is_empty());
    }

    #[test]
    fn listed_soonest_first() {
        let c = open_in_memory().unwrap();
        add(&c, &req(1, 3000));
        add(&c, &req(2, 1000));
        let starts: Vec<i64> = list(&c).unwrap().iter().map(|r| r.start).collect();
        assert_eq!(starts, [1000, 3000]);
    }
}
