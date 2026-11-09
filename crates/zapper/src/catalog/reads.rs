//! The catalogue's reads, as the interface asks for them: one place for any
//! front end and the recording service's web API
//! (`recording::browse`) to answer from, so a channel page or a guide row is
//! the same whichever of the two draws it.
//!
//! Every function here answers from SQLite alone. Nothing may block on the
//! network: startup latency is the whole point of the local mirror, and a
//! week-old cache must open exactly as fast as a fresh one.

use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

use crate::catalog::credentials::Kind;
use crate::catalog::{activity, db, guide_search, index, status, store, title_search, vod_store};
use crate::ipc::types::{
    CategoryDto, ChannelDto, FavouriteDto, GuideRowDto, MarksDto, MovieDto, MovieListKind,
    MoviePageDto, MovieQueryDto, MovieSort, MoviesSummaryDto, ProgrammeDto, ProgrammeHitDto,
    VodCategoryDto,
};

/// The app's (or the service's) clock, Unix seconds: every time a mark
/// records comes from it, never from the interface.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Attaches what is on air to a page of channels. Done for the page rather
/// than in the listing query so browsing a huge catalogue never joins the
/// guide for rows nobody is looking at.
fn attach_now_playing(conn: &Connection, page: &mut [ChannelDto]) -> Result<()> {
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

pub fn categories() -> Result<Vec<CategoryDto>> {
    db::with_db(store::categories)
}

/// `category_id` of `None` means every channel, ordered by `num`.
pub fn channels(category_id: Option<&str>, offset: u32, limit: u32) -> Result<Vec<ChannelDto>> {
    db::with_db(|conn| {
        let mut page = store::channels_in(conn, category_id, offset, limit)?;
        attach_now_playing(conn, &mut page)?;
        Ok(page)
    })
}

/// Specific channels, by stream id, with their now-playing attached. Ids that
/// no longer exist are absent from the answer rather than an error: a channel
/// can vanish between sessions.
pub fn channels_by_ids(ids: &[i64]) -> Result<Vec<ChannelDto>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    db::with_db(|conn| {
        let mut rows = store::channels_by_ids(conn, ids)?;
        attach_now_playing(conn, &mut rows)?;
        Ok(rows)
    })
}

/// Other sources for a channel's programmes, with their now-playing
/// attached. See [`store::alternatives_for`].
pub fn alternatives_for(stream_id: i64) -> Result<Vec<ChannelDto>> {
    db::with_db(|conn| {
        let mut rows = store::alternatives_for(conn, stream_id)?;
        attach_now_playing(conn, &mut rows)?;
        Ok(rows)
    })
}

/// Every channel number and the channel it tunes. See
/// [`store::channel_numbers`].
pub fn channel_numbers() -> Result<Vec<(String, i64)>> {
    db::with_db(store::channel_numbers)
}

/// The catalogue's last full pass when the search index was last built here.
/// The index is this process's, but the catalogue may be refreshed by
/// another one sharing it (the app and the recording service on one
/// computer), so a pass recorded since means the names may have changed.
static INDEXED_FOR: Mutex<Option<Option<i64>>> = Mutex::new(None);

/// Fuzzy match over all channels. Separators are never returned; a blank
/// query finds nothing rather than every channel.
pub fn search(query: &str, limit: u32) -> Result<Vec<ChannelDto>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let synced = status::last_synced();
    db::with_db(|conn| {
        // The index is built by sync, but search must work before the first
        // sync of the session lands — the mirror on disk is enough.
        let mut stamp = INDEXED_FOR.lock().unwrap_or_else(|e| e.into_inner());
        if !index::is_built() || stamp.is_some_and(|s| s != synced) {
            index::rebuild(conn)?;
        }
        *stamp = Some(synced);
        drop(stamp);
        let ids = index::query(query, limit as usize);
        let mut hits = store::channels_by_ids(conn, &ids)?;
        attach_now_playing(conn, &mut hits)?;
        Ok(hits)
    })
}

/// The most one page of programme search returns, whatever is asked for.
pub const PROGRAMME_PAGE_MAX: u32 = 500;

/// Airings whose title matches, on now first, then upcoming, then what can
/// still be played from an archive — see [`guide_search::search`]. Blank
/// finds nothing, as channel search does.
pub fn search_programmes(query: &str, offset: u32, limit: u32) -> Result<Vec<ProgrammeHitDto>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.min(PROGRAMME_PAGE_MAX);
    db::with_db(|conn| guide_search::search(conn, query, now(), offset, limit))
}

/// Where a channel sits in a category's listing, so the guide can be opened
/// at its row. `None` when it is not in that category.
pub fn channel_index(category_id: Option<&str>, stream_id: i64) -> Result<Option<u32>> {
    db::with_db(|conn| store::channel_index(conn, category_id, stream_id))
}

/// Programmes overlapping `[from, to]` for the given channels.
pub fn guide(stream_ids: &[i64], from: i64, to: i64) -> Result<Vec<(i64, Vec<ProgrammeDto>)>> {
    db::with_db(|conn| store::programmes(conn, stream_ids, from, to))
}

/// Guide rows for a window, one per channel, ordered as requested.
pub fn guide_rows(stream_ids: &[i64], from: i64, to: i64) -> Result<Vec<GuideRowDto>> {
    db::with_db(|conn| store::guide_rows(conn, stream_ids, from, to))
}

/// Channels ordered by how much they are actually watched, most first; see
/// [`activity::rank`]. An empty result is the ordinary first-run answer.
pub fn favourites(limit: u32) -> Result<Vec<FavouriteDto>> {
    let at = now();
    db::with_db(|conn| {
        let top = activity::rank(store::watched(conn)?, at, limit as usize);
        with_now_playing(conn, top)
    })
}

/// Channels in the order they were last watched, newest first, whatever the
/// watch time; see [`activity::recent`]. Empty before anything is watched.
pub fn recently_watched(limit: u32) -> Result<Vec<FavouriteDto>> {
    db::with_db(|conn| {
        let rows = activity::recent(store::watched(conn)?, limit as usize);
        with_now_playing(conn, rows)
    })
}

/// The same channel row the rest of the app draws, so a watched channel is not
/// the one place that claims nothing is on air.
fn with_now_playing(conn: &Connection, mut rows: Vec<FavouriteDto>) -> Result<Vec<FavouriteDto>> {
    let mut channels: Vec<ChannelDto> = rows.iter().map(|f| f.channel.clone()).collect();
    attach_now_playing(conn, &mut channels)?;
    for (f, ch) in rows.iter_mut().zip(channels) {
        f.channel = ch;
    }
    Ok(rows)
}

/// Forgets one channel's history.
pub fn forget_activity(stream_id: i64) -> Result<()> {
    db::with_db(|conn| store::forget_activity(conn, stream_id))
}

/// The logo the catalogue has for a channel, as the provider gave it.
pub fn logo_url(stream_id: i64) -> Result<Option<String>> {
    db::with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT logo_url FROM channels WHERE stream_id = ?1 AND deleted_at IS NULL",
                [stream_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    })
}

// ------------------------------------------------------------------ movies

pub(crate) fn movie_dto(r: vod_store::MovieRow) -> MovieDto {
    MovieDto {
        movie_id: r.movie_id,
        name: r.name,
        clean_name: r.clean_name,
        year: r.year,
        rating: r.rating,
        poster_url: r.poster_url,
        added: r.added,
        duration_secs: r.duration_secs,
        category_id: r.category_id,
        favourite: r.favourite,
        watched: r.watched,
        position: r.position,
        duration: r.duration,
        played_at: r.played_at,
    }
}

pub(crate) fn marks_dto(id: i64, m: vod_store::Marks) -> MarksDto {
    MarksDto {
        id,
        favourite: m.favourite,
        watched: m.watched,
        position: m.position,
        duration: m.duration,
        played_at: m.played_at,
    }
}

/// Why the account has no movies, in words, or `None` when it has some.
pub fn movies_note(kind: Option<Kind>, total: u32) -> Option<String> {
    if total > 0 {
        return None;
    }
    Some(
        match kind {
            None => "No account yet. Set one in Settings \u{203a} Account.",
            Some(Kind::HdHomeRun) => "A tuner carries live channels only: it has no movies.",
            Some(Kind::Tvheadend) => "Tvheadend serves live channels: it has no movies.",
            Some(Kind::M3u) => {
                "This playlist has no movies. Entries whose address is a video file (.mp4, .mkv\u{2026}) or contains /movie/ appear here."
            }
            Some(Kind::Xtream) => "No movies yet. They arrive with the catalogue's next refresh.",
        }
        .to_owned(),
    )
}

/// How many movies there are, and whether the interface should offer them.
pub fn movies_summary() -> Result<MoviesSummaryDto> {
    let (counts, origin) = db::with_db(|c| {
        Ok((
            vod_store::movie_counts(c, now())?,
            crate::catalog::vod_sync::movies_origin(c)?,
        ))
    })?;
    let kind = crate::catalog::credentials::load_account_stored()
        .ok()
        .flatten()
        .map(|a| a.kind());
    let note = movies_note(kind, counts.total);
    Ok(MoviesSummaryDto {
        available: note.is_none(),
        note,
        total: counts.total,
        favourites: counts.favourites,
        recent: counts.recent,
        watched: counts.watched,
        origin,
    })
}

pub fn movie_categories() -> Result<Vec<VodCategoryDto>> {
    db::with_db(|c| {
        Ok(vod_store::movie_categories(c)?
            .into_iter()
            .map(|r| VodCategoryDto {
                id: r.id,
                name: r.name,
                movie_count: r.movie_count.into(),
            })
            .collect())
    })
}

/// A list as the store asks for it, from the interface's words. The shows'
/// lists are the movies', so [`crate::catalog::series_reads`] asks the same
/// way.
pub(crate) fn store_query(
    list: MovieListKind,
    category_id: Option<&str>,
    sort: MovieSort,
    year_from: Option<i32>,
    year_to: Option<i32>,
    search: Option<&str>,
) -> Result<vod_store::MovieQuery> {
    let list = match list {
        MovieListKind::All => vod_store::MovieList::All,
        MovieListKind::Favourites => vod_store::MovieList::Favourites,
        MovieListKind::Recent => vod_store::MovieList::Recent,
        MovieListKind::Watched => vod_store::MovieList::Watched,
        MovieListKind::Category => {
            let Some(id) = category_id else {
                return Err(anyhow::anyhow!("a category list needs a category"));
            };
            vod_store::MovieList::Category(id.to_owned())
        }
    };
    Ok(vod_store::MovieQuery {
        list,
        sort: match sort {
            MovieSort::Added => vod_store::MovieSort::Added,
            MovieSort::Az => vod_store::MovieSort::Az,
            MovieSort::Year => vod_store::MovieSort::Year,
            MovieSort::Rating => vod_store::MovieSort::Rating,
        },
        year_from,
        year_to,
        // Blank, or only pipes, is no search rather than one that finds
        // everything the slow way.
        search: search
            .filter(|t| title_search::Needle::parse(t).is_some())
            .map(|t| t.trim().to_owned()),
    })
}

/// One page of a list, and how many titles the list has in all.
pub fn movies(q: &MovieQueryDto, offset: u32, limit: u32) -> Result<MoviePageDto> {
    let query = store_query(
        q.list,
        q.category_id.as_deref(),
        q.sort,
        q.year_from,
        q.year_to,
        q.search.as_deref(),
    )?;
    let (total, rows) = db::with_db(|c| vod_store::movies_page(c, &query, offset, limit, now()))?;
    Ok(MoviePageDto {
        total,
        items: rows.into_iter().map(movie_dto).collect(),
    })
}

/// Specific titles, in the order asked; ones that are gone are left out.
pub fn movies_by_ids(ids: &[i64]) -> Result<Vec<MovieDto>> {
    db::with_db(|c| {
        Ok(vod_store::movies_by_ids(c, ids)?
            .into_iter()
            .map(movie_dto)
            .collect())
    })
}

pub fn similar_movies(id: i64, limit: u32) -> Result<Vec<MovieDto>> {
    db::with_db(|c| {
        Ok(vod_store::similar_movies(c, id, limit)?
            .into_iter()
            .map(movie_dto)
            .collect())
    })
}

/// Sets or clears the favourite and watched marks; `None` leaves one alone.
pub fn set_movie_marks(
    id: i64,
    favourite: Option<bool>,
    watched: Option<bool>,
) -> Result<MarksDto> {
    db::with_db(|c| {
        let m = vod_store::set_marks(c, vod_store::ItemKind::Movie, id, favourite, watched, now())?;
        Ok(marks_dto(id, m))
    })
}

/// Writes down how far into a movie the player is.
pub fn save_movie_progress(id: i64, position: f64, duration: Option<f64>) -> Result<MarksDto> {
    db::with_db(|c| {
        let m = vod_store::record_progress(
            c,
            vod_store::ItemKind::Movie,
            id,
            position,
            duration,
            now(),
        )?;
        Ok(marks_dto(id, m))
    })
}

/// The poster the catalogue has for a movie, as the provider gave it.
pub fn movie_poster_url(id: i64) -> Result<Option<String>> {
    db::with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT poster_url FROM movies WHERE movie_id = ?1 AND deleted_at IS NULL",
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    })
}

/// The `n`th backdrop in the details already kept. Never fetches: an image
/// request must not cost the provider a call.
pub fn movie_backdrop_url(id: i64, n: usize) -> Result<Option<String>> {
    let kept = db::with_db(|c| vod_store::cached_details(c, id))?;
    Ok(kept
        .and_then(|(_, json)| {
            serde_json::from_str::<crate::catalog::vod_details::MovieInfo>(&json).ok()
        })
        .and_then(|i| i.backdrops.into_iter().nth(n)))
}

#[cfg(test)]
mod movie_tests {
    use super::*;

    #[test]
    fn movies_note_says_why_a_tuner_has_none() {
        let n = movies_note(Some(Kind::HdHomeRun), 0).unwrap();
        assert!(n.contains("tuner") && n.contains("no movies"), "{n}");
        assert!(movies_note(Some(Kind::Tvheadend), 0)
            .unwrap()
            .contains("Tvheadend"));
        assert!(movies_note(None, 0).unwrap().starts_with("No account yet."));
    }

    #[test]
    fn movies_note_is_silent_when_there_are_movies() {
        for k in [
            None,
            Some(Kind::Xtream),
            Some(Kind::M3u),
            Some(Kind::HdHomeRun),
        ] {
            assert_eq!(movies_note(k, 1), None);
        }
    }

    #[test]
    fn movies_note_tells_a_playlist_what_counts() {
        let n = movies_note(Some(Kind::M3u), 0).unwrap();
        assert!(n.contains(".mp4") && n.contains("/movie/"), "{n}");
        assert!(movies_note(Some(Kind::Xtream), 0)
            .unwrap()
            .contains("next refresh"));
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;
    use crate::catalog::settings as settings_store;
    use crate::ipc::types::SettingsDto;

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

    #[test]
    fn channels_page_carries_what_is_on_air() {
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

        let page = channels(Some("cmd-channels".into()), 0, 10).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(
            page[0].now_playing.as_ref().map(|p| p.title.as_str()),
            Some("On now")
        );
        // A programme that finished an hour ago is not what is on.
        assert!(page[1].now_playing.is_none());
    }

    #[test]
    fn categories_reports_its_channel_count() {
        seed(2000, "cmd-categories");
        let cats = categories().unwrap();
        let mine = cats
            .iter()
            .find(|c| c.id == "cmd-categories")
            .expect("seeded category missing");
        assert_eq!(mine.channel_count, 2);
    }

    #[test]
    fn a_blank_query_finds_nothing_rather_than_everything() {
        db::install_test_db();
        assert!(search("".into(), 20).unwrap().is_empty());
        assert!(search("   ".into(), 20).unwrap().is_empty());
    }

    #[test]
    fn search_answers_from_the_mirror_before_any_sync() {
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

        let hits = search("quixotic".into(), 20).unwrap();
        assert_eq!(hits.first().map(|c| c.stream_id), Some(3001));
    }

    #[test]
    fn guide_returns_a_row_per_requested_channel() {
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

        let g = guide(&[4002, 4001], 0, 1000).unwrap();
        assert_eq!(g.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [4002, 4001]);
        assert!(g[0].1.is_empty());
        assert_eq!(g[1].1.len(), 1);
    }

    #[test]
    fn guide_rows_carry_the_channel_beside_its_programmes() {
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

        let rows = guide_rows(&[5002, 5001], 0, 1000).unwrap();
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

    #[test]
    fn programme_search_reaches_the_guide_through_its_index() {
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

        let hits = search_programmes("zygomorph".into(), 0, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].channel.stream_id, 12002);
        assert_eq!(hits[0].airing, crate::ipc::types::AiringDto::Upcoming);
        assert!(search_programmes("  ".into(), 0, 10).unwrap().is_empty());

        // And the guide can find the row it is on.
        assert_eq!(
            channel_index(Some("cmd-programmes".into()), 12002).unwrap(),
            Some(1)
        );
    }

    #[test]
    fn settings_default_before_anything_is_saved_and_round_trip_after() {
        db::install_test_db();
        let _lock = settings_store::test_lock();
        // The key is process-wide, so this is the only test that writes the
        // guide's hours; the others that write settings leave them alone.
        let first = settings_store::load_or_default();
        assert_eq!(first.guide_hours, SettingsDto::default().guide_hours);

        let want = SettingsDto {
            guide_hours: 9,
            rewind_mb: 1500,
            ..Default::default()
        };
        db::with_db(|c| settings_store::save(c, &want)).unwrap();

        let got = settings_store::load_or_default();
        assert_eq!(got.guide_hours, 9);
        assert_eq!(got.rewind_mb, 1500);
    }

    #[test]
    fn favourites_rank_by_recency_weighted_watch_time() {
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
        let top = favourites(500).unwrap();
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

    #[test]
    fn recently_watched_is_newest_first() {
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
            
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .filter(|id| (13000..14000).contains(id))
            .collect();
        assert_eq!(ids, [13002, 13001]);
    }

    #[test]
    fn favourites_is_empty_rather_than_failing_before_anything_is_watched() {
        db::install_test_db();
        // Nothing has ever been watched on *this* channel, and the command
        // must not be the thing that says so with an error.
        seed(7000, "cmd-favourites-empty");
        let top = favourites(50).unwrap();
        assert!(!top.iter().any(|f| (7000..8000).contains(&f.channel.stream_id)));
    }

    #[test]
    fn favourites_never_offers_a_separator_or_a_deleted_channel() {
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
            
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .filter(|id| (8000..9000).contains(id))
            .collect();
        assert_eq!(ids, [8001]);
    }

    #[test]
    fn favourites_carry_what_is_on_air() {
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

        let top = favourites(50).unwrap();
        let mine = top.iter().find(|f| f.channel.stream_id == 9001).unwrap();
        assert_eq!(
            mine.channel.now_playing.as_ref().map(|p| p.title.as_str()),
            Some("On now")
        );
    }

    #[test]
    fn forget_activity_drops_one_channel_and_leaves_the_rest() {
        seed(11000, "cmd-forget");
        let t = now();
        db::with_db(|c| {
            store::record_activity(c, 11001, 40 * 3600, true, t - 60)?;
            store::record_activity(c, 11002, 3600, true, t - 60)?;
            Ok(())
        })
        .unwrap();

        forget_activity(11001).unwrap();
        let ids: Vec<i64> = favourites(50)
            
            .unwrap()
            .iter()
            .map(|f| f.channel.stream_id)
            .filter(|id| (11000..12000).contains(id))
            .collect();
        assert_eq!(ids, [11002]);
        // Twice is fine: forgetting what is already forgotten is not an error.
        forget_activity(11001).unwrap();
    }

    #[test]
    fn sync_status_answers_without_a_network() {
        db::install_test_db();
        let s = status::snapshot();
        assert!(s.channels_total >= 0);
    }

    #[test]
    fn set_movie_marks_round_trips() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT INTO movies (movie_id, category_id, name, clean_name, added)
                 VALUES (9801, 'cmd', 'Round', 'Round', 0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let on = set_movie_marks(9801, Some(true), None).unwrap();
        assert!(on.favourite && !on.watched);
        let off = set_movie_marks(9801, Some(false), None).unwrap();
        assert!(!off.favourite);
        assert_eq!(off.id, 9801);
    }
}
