//! Catalogue reads and background sync.
//!
//! Every command here must answer from SQLite alone. Nothing on this path may
//! block on the network: startup latency is the whole point of the local
//! mirror, and a week-old cache must open exactly as fast as a fresh one.
//! The reads themselves are in [`reads`], which the recording service's web
//! API answers from too.

#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use crate::catalog::settings as settings_store;
use crate::catalog::{account_form, db, reads, status, sync_engine};
#[cfg(test)]
use crate::catalog::{guide_search, index, store};
use crate::ipc::types::{
    AccountCandidateDto, AccountDto, CategoryDto, ChannelDto, FavouriteDto, GuideRowDto,
    ProgrammeDto, ProgrammeHitDto, SettingsDto, SyncStatusDto,
};

#[cfg(test)]
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn words(e: anyhow::Error) -> String {
    e.to_string()
}

#[tauri::command]
pub async fn categories() -> Result<Vec<CategoryDto>, String> {
    reads::categories().map_err(words)
}

/// `category_id` of `None` means every channel, ordered by `num`.
#[tauri::command]
pub async fn channels(
    category_id: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<ChannelDto>, String> {
    reads::channels(category_id.as_deref(), offset, limit).map_err(words)
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
    reads::channels_by_ids(&ids).map_err(words)
}

/// Other channels with the same programmes, to try when one is off air.
/// See [`crate::catalog::store::alternatives_for`].
#[tauri::command]
pub async fn alternatives_for(stream_id: i64) -> Result<Vec<ChannelDto>, String> {
    reads::alternatives_for(stream_id).map_err(words)
}

/// Every channel number and the channel it tunes, for typing a number to
/// change channel. See [`crate::catalog::store::channel_numbers`].
///
/// The whole list in one answer rather than a lookup per keystroke: deciding
/// whether another digit could still be coming needs every number that starts
/// with what has been typed, and the frontend asks once and keeps the answer
/// until the next sync.
#[tauri::command]
pub async fn channel_numbers() -> Result<Vec<(String, i64)>, String> {
    reads::channel_numbers().map_err(words)
}

/// Fuzzy match over all channels. Separators are never returned.
#[tauri::command]
pub async fn search(query: String, limit: u32) -> Result<Vec<ChannelDto>, String> {
    reads::search(&query, limit).map_err(words)
}

/// Airings whose title matches, on now first, then upcoming, then what can
/// still be played from an archive — see [`crate::catalog::guide_search::search`].
/// Paged, because a common word can match thousands of airings across a week.
#[tauri::command]
pub async fn search_programmes(
    query: String,
    offset: u32,
    limit: u32,
) -> Result<Vec<ProgrammeHitDto>, String> {
    reads::search_programmes(&query, offset, limit).map_err(words)
}

/// Where a channel sits in a category's listing, so the guide can be opened
/// at its row. `None` when it is not in that category.
#[tauri::command]
pub async fn channel_index(
    category_id: Option<String>,
    stream_id: i64,
) -> Result<Option<u32>, String> {
    reads::channel_index(category_id.as_deref(), stream_id).map_err(words)
}

/// Programmes overlapping `[from, to]` for the given channels.
#[tauri::command]
pub async fn guide(
    stream_ids: Vec<i64>,
    from: i64,
    to: i64,
) -> Result<Vec<(i64, Vec<ProgrammeDto>)>, String> {
    reads::guide(&stream_ids, from, to).map_err(words)
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
    async fn programme_search_reaches_the_guide_through_its_index() {
        seed(12000, "cmd-programmes");
        let t = now();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO epg (channel_ref, start, stop, title, description)
                 VALUES ('epg-12002', ?1, ?2, 'Zygomorphic Hour', NULL)",
                rusqlite::params![t + 3600, t + 7200],
            )?;
            // What every guide writer does before committing.
            guide_search::index_changes(c)?;
            Ok(())
        })
        .unwrap();

        let hits = search_programmes("zygomorph".into(), 0, 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].channel.stream_id, 12002);
        assert_eq!(hits[0].airing, crate::ipc::types::AiringDto::Upcoming);
        assert!(search_programmes("  ".into(), 0, 10).await.unwrap().is_empty());

        // And the guide can find the row it is on.
        assert_eq!(
            channel_index(Some("cmd-programmes".into()), 12002).await.unwrap(),
            Some(1)
        );
    }

    #[tokio::test]
    async fn settings_default_before_anything_is_saved_and_round_trip_after() {
        db::install_test_db();
        let _lock = settings_store::test_lock();
        // The key is process-wide, so this is the only test that writes the
        // guide's hours; the others that write settings leave them alone.
        let first = settings().await.unwrap();
        assert_eq!(first.guide_hours, SettingsDto::default().guide_hours);

        let want = SettingsDto {
            guide_hours: 9,
            rewind_mb: 1500,
            ..Default::default()
        };
        set_settings(want).await.unwrap();

        let got = settings().await.unwrap();
        assert_eq!(got.guide_hours, 9);
        assert_eq!(got.rewind_mb, 1500);
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
    async fn recently_watched_is_newest_first() {
        seed(13000, "cmd-recent");
        let t = now();
        db::with_db(|c| {
            store::record_activity(c, 13001, 20 * 3600, true, t - 3 * 86_400)?;
            store::record_activity(c, 13002, 60, true, t - 3600)?;
            Ok(())
        })
        .unwrap();

        // Shared database: assert the order of these two rows only.
        let ids: Vec<i64> = recently_watched(500)
            .await
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .filter(|id| (13000..14000).contains(id))
            .collect();
        assert_eq!(ids, [13002, 13001]);
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

// ----------------------------------------------------------------- account
//
// The form's rules are [`account_form`]'s, shared with the recording
// service's `/api/account`; what is the app's alone is dropping its cached
// client, so the next request goes to the new account without a restart.

/// The account, without its secrets.
#[tauri::command]
pub async fn account() -> Result<AccountDto, String> {
    Ok(account_form::view())
}

/// Saves the form, then drops the cached client and asks for a pass: new
/// credentials are the thing a refused sync was waiting for.
fn save(candidate: AccountCandidateDto) -> Result<(), String> {
    account_form::save(candidate)?;
    crate::ipc::player_cmds::forget_source();
    sync_engine::request_now();
    Ok(())
}

/// Makes an Xtream login the account in use. An empty password keeps the
/// one already on file, which is what lets the interface show the field as
/// set without ever being given the value to send back.
#[tauri::command]
pub async fn set_account(host: String, username: String, password: String) -> Result<(), String> {
    save(AccountCandidateDto::Xtream {
        host,
        username,
        password,
    })
}

/// Makes an M3U playlist the account in use. `playlist` and `epg_urls` of
/// `None` keep what is on file (it was shown redacted); an empty guide list
/// is a real value: the guide the playlist names for itself, if any.
#[tauri::command]
pub async fn set_m3u_account(
    playlist: Option<String>,
    epg_urls: Option<Vec<String>>,
    user_agent: String,
) -> Result<(), String> {
    save(AccountCandidateDto::M3u {
        playlist,
        epg_urls,
        user_agent,
    })
}

/// Makes an HDHomeRun the account in use. `epg_urls` of `None` keeps the
/// guide list on file; an empty list is SiliconDust's guide.
#[tauri::command]
pub async fn set_hdhomerun_account(
    device: String,
    epg_urls: Option<Vec<String>>,
) -> Result<(), String> {
    save(AccountCandidateDto::Hdhomerun { device, epg_urls })
}

/// Makes a Tvheadend server the account in use. An empty password keeps the
/// one on file; an empty username is a server that needs no login.
#[tauri::command]
pub async fn set_tvheadend_account(
    url: String,
    username: String,
    password: String,
) -> Result<(), String> {
    save(AccountCandidateDto::Tvheadend {
        url,
        username,
        password,
    })
}

/// Asks the account in use who we are, through the client playback uses.
#[tauri::command]
pub async fn test_account() -> Result<String, String> {
    account_form::probe(crate::ipc::player_cmds::source()?).await
}

/// Tests an account as typed, without saving it or touching the one in use.
#[tauri::command]
pub async fn test_account_candidate(candidate: AccountCandidateDto) -> Result<String, String> {
    account_form::test(candidate).await
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
    db::with_db(|c| settings_store::save(c, &settings)).map_err(|e| e.to_string())?;
    // Takes effect now, not at the next launch. Idempotent, so every save
    // can say it.
    crate::media_keys::set_enabled(settings.media_controls);
    Ok(())
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
    reads::guide_rows(&stream_ids, from, to).map_err(words)
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
    reads::favourites(limit).map_err(words)
}

/// The channels watched most recently, newest first, for the rail's Recently
/// watched. Empty before anything is watched.
#[tauri::command]
pub async fn recently_watched(limit: u32) -> Result<Vec<FavouriteDto>, String> {
    reads::recently_watched(limit).map_err(words)
}

/// Forgets one channel's history, for when something was left playing by
/// accident and has crowded out the real list.
#[tauri::command]
pub async fn forget_activity(stream_id: i64) -> Result<(), String> {
    reads::forget_activity(stream_id).map_err(words)
}
