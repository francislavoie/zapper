//! The app's commands for Continue watching: the list, taking a title off
//! it, and a title's marks as they are now (where it was left and when).
//!
//! All answer from SQLite, off the async threads (`spawn_blocking`), and
//! never ask the provider; see [`crate::catalog::continue_reads`]. Playing
//! an entry is `player_cmds`'.

use crate::catalog::continue_reads;
use crate::ipc::types::{ContinueItemDto, ContinueKind, MarksDto};

/// What the interface is told when a title it names has gone.
const GONE: &str = "that title is not in the catalogue any more";

fn words(e: anyhow::Error) -> String {
    // Nothing here puts an account in a message, but an error from a
    // library might: scrub as the sync's do.
    crate::catalog::sync_engine::scrub(&format!("{e:#}"))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("the read did not finish: {e}"))?
        .map_err(words)
}

/// Continue watching, newest first, at most `limit` entries.
#[tauri::command]
pub async fn continue_watching(limit: u32) -> Result<Vec<ContinueItemDto>, String> {
    blocking(move || continue_reads::continue_watching(limit)).await
}

/// Takes a movie or a show off Continue watching until it is played again.
#[tauri::command]
pub async fn remove_from_continue(kind: ContinueKind, id: i64) -> Result<(), String> {
    if blocking(move || continue_reads::remove_from_continue(kind, id)).await? {
        Ok(())
    } else {
        Err(GONE.to_owned())
    }
}

/// A movie's marks as they are now: where it was left, and when.
#[tauri::command]
pub async fn movie_marks(movie_id: i64) -> Result<MarksDto, String> {
    blocking(move || continue_reads::movie_marks(movie_id))
        .await?
        .ok_or_else(|| GONE.to_owned())
}

/// An episode's marks as they are now.
#[tauri::command]
pub async fn episode_marks(episode_id: i64) -> Result<MarksDto, String> {
    blocking(move || continue_reads::episode_marks(episode_id))
        .await?
        .ok_or_else(|| GONE.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn what_progress_wrote_is_what_the_app_lists() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        a_movie(9811);
        reads::save_movie_progress(9811, 30.0, Some(100.0)).unwrap();
        let list = continue_watching(10).await.unwrap();
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

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn marks_read_back_with_when_they_were_played() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        a_movie(9812);
        let before = movie_marks(9812).await.unwrap();
        reads::save_movie_progress(9812, 30.0, Some(100.0)).unwrap();
        let after = movie_marks(9812).await.unwrap();
        forget(9812);
        assert_eq!(before.played_at, None);
        assert_eq!(after.id, 9812);
        assert_eq!(after.position, Some(30.0));
        assert!(after.played_at.is_some_and(|t| t > 0));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn removing_from_the_app_hides_it() {
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
        let was = listed(&continue_watching(100).await.unwrap());
        let removed = remove_from_continue(ContinueKind::Movie, 9813).await;
        let is = listed(&continue_watching(100).await.unwrap());
        let unknown = remove_from_continue(ContinueKind::Movie, 989_813).await;
        let place = movie_marks(9813).await.unwrap().position;
        forget(9813);
        assert!(was);
        assert_eq!(removed, Ok(()));
        assert!(!is);
        assert_eq!(place, Some(30.0), "the place is kept");
        assert!(unknown.unwrap_err().contains("not in the catalogue"));
    }

    #[tokio::test]
    async fn an_unknown_episode_has_no_marks() {
        db::install_test_db();
        let e = episode_marks(989_814).await.unwrap_err();
        assert!(e.contains("not in the catalogue"), "{e}");
        let e = movie_marks(989_814).await.unwrap_err();
        assert!(e.contains("not in the catalogue"), "{e}");
    }
}
