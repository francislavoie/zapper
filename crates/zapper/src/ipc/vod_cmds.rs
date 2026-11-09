//! The app's commands for movies: lists, search and details.
//!
//! Lists, search and marks answer from SQLite, off the async threads
//! (`spawn_blocking`): a page of a catalogue of a hundred thousand titles is
//! a real query, not a lookup. Details may ask the provider, once per title
//! and then from the kept copy; see [`crate::catalog::vod_details`].
//! Playing a movie is `player::controller`'.

use crate::catalog::{reads, vod_details};
use crate::ipc::types::{
    MarksDto, MovieDetailsDto, MovieDto, MoviePageDto, MovieQueryDto, MoviesSummaryDto,
    VodCategoryDto,
};

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

#[tauri::command]
pub async fn movies_summary() -> Result<MoviesSummaryDto, String> {
    blocking(reads::movies_summary).await
}

#[tauri::command]
pub async fn movie_categories() -> Result<Vec<VodCategoryDto>, String> {
    blocking(reads::movie_categories).await
}

#[tauri::command]
pub async fn movies(query: MovieQueryDto, offset: u32, limit: u32) -> Result<MoviePageDto, String> {
    blocking(move || reads::movies(&query, offset, limit)).await
}

/// The details pane's content. The one command here that may ask the
/// provider, and only the first time a title is opened in a week.
#[tauri::command]
pub async fn movie_details(movie_id: i64) -> Result<MovieDetailsDto, String> {
    vod_details::movie_details(movie_id).await.map_err(words)
}

#[tauri::command]
pub async fn similar_movies(movie_id: i64, limit: u32) -> Result<Vec<MovieDto>, String> {
    blocking(move || reads::similar_movies(movie_id, limit)).await
}

/// `None` leaves a mark as it is.
#[tauri::command]
pub async fn set_movie_marks(
    movie_id: i64,
    favourite: Option<bool>,
    watched: Option<bool>,
) -> Result<MarksDto, String> {
    blocking(move || reads::set_movie_marks(movie_id, favourite, watched)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::db;

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_movie_marks_round_trips() {
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
        let on = set_movie_marks(9801, Some(true), None).await.unwrap();
        assert!(on.favourite && !on.watched);
        let off = set_movie_marks(9801, Some(false), None).await.unwrap();
        assert!(!off.favourite);
        assert_eq!(off.id, 9801);
    }
}
