//! Continue watching and a title's marks, as the interface asks for them:
//! one place for any front end and the recording service's web API to
//! answer from, as [`crate::catalog::reads`] is for
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::ContinueKind;
    use crate::catalog::{db, reads};

    fn a_movie(id: i64) {
        db::with_db(|c| {
            c.execute(
                "INSERT OR REPLACE INTO movies (movie_id, category_id, name, clean_name, added)
                 VALUES (?1, 'cmd', 'A Film', 'A Film', 0)",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
    }

    /// Leaves the shared test catalogue as it was.
    fn forget(id: i64) {
        db::with_db(|c| {
            c.execute(
                "DELETE FROM vod_marks WHERE kind = 'movie' AND item_id = ?1",
                [id],
            )?;
            c.execute("DELETE FROM movies WHERE movie_id = ?1", [id])?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn what_progress_wrote_is_what_the_app_lists() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        a_movie(9811);
        reads::save_movie_progress(9811, 30.0, Some(100.0)).unwrap();
        let list = continue_watching(10).unwrap();
        let found = list.iter().find_map(|item| match item {
            ContinueItemDto::Movie {
                movie,
                last_watched,
            } if movie.movie_id == 9811 => Some((movie.clone(), *last_watched)),
            _ => None,
        });
        forget(9811);
        let (movie, last_watched) = found.expect("listed");
        assert_eq!(movie.position, Some(30.0));
        assert_eq!(movie.duration, Some(100.0));
        assert!(last_watched > 0);
        assert_eq!(movie.played_at, Some(last_watched));
    }

    #[test]
    fn marks_read_back_with_when_they_were_played() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        a_movie(9812);
        let before = movie_marks(9812).unwrap().unwrap();
        reads::save_movie_progress(9812, 30.0, Some(100.0)).unwrap();
        let after = movie_marks(9812).unwrap().unwrap();
        forget(9812);
        assert_eq!(before.played_at, None);
        assert_eq!(after.id, 9812);
        assert_eq!(after.position, Some(30.0));
        assert!(after.played_at.is_some_and(|t| t > 0));
    }

    #[test]
    fn removing_from_the_app_hides_it() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        a_movie(9813);
        // Played a minute ago, so the removal (now) is later.
        db::with_db(|c| {
            crate::catalog::vod_store::record_progress(
                c,
                crate::catalog::vod_store::ItemKind::Movie,
                9813,
                30.0,
                Some(100.0),
                reads::now() - 60,
            )?;
            Ok(())
        })
        .unwrap();
        let listed = |list: &[ContinueItemDto]| {
            list.iter().any(
                |i| matches!(i, ContinueItemDto::Movie { movie, .. } if movie.movie_id == 9813),
            )
        };
        let was = listed(&continue_watching(100).unwrap());
        let removed = remove_from_continue(ContinueKind::Movie, 9813).unwrap();
        let is = listed(&continue_watching(100).unwrap());
        let unknown = remove_from_continue(ContinueKind::Movie, 989_813).unwrap();
        let place = movie_marks(9813).unwrap().unwrap().position;
        forget(9813);
        assert!(was);
        assert!(removed);
        assert!(!is);
        assert_eq!(place, Some(30.0), "the place is kept");
        assert!(!unknown, "not in the catalogue");
    }

    #[test]
    fn an_unknown_episode_has_no_marks() {
        db::install_test_db();
        let e = episode_marks(989_814).unwrap();
        assert!(e.is_none(), "not in the catalogue: {e:?}");
        let e = movie_marks(989_814).unwrap();
        assert!(e.is_none(), "not in the catalogue: {e:?}");
    }
}
