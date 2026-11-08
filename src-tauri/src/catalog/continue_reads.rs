//! Continue watching and a title's marks, as the interface asks for them:
//! one place for the app's commands (`ipc::continue_cmds`) and the recording
//! service's web API to answer from, as [`crate::catalog::reads`] is for
//! movies.
//!
//! Every read answers from SQLite alone and never asks the provider (see
//! [`continue_store`]). A removal is dated by this process's clock, never by
//! one the interface sends: the entry shows again only once it is played
//! later than that.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

use crate::catalog::continue_store::{self, ContinueRow, ContinueTarget};
use crate::catalog::db;
use crate::catalog::reads::{marks_dto, movie_dto, now};
use crate::catalog::series_reads::{episode_dto, series_dto};
use crate::catalog::vod_store::{self, ItemKind};
use crate::ipc::types::{ContinueItemDto, ContinueKind, MarksDto};

fn item_dto(row: ContinueRow) -> ContinueItemDto {
    match row {
        ContinueRow::Movie {
            movie,
            last_watched,
        } => ContinueItemDto::Movie {
            movie: movie_dto(movie),
            last_watched,
        },
        ContinueRow::Episode {
            series,
            episode,
            up_next,
            last_watched,
        } => ContinueItemDto::Episode {
            series: series_dto(series),
            episode: episode_dto(episode),
            up_next,
            last_watched,
        },
    }
}

/// The current catalogue's Continue watching, newest first, at most `limit`
/// entries (clamped to `1..=`[`continue_store::CONTINUE_MAX`]).
pub fn continue_watching(limit: u32) -> Result<Vec<ContinueItemDto>> {
    let rows = db::with_db(|c| continue_store::continue_watching(c, limit))?;
    Ok(rows.into_iter().map(item_dto).collect())
}

/// Takes a movie or a show off Continue watching, as of now; its place is
/// kept. False, with nothing written, for a title not live in the catalogue.
pub fn remove_from_continue(kind: ContinueKind, id: i64) -> Result<bool> {
    let target = match kind {
        ContinueKind::Movie => ContinueTarget::Movie(id),
        ContinueKind::Series => ContinueTarget::Series(id),
    };
    db::with_db(|c| continue_store::remove_from_continue(c, target, now()))
}

/// A title's marks in the current catalogue, or `None` when it is not live.
fn marks(c: &Connection, kind: ItemKind, id: i64) -> Result<Option<MarksDto>> {
    let live = match kind {
        ItemKind::Movie => "SELECT 1 FROM movies WHERE movie_id = ?1 AND deleted_at IS NULL",
        ItemKind::Episode => "SELECT 1 FROM episodes WHERE episode_id = ?1 AND deleted_at IS NULL",
        ItemKind::Series => "SELECT 1 FROM series WHERE series_id = ?1 AND deleted_at IS NULL",
    };
    if c.query_row(live, [id], |_| Ok(())).optional()?.is_none() {
        return Ok(None);
    }
    let origin = vod_store::origin(c)?;
    let m = vod_store::marks_of(c, &origin, kind, id)?;
    Ok(Some(marks_dto(id, m)))
}

/// A movie's marks as they are now (where it was left and when); `None` for
/// a movie not live in the catalogue.
pub fn movie_marks(id: i64) -> Result<Option<MarksDto>> {
    db::with_db(|c| marks(c, ItemKind::Movie, id))
}

/// An episode's marks as they are now; `None` for an episode not live in the
/// catalogue.
pub fn episode_marks(id: i64) -> Result<Option<MarksDto>> {
    db::with_db(|c| marks(c, ItemKind::Episode, id))
}
