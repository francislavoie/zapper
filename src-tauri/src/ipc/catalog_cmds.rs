//! Catalogue reads and background sync.
//!
//! Every command here must answer from SQLite alone. Nothing on this path may
//! block on the network: startup latency is the whole point of the local
//! mirror, and a week-old cache must open exactly as fast as a fresh one.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::catalog::settings as settings_store;
use crate::catalog::{activity, db, index, status, store, sync_engine};
use crate::ipc::types::{
    CategoryDto, ChannelDto, FavouriteDto, GuideRowDto, ProgrammeDto, SettingsDto, SyncStatusDto,
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

/// Specific channels, by stream id, with their now-playing attached.
///
/// Exists for restoring a remembered channel at startup: the id survives a
/// restart but nothing else does, and paging the whole catalogue to find one
/// row would be absurd. Ids that no longer exist are simply absent from the
/// answer rather than an error — a channel can vanish between sessions, and
/// that is a thing to notice quietly rather than fail on.
#[tauri::command]
pub async fn channels_by_ids(ids: Vec<i64>) -> Result<Vec<ChannelDto>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    db::with_db(|conn| {
        let mut rows = store::channels_by_ids(conn, &ids)?;
        attach_now_playing(conn, &mut rows)?;
        Ok(rows)
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
pub async fn sync_now() -> Result<(), String> {
    // Through the periodic loop, not a pass of its own: one schedule, one
    // pass at a time, and a retry resets the failure backoff.
    sync_engine::request_now();
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
    async fn favourites_rank_by_recency_weighted_watch_time() {
        seed(6000, "cmd-favourites");
        let t = now();
        db::with_db(|c| {
            // Alpha: a long binge two months ago. Beta: an hour this week.
            store::record_activity(c, 6001, 20 * 3600, true, t - 56 * 86_400)?;
            store::record_activity(c, 6002, 2 * 3600, true, t - 86_400)?;
            Ok(())
        })
        .unwrap();

        // A generous limit deliberately: the process shares one in-memory
        // database and tests run in parallel, so this assertion is about the
        // order of *these* two rows, not about the whole table.
        let top = favourites(500).await.unwrap();
        let mine: Vec<i64> = top
            .iter()
            .map(|f| f.channel.stream_id)
            .filter(|id| (6000..7000).contains(id))
            .collect();
        assert_eq!(mine, [6002, 6001], "March must not outrank this week");
        let beta = top.iter().find(|f| f.channel.stream_id == 6002).unwrap();
        assert_eq!(beta.watch_seconds, 2 * 3600);
        assert_eq!(beta.open_count, 1);
        assert_eq!(beta.channel.clean_name, "Beta");
    }

    #[tokio::test]
    async fn favourites_is_empty_rather_than_failing_before_anything_is_watched() {
        db::install_test_db();
        // Nothing has ever been watched on *this* channel, and the command
        // must not be the thing that says so with an error.
        seed(7000, "cmd-favourites-empty");
        let top = favourites(50).await.unwrap();
        assert!(!top.iter().any(|f| (7000..8000).contains(&f.channel.stream_id)));
    }

    #[tokio::test]
    async fn favourites_never_offers_a_separator_or_a_deleted_channel() {
        seed(8000, "cmd-favourites-hidden");
        let t = now();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO channels (stream_id, category_id, name, clean_name, is_separator)
                 VALUES (8003, 'cmd-favourites-hidden', '###### X ######', 'X', 1)",
                [],
            )?;
            c.execute(
                "UPDATE channels SET deleted_at = 1 WHERE stream_id = 8002",
                [],
            )?;
            for id in [8001i64, 8002, 8003] {
                store::record_activity(c, id, 4 * 3600, true, t - 60)?;
            }
            Ok(())
        })
        .unwrap();

        let ids: Vec<i64> = favourites(50)
            .await
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .filter(|id| (8000..9000).contains(id))
            .collect();
        assert_eq!(ids, [8001]);
    }

    #[tokio::test]
    async fn favourites_carry_what_is_on_air() {
        seed(9000, "cmd-favourites-epg");
        let t = now();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO epg (channel_ref, start, stop, title, description)
                 VALUES ('epg-9001', ?1, ?2, 'On now', NULL)",
                rusqlite::params![t - 60, t + 60],
            )?;
            store::record_activity(c, 9001, 3600, true, t - 60)?;
            Ok(())
        })
        .unwrap();

        let top = favourites(50).await.unwrap();
        let mine = top.iter().find(|f| f.channel.stream_id == 9001).unwrap();
        assert_eq!(
            mine.channel.now_playing.as_ref().map(|p| p.title.as_str()),
            Some("On now")
        );
    }

    #[tokio::test]
    async fn forget_activity_drops_one_channel_and_leaves_the_rest() {
        seed(11000, "cmd-forget");
        let t = now();
        db::with_db(|c| {
            store::record_activity(c, 11001, 40 * 3600, true, t - 60)?;
            store::record_activity(c, 11002, 3600, true, t - 60)?;
            Ok(())
        })
        .unwrap();

        forget_activity(11001).await.unwrap();
        let ids: Vec<i64> = favourites(50)
            .await
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .filter(|id| (11000..12000).contains(id))
            .collect();
        assert_eq!(ids, [11002]);
        // Twice is fine: forgetting what is already forgotten is not an error.
        forget_activity(11001).await.unwrap();
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
/// The Xtream account, without the password.
#[tauri::command]
pub async fn account() -> Result<crate::ipc::types::AccountDto, String> {
    use crate::catalog::credentials::{account_path, summary, Source};
    let (creds, source) = summary();
    Ok(crate::ipc::types::AccountDto {
        host: creds.as_ref().map(|c| c.host.clone()).unwrap_or_default(),
        username: creds.as_ref().map(|c| c.username.clone()).unwrap_or_default(),
        has_password: creds.as_ref().is_some_and(|c| !c.password.is_empty()),
        source: match source {
            Source::Zapper => "zapper",
            Source::None => "none",
        }
        .to_owned(),
        path: account_path().display().to_string(),
    })
}

/// Replaces the stored account and drops the cached client, so the next
/// request goes to the new server without a restart.
///
/// An empty password means "keep the one already on file", which is what
/// lets the interface show the field as set without ever being given the
/// value to send back.
#[tauri::command]
pub async fn set_account(
    host: String,
    username: String,
    password: String,
) -> Result<(), String> {
    use crate::catalog::credentials;
    use crate::xtream::Credentials;

    let host = host.trim().to_owned();
    let username = username.trim().to_owned();
    if host.is_empty() {
        return Err("a host is required".to_owned());
    }
    if username.is_empty() {
        return Err("a username is required".to_owned());
    }

    let password = if password.is_empty() {
        let (existing, _) = credentials::summary();
        match existing.map(|c| c.password).filter(|p| !p.is_empty()) {
            Some(p) => p,
            None => return Err("a password is required".to_owned()),
        }
    } else {
        password
    };

    credentials::save(&Credentials {
        host,
        username,
        password,
    })
    .map_err(|e| e.to_string())?;
    crate::ipc::player_cmds::forget_panel();
    // New credentials are the thing a refused sync was waiting for.
    sync_engine::request_now();
    Ok(())
}

/// Asks the server who we are. The one honest test of an account: it either
/// answers or it does not.
///
/// The message is scrubbed, because a failure from `reqwest` quotes the URL
/// it was given and that URL carries the username and password.
#[tauri::command]
pub async fn test_account() -> Result<String, String> {
    let panel = crate::ipc::player_cmds::panel()?;
    let info = panel
        .login()
        .await
        .map_err(|e| crate::catalog::sync_engine::scrub(&format!("{e:#}")))?;
    if !info.auth {
        // The server answered and said no, which is a different thing from
        // not answering and worth saying differently.
        let status = if info.status.is_empty() {
            "rejected".to_owned()
        } else {
            info.status.clone()
        };
        return Err(format!("the panel rejected these credentials ({status})"));
    }
    let conns = match (info.active_cons, info.max_connections) {
        (Some(a), Some(m)) => format!(" · {a} of {m} connections in use"),
        _ => String::new(),
    };
    let status = if info.status.is_empty() {
        "Active".to_owned()
    } else {
        info.status
    };
    Ok(format!("{status}{conns}"))
}

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

/// Channels ordered by how much they are actually watched, most first.
///
/// Nobody adds anything to this list: it is accumulated by the player as it
/// runs (see [`crate::catalog::activity`]) and ordered by watch time decayed
/// against how long ago the channel was last watched, so a fortnight's
/// absence halves a channel's standing. An empty result is the ordinary
/// first-run answer, not a failure.
#[tauri::command]
pub async fn favourites(limit: u32) -> Result<Vec<FavouriteDto>, String> {
    let at = now();
    db::with_db(|conn| {
        let mut top = activity::rank(store::watched(conn)?, at, limit as usize);
        // Cheap here in a way it would not be for a listing: this is a
        // screenful at most, and it is the same channel row the rest of the
        // app draws, so it should not be the one place that claims nothing is
        // on air.
        let mut channels: Vec<ChannelDto> =
            top.iter().map(|f| f.channel.clone()).collect();
        attach_now_playing(conn, &mut channels)?;
        for (f, ch) in top.iter_mut().zip(channels) {
            f.channel = ch;
        }
        Ok(top)
    })
    .map_err(|e| e.to_string())
}

/// Forgets one channel's history, for when something was left playing by
/// accident and has crowded out the real list.
#[tauri::command]
pub async fn forget_activity(stream_id: i64) -> Result<(), String> {
    db::with_db(|conn| store::forget_activity(conn, stream_id)).map_err(|e| e.to_string())
}
