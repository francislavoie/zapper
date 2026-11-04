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

use crate::catalog::{activity, db, guide_search, index, status, store};
use crate::ipc::types::{
    CategoryDto, ChannelDto, FavouriteDto, GuideRowDto, ProgrammeDto, ProgrammeHitDto,
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
