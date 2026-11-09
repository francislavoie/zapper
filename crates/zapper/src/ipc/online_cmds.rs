//! The app's commands for the online services: Settings › Online services,
//! and subtitles found on OpenSubtitles for a movie or an episode, kept,
//! and added to the title playing.
//!
//! The work is [`crate::online::subtitles`]'; adding a file to the player
//! is `player::controller`'. What reads or writes a file (the settings, the kept
//! files' rows) runs off the async threads. Every error passes
//! [`scrub`]: none carries a password, a token or a link.

use std::path::PathBuf;

use crate::catalog::sync_engine::scrub;
use crate::catalog::vod_store::VodItem;
use crate::player::controller;
use crate::ipc::types::{
    KeptSubtitleDto, OnDemandKind, OnlineServicesDto, PlayerOptionsDto, SubtitleDownloadDto,
    SubtitleSearchDto,
};
use crate::online::subtitles;

fn item(kind: OnDemandKind, id: i64) -> VodItem {
    match kind {
        OnDemandKind::Movie => VodItem::movie(id),
        OnDemandKind::Episode => VodItem::episode(id),
    }
}

fn scrubbed<T>(r: Result<T, String>) -> Result<T, String> {
    r.map_err(|e| scrub(&e))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let r = tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("the read did not finish: {e}"))?;
    scrubbed(r)
}

/// What Settings › Online services shows.
#[tauri::command]
pub async fn online_services() -> Result<OnlineServicesDto, String> {
    blocking(|| Ok(subtitles::status())).await
}

/// Turns OpenSubtitles on or off, and sets the languages searched for;
/// `None` leaves either as it is.
#[tauri::command]
pub async fn set_opensubtitles(
    enabled: Option<bool>,
    languages: Option<Vec<String>>,
) -> Result<OnlineServicesDto, String> {
    blocking(move || subtitles::set_opensubtitles(enabled, languages)).await
}

#[tauri::command]
pub async fn opensubtitles_sign_in(
    username: String,
    password: String,
) -> Result<OnlineServicesDto, String> {
    scrubbed(subtitles::sign_in(username, password).await)
}

#[tauri::command]
pub async fn opensubtitles_sign_out() -> Result<OnlineServicesDto, String> {
    scrubbed(subtitles::sign_out().await)
}

/// Subtitles for the title in `languages` (none: the settings').
#[tauri::command]
pub async fn find_subtitles(
    kind: OnDemandKind,
    id: i64,
    languages: Vec<String>,
) -> Result<SubtitleSearchDto, String> {
    scrubbed(subtitles::search(item(kind, id), languages).await)
}

/// Downloads `file_id` for the title and keeps it with it.
#[tauri::command]
pub async fn download_subtitle(
    kind: OnDemandKind,
    id: i64,
    file_id: i64,
) -> Result<SubtitleDownloadDto, String> {
    scrubbed(subtitles::download(item(kind, id), file_id).await)
}

/// The files kept for the title, newest first.
#[tauri::command]
pub async fn kept_subtitles(kind: OnDemandKind, id: i64) -> Result<Vec<KeptSubtitleDto>, String> {
    blocking(move || subtitles::kept(item(kind, id))).await
}

/// The kept file `file_id` of the title, as `lookup` finds it.
fn kept_file(
    lookup: impl FnOnce(VodItem, i64) -> Result<Option<PathBuf>, String>,
    kind: OnDemandKind,
    id: i64,
    file_id: i64,
) -> Result<PathBuf, String> {
    lookup(item(kind, id), file_id)?.ok_or_else(|| "that subtitle is not kept for this title".to_owned())
}

/// Adds kept file `file_id` to the title, which must be the one playing,
/// selected and shown; `title` names the track and `lang` is its language.
#[tauri::command]
pub async fn add_subtitle_file(
    kind: OnDemandKind,
    id: i64,
    file_id: i64,
    title: String,
    lang: String,
) -> Result<PlayerOptionsDto, String> {
    let path = blocking(move || kept_file(subtitles::kept_path, kind, id, file_id)).await?;
    scrubbed(controller::add_external_subtitle(kind, id, &path, &title, &lang).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_needs_a_kept_file() {
        let mut asked = None;
        let r = kept_file(
            |item, file_id| {
                asked = Some((item, file_id));
                Ok(None)
            },
            OnDemandKind::Episode,
            7,
            7_999_999,
        );
        assert_eq!(r, Err("that subtitle is not kept for this title".to_owned()));
        assert_eq!(asked, Some((VodItem::episode(7), 7_999_999)));
        let found = PathBuf::from("/d/subtitles/7000001.srt");
        let r = kept_file(|_, _| Ok(Some(found.clone())), OnDemandKind::Movie, 7, 7_000_001);
        assert_eq!(r, Ok(found));
        let r = kept_file(|_, _| Err("no catalogue".to_owned()), OnDemandKind::Movie, 7, 1);
        assert_eq!(r, Err("no catalogue".to_owned()));
    }
}
