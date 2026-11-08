//! The catalogue's reads, as the interface asks for them: one place for the
//! app's commands (`ipc::catalog_cmds`) and the recording service's web API
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

fn now() -> i64 {
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

fn movie_dto(r: vod_store::MovieRow) -> MovieDto {
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
    }
}

pub(crate) fn marks_dto(id: i64, m: vod_store::Marks) -> MarksDto {
    MarksDto {
        id,
        favourite: m.favourite,
        watched: m.watched,
        position: m.position,
        duration: m.duration,
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
