//! The app's commands for shows: lists, search, a show's details and
//! episodes, and marks.
//!
//! Reads answer from SQLite, off the async threads (`spawn_blocking`), as
//! the movies' do. A show's details may ask the provider, once per show and
//! then from the kept copy; see [`crate::catalog::series_details`]. Playing
//! an episode is `player_cmds`'.

use crate::catalog::{series_details as details, series_reads};
use crate::ipc::types::{
    MarksDto, PlayingEpisodeDto, SeriesCategoryDto, SeriesDetailsDto, SeriesPageDto,
    SeriesQueryDto, ShowsSummaryDto,
};

fn words(e: anyhow::Error) -> String {
    // An error from the provider's client carries its URL, and an Xtream
    // URL carries the account: scrubbed, as the sync's are.
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
pub async fn shows_summary() -> Result<ShowsSummaryDto, String> {
    blocking(series_reads::shows_summary).await
}

#[tauri::command]
pub async fn series_categories() -> Result<Vec<SeriesCategoryDto>, String> {
    blocking(series_reads::series_categories).await
}

#[tauri::command]
pub async fn series(
    query: SeriesQueryDto,
    offset: u32,
    limit: u32,
) -> Result<SeriesPageDto, String> {
    blocking(move || series_reads::series(&query, offset, limit)).await
}

/// The show's sheet: details, seasons, episodes and what is next up. May
/// ask the provider, and only the first time a show is opened in a week.
#[tauri::command]
pub async fn series_details(series_id: i64) -> Result<SeriesDetailsDto, String> {
    details::series_details(series_id).await.map_err(words)
}

/// `None` leaves a mark as it is. Marking watched a show whose episodes are
/// not here yet asks for them first.
#[tauri::command]
pub async fn set_series_marks(
    series_id: i64,
    favourite: Option<bool>,
    watched: Option<bool>,
) -> Result<MarksDto, String> {
    series_reads::set_series_marks(series_id, favourite, watched)
        .await
        .map_err(words)
}

#[tauri::command]
pub async fn set_episode_marks(episode_id: i64, watched: bool) -> Result<MarksDto, String> {
    blocking(move || series_reads::set_episode_marks(episode_id, watched)).await
}

/// The episode playing and its show, for the player's title.
#[tauri::command]
pub async fn playing_episode(episode_id: i64) -> Result<PlayingEpisodeDto, String> {
    blocking(move || series_reads::playing_episode(episode_id)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::db;

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_series_marks_round_trips() {
        let _catalogue = db::movie_catalogue();
        db::install_test_db();
        db::with_db(|c| {
            c.execute(
                "INSERT OR REPLACE INTO series (series_id, name, clean_name, added)
                 VALUES (9851, 'Round', 'Round', 0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let on = set_series_marks(9851, Some(true), None).await.unwrap();
        assert!(on.favourite && !on.watched);
        let off = set_series_marks(9851, Some(false), None).await.unwrap();
        assert!(!off.favourite);
        assert_eq!(off.id, 9851);
        let gone = playing_episode(-7).await.unwrap_err();
        assert_eq!(gone, "that episode is not in the catalogue any more");
    }
}
