//! Catalogue reads and background sync.
//!
//! Every command here must answer from SQLite alone. Nothing on this path may
//! block on the network: startup latency is the whole point of the local
//! mirror, and a week-old cache must open exactly as fast as a fresh one.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::catalog::settings as settings_store;
use crate::catalog::{db, index, status, store, sync_engine};
use crate::ipc::types::{
    CategoryDto, ChannelDto, GuideRowDto, ProgrammeDto, SettingsDto, SyncStatusDto,
};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Attaches what is on air to a page of channels. Done for the page rather
/// than in the listing query so browsing a huge catalogue never joins the
/// guide for rows nobody is looking at.
fn attach_now_playing(conn: &rusqlite::Connection, page: &mut [ChannelDto]) -> anyhow::Result<()> {
    let ids: Vec<i64> = page
        .iter()
        .filter(|c| !c.is_separator)
        .map(|c| c.stream_id)
        .collect();
    let mut playing = store::now_playing(conn, &ids, now())?;
    for ch in page.iter_mut() {
        ch.now_playing = playing.remove(&ch.stream_id);
    }
    Ok(())
}

#[tauri::command]
pub async fn categories() -> Result<Vec<CategoryDto>, String> {
    db::with_db(store::categories).map_err(|e| e.to_string())
}

/// `category_id` of `None` means every channel, ordered by `num`.
#[tauri::command]
pub async fn channels(
    category_id: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<ChannelDto>, String> {
    db::with_db(|conn| {
        let mut page = store::channels_in(conn, category_id.as_deref(), offset, limit)?;
        attach_now_playing(conn, &mut page)?;
        Ok(page)
    })
    .map_err(|e| e.to_string())
}

/// Fuzzy match over all channels. Separators are never returned.
#[tauri::command]
pub async fn search(query: String, limit: u32) -> Result<Vec<ChannelDto>, String> {
    // A blank query means "nothing yet", not "every channel": the picker
    // opens empty and fills as you type.
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    db::with_db(|conn| {
        // The index is built by sync, but search must work before the first
        // sync of the session lands — the mirror on disk is enough.
        if !index::is_built() {
            index::rebuild(conn)?;
        }
        let ids = index::query(&query, limit as usize);
        let mut hits = store::channels_by_ids(conn, &ids)?;
        attach_now_playing(conn, &mut hits)?;
        Ok(hits)
    })
    .map_err(|e| e.to_string())
}

/// Programmes overlapping `[from, to]` for the given channels.
#[tauri::command]
pub async fn guide(
    stream_ids: Vec<i64>,
    from: i64,
    to: i64,
) -> Result<Vec<(i64, Vec<ProgrammeDto>)>, String> {
    db::with_db(|conn| store::programmes(conn, &stream_ids, from, to)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sync_status() -> Result<SyncStatusDto, String> {
    Ok(status::snapshot())
}

/// Kicks a refresh. Returns immediately; progress arrives as events.
///
/// `app` is injected by Tauri and does not appear in the JS argument list.
#[tauri::command]
pub async fn sync_now(app: tauri::AppHandle) -> Result<(), String> {
    sync_engine::spawn(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids are namespaced per test: the process shares one in-memory database
    /// and tests run in parallel.
    fn seed(base: i64, category: &str) {
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO categories (id, name, sort) VALUES (?1, ?1, 0)",
                [category],
            )?;
            for (n, name) in [(1i64, "Alpha"), (2, "Beta")] {
                c.execute(
                    "INSERT INTO channels (stream_id, category_id, name, clean_name, num,
                                           epg_channel_id)
                     VALUES (?1,?2,?3,?3,?4,?5)",
                    rusqlite::params![base + n, category, name, n, format!("epg-{}", base + n)],
                )?;
            }
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn channels_page_carries_what_is_on_air() {
        seed(1000, "cmd-channels");
        let t = now();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO epg (channel_ref, start, stop, title, description)
                 VALUES ('epg-1001', ?1, ?2, 'On now', NULL),
                        ('epg-1002', ?3, ?4, 'Long over', NULL)",
                rusqlite::params![t - 60, t + 60, t - 7200, t - 3600],
            )?;
            Ok(())
        })
        .unwrap();

        let page = channels(Some("cmd-channels".into()), 0, 10).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(
            page[0].now_playing.as_ref().map(|p| p.title.as_str()),
            Some("On now")
        );
        // A programme that finished an hour ago is not what is on.
        assert!(page[1].now_playing.is_none());
    }

    #[tokio::test]
    async fn categories_reports_its_channel_count() {
        seed(2000, "cmd-categories");
        let cats = categories().await.unwrap();
        let mine = cats
            .iter()
            .find(|c| c.id == "cmd-categories")
            .expect("seeded category missing");
        assert_eq!(mine.channel_count, 2);
    }

    #[tokio::test]
    async fn a_blank_query_finds_nothing_rather_than_everything() {
        db::install_test_db();
        assert!(search("".into(), 20).await.unwrap().is_empty());
        assert!(search("   ".into(), 20).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_answers_from_the_mirror_before_any_sync() {
        seed(3000, "cmd-search");
        db::with_db(|c| {
            c.execute(
                "UPDATE channels SET name = 'ZZ Quixotic', clean_name = 'ZZ Quixotic'
                 WHERE stream_id = 3001",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        // Forced, not left to `search`: the index is process-wide, and a
        // rebuild triggered by an earlier test would predate this row.
        db::with_db(index::rebuild).unwrap();

        let hits = search("quixotic".into(), 20).await.unwrap();
        assert_eq!(hits.first().map(|c| c.stream_id), Some(3001));
    }

    #[tokio::test]
    async fn guide_returns_a_row_per_requested_channel() {
        seed(4000, "cmd-guide");
        db::with_db(|c| {
            c.execute(
                "INSERT INTO epg (channel_ref, start, stop, title, description)
                 VALUES ('epg-4001', 100, 200, 'Show', NULL)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let g = guide(vec![4002, 4001], 0, 1000).await.unwrap();
        assert_eq!(g.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [4002, 4001]);
        assert!(g[0].1.is_empty());
        assert_eq!(g[1].1.len(), 1);
    }

    #[tokio::test]
    async fn guide_rows_carry_the_channel_beside_its_programmes() {
        seed(5000, "cmd-guide-rows");
        db::with_db(|c| {
            c.execute(
                "INSERT INTO epg (channel_ref, start, stop, title, description)
                 VALUES ('epg-5001', 100, 200, 'Show', NULL)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let rows = guide_rows(vec![5002, 5001], 0, 1000).await.unwrap();
        assert_eq!(
            rows.iter().map(|r| r.stream_id).collect::<Vec<_>>(),
            [5002, 5001]
        );
        // The row exists even with nothing on it, or the grid's rows and its
        // header stop lining up.
        assert!(rows[0].programmes.is_empty());
        assert_eq!(rows[0].name, "Beta");
        assert_eq!(rows[1].programmes.len(), 1);
        assert_eq!(rows[1].programmes[0].title, "Show");
    }

    #[tokio::test]
    async fn settings_default_before_anything_is_saved_and_round_trip_after() {
        db::install_test_db();
        // The key is process-wide, so this is deliberately the only test that
        // writes it.
        let first = settings().await.unwrap();
        assert_eq!(first.guide_hours, SettingsDto::default().guide_hours);

        let want = SettingsDto {
            guide_hours: 9,
            rewind_minutes: 15.0,
            ..Default::default()
        };
        set_settings(want).await.unwrap();

        let got = settings().await.unwrap();
        assert_eq!(got.guide_hours, 9);
        assert_eq!(got.rewind_minutes, 15.0);
    }

    #[tokio::test]
    async fn sync_status_answers_without_a_network() {
        db::install_test_db();
        let s = sync_status().await.unwrap();
        assert!(s.channels_total >= 0);
    }
}

/// Settings as stored. Defaults on first run; never fails for a missing row.
///
/// Infallible by construction: a missing row, an unparseable blob, or a
/// database that will not open all read as defaults. Settings must never be
/// the reason the app will not open.
#[tauri::command]
pub async fn settings() -> Result<SettingsDto, String> {
    Ok(settings_store::load_or_default())
}

/// Persists settings. Applying the playback-relevant parts to the running
/// player is `apply_playback_settings`, which the frontend calls alongside.
#[tauri::command]
pub async fn set_settings(settings: SettingsDto) -> Result<(), String> {
    db::with_db(|c| settings_store::save(c, &settings)).map_err(|e| e.to_string())
}

/// Guide rows for a window, one per channel, ordered as requested.
///
/// Distinct from `guide`, which returns bare programmes: the grid needs the
/// channel's name, logo and archive flag beside its programmes, and fetching
/// those separately would mean a second round trip per row.
#[tauri::command]
pub async fn guide_rows(
    stream_ids: Vec<i64>,
    from: i64,
    to: i64,
) -> Result<Vec<GuideRowDto>, String> {
    db::with_db(|conn| store::guide_rows(conn, &stream_ids, from, to)).map_err(|e| e.to_string())
}
