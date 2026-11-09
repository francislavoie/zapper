//! Diffing sync. Never truncates: `channel_quality` and `activity` rows are keyed on
//! `stream_id` and must survive a refresh, and channels must not flicker out
//! of the UI while a sync runs.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::xtream::{Category, Channel};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub inserted: usize,
    pub updated: usize,
    pub deleted: usize,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Strips the decorative tags servers bury in names: `|TR|`, `[FHD]`,
/// `(Backup)` and the unicode superscript markers some servers favour.
pub fn clean_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth = 0i32;
    // Pipes come in pairs and do not nest, so they toggle rather than nest
    // like brackets do. Counting them as depth swallows the delimiter but
    // keeps the tag between them.
    let mut piped = false;
    for ch in raw.chars() {
        match ch {
            '|' => piped = !piped,
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 && !piped => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn sync_categories(conn: &Connection, cats: &[Category]) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let tx = conn.unchecked_transaction()?;
    for (i, c) in cats.iter().enumerate() {
        let changed = tx.execute(
            "INSERT INTO categories (id, name, sort) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, sort=excluded.sort
             WHERE name<>excluded.name OR sort<>excluded.sort",
            params![c.category_id, c.category_name, i as i64],
        )?;
        if changed > 0 {
            stats.inserted += 1;
        }
    }
    tx.commit()?;
    Ok(stats)
}

pub fn sync_channels(conn: &Connection, chans: &[Channel]) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    let tx = conn.unchecked_transaction()?;

    let existing: HashSet<i64> = {
        let mut s = tx.prepare("SELECT stream_id FROM channels WHERE deleted_at IS NULL")?;
        // Bound to a local so the statement outlives the row iterator; as a
        // tail expression the temporary would drop after `s`.
        let ids = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids
    };
    let mut seen = HashSet::with_capacity(chans.len());

    for c in chans {
        seen.insert(c.stream_id);
        let is_new = !existing.contains(&c.stream_id);
        tx.execute(
            "INSERT INTO channels
               (stream_id, category_id, name, clean_name, num, logo_url,
                epg_channel_id, has_archive, archive_days, is_separator, deleted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL)
             ON CONFLICT(stream_id) DO UPDATE SET
               category_id=excluded.category_id, name=excluded.name,
               clean_name=excluded.clean_name, num=excluded.num,
               logo_url=excluded.logo_url, epg_channel_id=excluded.epg_channel_id,
               has_archive=excluded.has_archive, archive_days=excluded.archive_days,
               is_separator=excluded.is_separator, deleted_at=NULL",
            params![
                c.stream_id,
                c.category_id.map(|v| v.to_string()),
                c.name,
                clean_name(&c.name),
                c.num,
                c.stream_icon,
                c.epg_channel_id,
                c.tv_archive as i64,
                c.tv_archive_duration(),
                c.is_separator() as i64,
            ],
        )?;
        if is_new {
            stats.inserted += 1;
        } else {
            stats.updated += 1;
        }
    }

    for gone in existing.difference(&seen) {
        tx.execute(
            "UPDATE channels SET deleted_at=?1 WHERE stream_id=?2",
            params![now(), gone],
        )?;
        stats.deleted += 1;
    }

    tx.commit()?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::schema::open_in_memory;
    use crate::xtream::Channel;

    fn ch(id: i64, name: &str) -> Channel {
        serde_json::from_str(&format!(
            r#"{{"stream_id":{id},"name":"{name}","category_id":"1"}}"#
        ))
        .unwrap()
    }

    #[test]
    fn first_sync_inserts() {
        let c = open_in_memory().unwrap();
        let s = sync_channels(&c, &[ch(1, "A"), ch(2, "B")]).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (2, 0, 0));
    }

    #[test]
    fn resync_updates_rather_than_reinserting() {
        let c = open_in_memory().unwrap();
        sync_channels(&c, &[ch(1, "A")]).unwrap();
        let s = sync_channels(&c, &[ch(1, "A renamed")]).unwrap();
        assert_eq!((s.inserted, s.updated, s.deleted), (0, 1, 0));
        let n: String = c
            .query_row("SELECT name FROM channels WHERE stream_id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, "A renamed");
    }

    #[test]
    fn vanished_channels_are_soft_deleted_not_removed() {
        let c = open_in_memory().unwrap();
        sync_channels(&c, &[ch(1, "A"), ch(2, "B")]).unwrap();
        let s = sync_channels(&c, &[ch(1, "A")]).unwrap();
        assert_eq!(s.deleted, 1);
        let alive: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM channels WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alive, 1);
        let total: i64 = c
            .query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2, "row must survive so quality/activity stay joinable");
    }

    #[test]
    fn resync_preserves_quality_and_activity() {
        let c = open_in_memory().unwrap();
        sync_channels(&c, &[ch(1, "A")]).unwrap();
        c.execute(
            "INSERT INTO channel_quality (origin, stream_id, codec, width, height)
             VALUES ('',1,'h264',1920,1080)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO activity (stream_id, watch_seconds) VALUES (1, 3600)",
            [],
        )
        .unwrap();
        sync_channels(&c, &[ch(1, "A")]).unwrap();
        let w: i64 = c
            .query_row("SELECT width FROM channel_quality WHERE stream_id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let s: i64 = c
            .query_row(
                "SELECT watch_seconds FROM activity WHERE stream_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((w, s), (1920, 3600));
    }

    #[test]
    fn returning_channel_is_undeleted() {
        let c = open_in_memory().unwrap();
        sync_channels(&c, &[ch(1, "A")]).unwrap();
        sync_channels(&c, &[]).unwrap();
        sync_channels(&c, &[ch(1, "A")]).unwrap();
        let d: Option<i64> = c
            .query_row("SELECT deleted_at FROM channels WHERE stream_id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(d, None);
    }

    #[test]
    fn separators_are_flagged_on_write() {
        let c = open_in_memory().unwrap();
        sync_channels(&c, &[ch(1, "###### SPORTS ######"), ch(2, "Northwind One")]).unwrap();
        let f: i64 = c
            .query_row("SELECT is_separator FROM channels WHERE stream_id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        let g: i64 = c
            .query_row("SELECT is_separator FROM channels WHERE stream_id=2", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!((f, g), (1, 0));
    }

    #[test]
    fn clean_name_strips_decorative_tags() {
        // Servers bury country pipes, quality brackets and parenthesised
        // notes in names; the picker should see none of them.
        assert_eq!(clean_name("|CA| NWB  [FHD] (Backup)"), "NWB");
        assert_eq!(clean_name("Northwind One"), "Northwind One");
    }
}
