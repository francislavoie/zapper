//! Subtitles found online for a movie or an episode: what a title is looked
//! up by ([`subject`]), and which downloaded files each title offers
//! (`subtitle_downloads`).
//!
//! A row is the current catalogue's (see [`crate::catalog::origin`]), as
//! `vod_marks` are: another account's movie 7 is another film. The files
//! themselves are kept once per `file_id` (`online::subtitles::file_path`)
//! and shared by every title that offers them.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::catalog::origin::current_origin_sql;
use crate::catalog::vod_store::{ItemKind, VodItem};
use crate::ipc::types::KeptSubtitleDto;

/// What OpenSubtitles is asked about a title: the film, or for an episode
/// the show with its season and episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub item: VodItem,
    /// The film's cleaned name, or the show's.
    pub name: String,
    /// The film's year, or the show's.
    pub year: Option<i32>,
    /// The film's TMDB id, or the show's.
    pub tmdb_id: Option<i64>,
    pub season: Option<i32>,
    /// The first episode of an entry that holds several.
    pub episode: Option<i32>,
}

/// The `tmdb_id` in a title's kept details (`movie_details.info` or
/// `series_details.info`), when there is one that reads.
fn details_tmdb(info: Option<String>) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(&info?).ok()?;
    v.get("tmdb_id")?.as_i64().filter(|id| *id > 0)
}

/// The title as OpenSubtitles is asked about it; `None` when it is not in
/// the catalogue (or not any more). The TMDB id is the row's, else the kept
/// details'.
pub fn subject(conn: &Connection, item: VodItem) -> Result<Option<Subject>> {
    type Found = (
        String,
        Option<i32>,
        Option<i64>,
        Option<String>,
        Option<i32>,
        Option<i32>,
    );
    let found: Option<Found> = match item.kind {
        ItemKind::Movie => conn
            .query_row(
                "SELECT m.clean_name, m.year, m.tmdb_id, d.info, NULL, NULL
                 FROM movies m LEFT JOIN movie_details d ON d.movie_id = m.movie_id
                 WHERE m.movie_id = ?1 AND m.deleted_at IS NULL",
                [item.id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?,
        ItemKind::Episode => conn
            .query_row(
                "SELECT s.clean_name, s.year, s.tmdb_id, d.info, e.season, e.episode
                 FROM episodes e
                 JOIN series s ON s.series_id = e.series_id
                 LEFT JOIN series_details d ON d.series_id = s.series_id
                 WHERE e.episode_id = ?1 AND e.deleted_at IS NULL",
                [item.id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?,
        ItemKind::Series => None,
    };
    Ok(
        found.map(|(name, year, tmdb, info, season, episode)| Subject {
            item,
            name,
            year,
            tmdb_id: tmdb.filter(|id| *id > 0).or_else(|| details_tmdb(info)),
            season,
            episode,
        }),
    )
}

/// Notes that `item` offers the downloaded file `s`, under the current
/// catalogue. Downloaded again, it keeps its first date.
pub fn keep(conn: &Connection, item: VodItem, s: &KeptSubtitleDto) -> Result<()> {
    conn.execute(
        concat!(
            "INSERT INTO subtitle_downloads
               (origin, kind, item_id, file_id, language, release, hearing_impaired, downloaded_at)
             VALUES (",
            current_origin_sql!(),
            ", ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (origin, kind, item_id, file_id) DO UPDATE SET
               language = excluded.language, release = excluded.release,
               hearing_impaired = excluded.hearing_impaired"
        ),
        params![
            item.kind.as_str(),
            item.id,
            s.file_id,
            s.language,
            s.release,
            s.hearing_impaired,
            s.downloaded_at
        ],
    )?;
    Ok(())
}

const KEPT_COLUMNS: &str = "file_id, language, release, hearing_impaired, downloaded_at";

fn kept_row(r: &rusqlite::Row) -> rusqlite::Result<KeptSubtitleDto> {
    Ok(KeptSubtitleDto {
        file_id: r.get(0)?,
        language: r.get(1)?,
        release: r.get(2)?,
        hearing_impaired: r.get(3)?,
        downloaded_at: r.get(4)?,
    })
}

/// The files `item` offers under the current catalogue, newest first.
pub fn kept(conn: &Connection, item: VodItem) -> Result<Vec<KeptSubtitleDto>> {
    let sql = format!(
        "SELECT {KEPT_COLUMNS} FROM subtitle_downloads
         WHERE origin = {} AND kind = ?1 AND item_id = ?2
         ORDER BY downloaded_at DESC, file_id DESC",
        current_origin_sql!()
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![item.kind.as_str(), item.id], kept_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `file_id` as `item` offers it under the current catalogue, if it does.
pub fn kept_file(
    conn: &Connection,
    item: VodItem,
    file_id: i64,
) -> Result<Option<KeptSubtitleDto>> {
    let sql = format!(
        "SELECT {KEPT_COLUMNS} FROM subtitle_downloads
         WHERE origin = {} AND kind = ?1 AND item_id = ?2 AND file_id = ?3",
        current_origin_sql!()
    );
    Ok(conn
        .query_row(
            &sql,
            params![item.kind.as_str(), item.id, file_id],
            kept_row,
        )
        .optional()?)
}

/// How `file_id` was described when any title, under any catalogue, kept
/// it: the file is the same whoever downloaded it.
pub fn described(conn: &Connection, file_id: i64) -> Result<Option<KeptSubtitleDto>> {
    let sql = format!(
        "SELECT {KEPT_COLUMNS} FROM subtitle_downloads WHERE file_id = ?1
         ORDER BY downloaded_at LIMIT 1"
    );
    Ok(conn.query_row(&sql, [file_id], kept_row).optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{open_in_memory, origin};

    fn sub(file_id: i64, at: i64) -> KeptSubtitleDto {
        KeptSubtitleDto {
            file_id,
            language: "en".into(),
            release: format!("Film.0001.2021.{file_id}.TEST"),
            hearing_impaired: false,
            downloaded_at: at,
        }
    }

    #[test]
    fn a_subject_is_the_film_or_the_show() {
        let c = open_in_memory().unwrap();
        c.execute_batch(
            r#"INSERT INTO movies (movie_id, name, clean_name, year, added, tmdb_id)
                 VALUES (1, '|EN| Film 0001', 'Film 0001', 2021, 0, 900001),
                        (2, 'Film 0002', 'Film 0002', 2019, 0, NULL),
                        (3, 'Film 0003', 'Film 0003', NULL, 0, NULL);
               INSERT INTO movies (movie_id, name, clean_name, added, deleted_at)
                 VALUES (4, 'Film 0004', 'Film 0004', 0, 5);
               INSERT INTO movie_details (movie_id, fetched_at, info)
                 VALUES (3, 0, '{"tmdb_id": 900003}');
               INSERT INTO series (series_id, name, clean_name, year, added, tmdb_id)
                 VALUES (10, 'Show 0001', 'Show 0001', 2020, 0, NULL);
               INSERT INTO series_details (series_id, fetched_at, info)
                 VALUES (10, 0, '{"tmdb_id": 800001}');
               INSERT INTO episodes (episode_id, series_id, season, episode, episode_end)
                 VALUES (100, 10, 2, 1, 2);"#,
        )
        .unwrap();
        let s = subject(&c, VodItem::movie(1)).unwrap().unwrap();
        assert_eq!(
            (s.name.as_str(), s.year, s.tmdb_id),
            ("Film 0001", Some(2021), Some(900001))
        );
        assert_eq!((s.season, s.episode), (None, None));
        assert_eq!(
            subject(&c, VodItem::movie(2)).unwrap().unwrap().tmdb_id,
            None
        );
        assert_eq!(
            subject(&c, VodItem::movie(3)).unwrap().unwrap().tmdb_id,
            Some(900003),
            "the kept details' id"
        );
        assert_eq!(subject(&c, VodItem::movie(4)).unwrap(), None, "gone");
        assert_eq!(subject(&c, VodItem::movie(99)).unwrap(), None);
        let e = subject(&c, VodItem::episode(100)).unwrap().unwrap();
        assert_eq!(
            (e.name.as_str(), e.year, e.tmdb_id, e.season, e.episode),
            ("Show 0001", Some(2020), Some(800001), Some(2), Some(1))
        );
    }

    #[test]
    fn kept_rows_are_this_catalogues_newest_first() {
        let c = open_in_memory().unwrap();
        origin::record(&c, "xtream:aaaa").unwrap();
        let film = VodItem::movie(7);
        keep(&c, film, &sub(7000001, 100)).unwrap();
        keep(&c, film, &sub(7000002, 200)).unwrap();
        keep(&c, VodItem::episode(7), &sub(7000003, 300)).unwrap();
        let ids: Vec<i64> = kept(&c, film).unwrap().iter().map(|k| k.file_id).collect();
        assert_eq!(ids, [7000002, 7000001]);
        assert_eq!(
            kept_file(&c, film, 7000001).unwrap(),
            Some(sub(7000001, 100))
        );
        assert_eq!(kept_file(&c, film, 7000003).unwrap(), None, "the episode's");
        // Kept again: the first date stays.
        keep(&c, film, &sub(7000001, 900)).unwrap();
        assert_eq!(
            kept_file(&c, film, 7000001).unwrap().unwrap().downloaded_at,
            100
        );

        origin::record(&c, "xtream:bbbb").unwrap();
        assert!(kept(&c, film).unwrap().is_empty());
        assert_eq!(kept_file(&c, film, 7000001).unwrap(), None);
        assert_eq!(described(&c, 7000001).unwrap(), Some(sub(7000001, 100)));
        origin::record(&c, "xtream:aaaa").unwrap();
        assert_eq!(kept(&c, film).unwrap().len(), 2);
    }
}
