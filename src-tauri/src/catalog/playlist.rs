//! Writing a playlist into the mirror, and reading back what playback needs.
//!
//! The same diffing rules as [`crate::catalog::sync`] — upsert, never
//! truncate, soft-delete what vanished — because `channel_quality` and `activity` are
//! keyed on `stream_id` whichever kind of account produced it. The difference
//! is shape: an Xtream server hands over a parsed list, and a playlist is a
//! stream of tens of megabytes that is written as it is read, one entry at a
//! time, inside one transaction. Nothing of it is held but the set of ids
//! already seen.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::catalog::epg_match::Matcher;
use crate::catalog::series_sync::{NewEpisode, NewSeries, SeriesWriter};
use crate::catalog::sync::{clean_name, SyncStats};
use crate::catalog::vod_sync::{self, MovieWriter, NewMovie};
use crate::catalog::origin;
use crate::m3u::kind::{classify, episode_name, EntryKind};
use crate::m3u::{catchup, ids, Entry, Header};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What a playlist write did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PlaylistStats {
    pub categories: usize,
    pub channels: SyncStats,
    /// Entries with usable catch-up.
    pub with_catchup: usize,
    /// The on-demand entries, which are movies and not channels.
    pub movies: SyncStats,
    pub movie_categories: usize,
    /// The shows the episode entries were gathered into.
    pub series: SyncStats,
    pub series_categories: usize,
    /// The episode entries, which belong to shows and are not channels.
    pub episodes: usize,
}

/// Where the playlist's own `x-tvg-url` is remembered between phases and
/// passes, in the `meta` table, one URL per line.
const HEADER_EPG_KEY: &str = "m3u_header_epg_urls";

/// One open write of a playlist. Entries are pushed as the parser produces
/// them; [`finish`](Self::finish) soft-deletes what was not seen, writes the
/// categories and commits. Dropping it without finishing rolls everything
/// back, which is what a download that fails halfway should do.
pub struct PlaylistWriter<'c> {
    tx: Transaction<'c>,
    /// The movies of the same playlist, written inside the same transaction
    /// so that a download that fails halfway undoes them too.
    movies: MovieWriter<'c>,
    /// Its shows and their episodes, likewise. A playlist lists every
    /// episode, so this write owns them.
    series: SeriesWriter<'c>,
    /// The catalogue this write is filling, when the caller knows it. See
    /// [`set_origin`](Self::set_origin).
    origin: Option<String>,
    existing: HashSet<i64>,
    seen: HashSet<i64>,
    /// How many times each identity (by hash) has appeared, for the ordinal
    /// that tells duplicates apart.
    occurrences: HashMap<u64, u32>,
    /// Groups in order of first appearance, which is the order the playlist's
    /// author meant them in.
    groups: Vec<String>,
    group_set: HashSet<String>,
    position: i64,
    stats: PlaylistStats,
}

impl<'c> PlaylistWriter<'c> {
    pub fn begin(conn: &'c Connection) -> Result<Self> {
        let tx = conn.unchecked_transaction()?;
        let existing: HashSet<i64> = {
            let mut s = tx.prepare("SELECT stream_id FROM channels WHERE deleted_at IS NULL")?;
            let ids = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            ids
        };
        let mut movies = MovieWriter::begin_on(conn)?;
        // A playlist that still has channels but no movies any more has
        // dropped them; the guard against a bad minute is the empty
        // playlist's, in `finish`.
        movies.accept_empty();
        // Shows likewise, and the episodes it pushes are all there are.
        let mut series = SeriesWriter::begin_on(conn)?;
        series.accept_empty();
        series.own_episodes();
        Ok(Self {
            tx,
            movies,
            series,
            origin: None,
            existing,
            seen: HashSet::new(),
            occurrences: HashMap::new(),
            groups: Vec::new(),
            group_set: HashSet::new(),
            position: 0,
            stats: PlaylistStats::default(),
        })
    }

    /// Names the catalogue being written, so that [`finish`](Self::finish)
    /// records it before it lands the movies: the movie list compares its
    /// own record of where it came from with the one on file, and a write
    /// that only recorded the new catalogue afterwards would see no change
    /// of account until the pass after.
    pub fn set_origin(&mut self, origin: String) {
        self.origin = Some(origin);
    }

    pub fn push(&mut self, e: Entry) -> Result<()> {
        // See `m3u::ids`: the id comes from what the entry is called, not
        // from its URL, which may carry a token that changes every download.
        let identity = ids::identity(&e);
        match classify(&e) {
            EntryKind::Movie => self.push_movie(e, &identity),
            EntryKind::Episode => self.push_episode(e, &identity),
            EntryKind::Channel => self.push_identified(e, &identity),
        }
    }

    /// The ordinal that tells entries of one identity apart.
    fn occurrence(&mut self, identity: &str) -> u32 {
        let n = self
            .occurrences
            .entry(ids::fnv1a(identity.as_bytes()))
            .or_insert(0);
        let this = *n;
        *n += 1;
        this
    }

    fn push_movie(&mut self, e: Entry, identity: &str) -> Result<()> {
        // Apart from channels' ids by the prefix: the same name can be a
        // channel and a film, and their ids are different tables' keys.
        let identity = format!("movie\u{0}{identity}");
        let occurrence = self.occurrence(&identity);
        let movie = NewMovie {
            movie_id: ids::stream_id(&identity, occurrence),
            year: vod_sync::year_from_name(&e.name),
            poster_url: e.logo,
            stream_url: Some(e.url),
            user_agent: e.user_agent,
            referrer: e.referrer,
            name: e.name,
            ..NewMovie::default()
        };
        self.movies.push(&movie, e.group.as_deref())
    }

    /// An episode, under the show its group and name gather it into: the
    /// name before its number, or the group's own name when there is none
    /// before it. One with neither is a movie, having no show to be in.
    fn push_episode(&mut self, e: Entry, identity: &str) -> Result<()> {
        let Some(parts) = episode_name(&e.name) else {
            return self.push_movie(e, identity);
        };
        let group = e.group.as_deref().map(str::trim).unwrap_or_default();
        let show = if parts.show.is_empty() {
            group.to_owned()
        } else {
            parts.show
        };
        if show.is_empty() || vod_sync::is_separator(&show) {
            return self.push_movie(e, identity);
        }
        // Spelled the same, case and spacing aside, in the same group: one
        // show. The same name in two groups is two, as a person sorting
        // them would have it.
        let loose = show
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let series_id = ids::stream_id(&format!("series\u{0}{group}\u{0}{loose}"), 0);
        self.series.push_series(
            &NewSeries {
                series_id,
                year: vod_sync::year_from_name(&show),
                poster_url: e.logo.clone(),
                name: show,
                ..NewSeries::default()
            },
            e.group.as_deref(),
        )?;
        let identity = format!("episode\u{0}{identity}");
        let occurrence = self.occurrence(&identity);
        self.series.push_episode(&NewEpisode {
            episode_id: ids::stream_id(&identity, occurrence),
            series_id,
            season: parts.season,
            episode: parts.episode,
            episode_end: parts.episode_end,
            title: parts.title,
            stream_url: Some(e.url),
            user_agent: e.user_agent,
            referrer: e.referrer,
            ..NewEpisode::default()
        })
    }

    /// [`push`](Self::push), for an entry whose source knows better than a
    /// display name what makes it the same channel next time: an HDHomeRun's
    /// device and guide number, a Tvheadend channel's UUID. Those survive a
    /// rename and a retag, which a name and a group do not. The identity is
    /// hashed into the same id range as a playlist's, so the mirror cannot
    /// tell — and does not need to — which kind of source wrote a row.
    pub fn push_identified(&mut self, e: Entry, identity: &str) -> Result<()> {
        self.position += 1;
        let occurrence = self.occurrence(identity);
        let stream_id = ids::stream_id(identity, occurrence);
        let category = e.group.as_deref().map(|g| {
            if self.group_set.insert(g.to_owned()) {
                self.groups.push(g.to_owned());
            }
            ids::category_id(g)
        });
        let mode = catchup::resolve(e.catchup.as_deref(), e.catchup_source.as_deref(), &e.url);
        if mode.is_some() {
            self.stats.with_catchup += 1;
        }
        // The playlist's order is meaningful (it is how its author arranged
        // the channels), and `num` is what the listing sorts by. So an
        // entry's channel number is its `tvg-chno` when it has one and its
        // position otherwise.
        let num = e.chno.unwrap_or(self.position);

        self.tx
            .prepare_cached(
                "INSERT INTO channels
                   (stream_id, category_id, name, clean_name, num, logo_url,
                    epg_channel_id, has_archive, archive_days, is_separator, deleted_at,
                    stream_url, tvg_id, tvg_name, catchup_mode, catchup_source,
                    http_user_agent, http_referrer, num_label)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL,?11,?12,?13,?14,?15,?16,?17,?18)
                 ON CONFLICT(stream_id) DO UPDATE SET
                   category_id=excluded.category_id, name=excluded.name,
                   clean_name=excluded.clean_name, num=excluded.num,
                   logo_url=excluded.logo_url,
                   -- Keep the guide channel the last EPG pass matched, unless
                   -- what it was matched on has changed. Otherwise every
                   -- refresh would drop matched channels back to their raw
                   -- tvg-id until the guide phase ran again.
                   epg_channel_id = CASE
                       WHEN channels.epg_channel_id IS NOT NULL
                        AND channels.tvg_id IS excluded.tvg_id
                        AND channels.tvg_name IS excluded.tvg_name
                        AND channels.name = excluded.name
                       THEN channels.epg_channel_id
                       ELSE excluded.epg_channel_id END,
                   has_archive=excluded.has_archive, archive_days=excluded.archive_days,
                   is_separator=excluded.is_separator, deleted_at=NULL,
                   stream_url=excluded.stream_url, tvg_id=excluded.tvg_id,
                   tvg_name=excluded.tvg_name, catchup_mode=excluded.catchup_mode,
                   catchup_source=excluded.catchup_source,
                   http_user_agent=excluded.http_user_agent,
                   http_referrer=excluded.http_referrer,
                   num_label=excluded.num_label",
            )?
            .execute(params![
                stream_id,
                category,
                e.name,
                clean_name(&e.name),
                num,
                e.logo,
                e.tvg_id,
                i64::from(mode.is_some()),
                mode.and(e.catchup_days),
                i64::from(e.is_separator()),
                e.url,
                e.tvg_id,
                e.tvg_name,
                mode.map(catchup::Mode::as_str),
                e.catchup_source,
                e.user_agent,
                e.referrer,
                // Only with the number it describes: a label beside a
                // position would name a number the row does not have.
                e.chno.and(e.chno_label),
            ])?;

        if self.seen.insert(stream_id) {
            if self.existing.contains(&stream_id) {
                self.stats.channels.updated += 1;
            } else {
                self.stats.channels.inserted += 1;
            }
        }
        Ok(())
    }

    /// Puts the categories in `order` first, in that order, and the rest
    /// after them as they appeared.
    ///
    /// A playlist's group order is its author's, and first appearance is the
    /// right reading of it. A lineup or a server has an order of its own to
    /// give instead — an HDHomeRun's favourites before the rest, a
    /// Tvheadend's tags as its administrator numbered them — and pushing the
    /// channels in that order to get it would cost them their own.
    pub fn rank_groups(&mut self, order: &[String]) {
        let rank = |g: &String| order.iter().position(|o| o == g).unwrap_or(order.len());
        // Stable, so the unranked keep their order of appearance.
        self.groups.sort_by_key(rank);
    }

    /// Commits the write. Refuses a playlist with no channels, movies or
    /// episodes in it outright: an empty answer is far more likely to be a
    /// provider having a bad minute than a subscription with nothing, and
    /// accepting it would soft-delete the whole catalogue.
    pub fn finish(mut self, header: &Header) -> Result<PlaylistStats> {
        if self.seen.is_empty() && self.movies.seen() == 0 && self.series.seen() == 0 {
            return Err(anyhow!(
                "the playlist had no channels in it; keeping the ones already here"
            ));
        }
        let at = now();
        for gone in self.existing.difference(&self.seen) {
            self.tx.execute(
                "UPDATE channels SET deleted_at=?1 WHERE stream_id=?2",
                params![at, gone],
            )?;
            self.stats.channels.deleted += 1;
        }

        // Categories are derived from the groups, so they are replaced
        // wholesale: a group nobody is in any more is not a category. That
        // includes an Xtream server's categories after switching to a playlist
        // — their channels have just been soft-deleted above.
        self.tx.execute("DELETE FROM categories", [])?;
        for (i, g) in self.groups.iter().enumerate() {
            self.tx.execute(
                "INSERT INTO categories (id, name, sort) VALUES (?1, ?2, ?3)",
                params![ids::category_id(g), g, i as i64],
            )?;
        }
        self.stats.categories = self.groups.len();

        if let Some(o) = &self.origin {
            origin::record(&self.tx, o)?;
        }
        let (movies, categories) = self.movies.finish(at)?;
        self.stats.movies = movies;
        self.stats.movie_categories = categories;
        // After the origin, as the movies: the show list compares its own
        // record of where it came from with this write's, and judges what
        // the shows keep (episodes, watched marks) by it.
        let shows = self.series.finish(at)?;
        self.stats.series = shows.stats;
        self.stats.series_categories = shows.categories;
        self.stats.episodes = shows.episodes;

        self.tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![HEADER_EPG_KEY, header.epg_urls.join("\n")],
        )?;
        self.tx.commit()?;
        Ok(self.stats)
    }
}

/// The guide URLs the last playlist named for itself.
pub fn header_epg_urls(conn: &Connection) -> Result<Vec<String>> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [HEADER_EPG_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Resolves every playlist channel's guide channel. Returns `(matched,
/// total)` over live playlist channels.
///
/// With no guide channels known at all (no guide configured, or none has
/// loaded yet) nothing is changed: the `tvg-id` each channel was written
/// with is the best guess there is.
pub fn match_epg(conn: &Connection) -> Result<(usize, usize)> {
    let ids: Vec<String> = conn
        .prepare("SELECT DISTINCT channel_ref FROM epg")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let names: Vec<(String, String)> = conn
        .prepare("SELECT channel_id, name FROM epg_channel_names ORDER BY rowid")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let matcher = Matcher::new(ids, names);

    type Row = (i64, Option<String>, Option<String>, String, Option<String>);
    let rows: Vec<Row> = conn
        .prepare(
            "SELECT stream_id, tvg_id, tvg_name, name, epg_channel_id FROM channels
             WHERE stream_url IS NOT NULL AND deleted_at IS NULL AND is_separator = 0",
        )?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    let total = rows.len();
    if matcher.is_empty() {
        return Ok((0, total));
    }

    let tx = conn.unchecked_transaction()?;
    let mut matched = 0;
    {
        let mut update =
            tx.prepare("UPDATE channels SET epg_channel_id = ?2 WHERE stream_id = ?1")?;
        for (id, tvg_id, tvg_name, name, current) in rows {
            let found = matcher.resolve(tvg_id.as_deref(), tvg_name.as_deref(), &name);
            if found.is_some() {
                matched += 1;
            }
            let want = found.or(tvg_id);
            if want != current {
                update.execute(params![id, want])?;
            }
        }
    }
    tx.commit()?;
    Ok((matched, total))
}

/// What playback needs to open a playlist channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRow {
    pub url: String,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    pub catchup_mode: Option<String>,
    pub catchup_source: Option<String>,
}

/// `None` for an id with no playlist channel behind it — including an Xtream
/// channel left in the mirror from before the account changed.
pub fn stream_row(conn: &Connection, stream_id: i64) -> Result<Option<StreamRow>> {
    Ok(conn
        .query_row(
            "SELECT stream_url, http_user_agent, http_referrer, catchup_mode, catchup_source
             FROM channels WHERE stream_id = ?1 AND stream_url IS NOT NULL",
            [stream_id],
            |r| {
                Ok(StreamRow {
                    url: r.get(0)?,
                    user_agent: r.get(1)?,
                    referrer: r.get(2)?,
                    catchup_mode: r.get(3)?,
                    catchup_source: r.get(4)?,
                })
            },
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;
    use crate::m3u::parse;

    const FIXTURE: &str = include_str!("../../tests/fixtures/sample.m3u");

    fn write(c: &Connection, text: &str) -> Result<PlaylistStats> {
        let mut w = PlaylistWriter::begin(c)?;
        let header = parse::parse(text.as_bytes(), |e| w.push(e))?;
        w.finish(&header)
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn the_fixture_lands_with_categories_in_playlist_order() {
        let c = open_in_memory().unwrap();
        let s = write(&c, FIXTURE).unwrap();
        assert_eq!(s.channels.inserted, 8);
        // "News;Europe" files under News, beside the entry that is only News.
        assert_eq!(s.categories, 5);
        let cats: Vec<String> = c
            .prepare("SELECT name FROM categories ORDER BY sort")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(cats, ["UK", "News", "Xtream", "Music", "Radio"]);
    }

    #[test]
    fn ids_are_stable_across_a_reparse_so_history_survives() {
        let c = open_in_memory().unwrap();
        write(&c, FIXTURE).unwrap();
        let before: Vec<i64> = c
            .prepare("SELECT stream_id FROM channels ORDER BY stream_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let s = write(&c, FIXTURE).unwrap();
        assert_eq!(
            (s.channels.inserted, s.channels.updated, s.channels.deleted),
            (0, 8, 0)
        );
        let after: Vec<i64> = c
            .prepare("SELECT stream_id FROM channels ORDER BY stream_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn sd_hd_and_backup_copies_of_one_tvg_id_are_separate_channels() {
        let c = open_in_memory().unwrap();
        write(&c, FIXTURE).unwrap();
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM channels WHERE tvg_id = 'nw1.uk'"),
            2
        );
    }

    #[test]
    fn a_url_listed_twice_is_two_channels() {
        let c = open_in_memory().unwrap();
        let s = write(
            &c,
            "#EXTM3U\n#EXTINF:-1 group-title=\"A\",One\nhttp://s/x\n#EXTINF:-1 group-title=\"B\",One again\nhttp://s/x\n",
        )
        .unwrap();
        assert_eq!(s.channels.inserted, 2);
    }

    /// `(name, stream_id)` of every live channel, by name then id.
    fn ids_by_name(c: &Connection) -> Vec<(String, i64)> {
        c.prepare(
            "SELECT name, stream_id FROM channels WHERE deleted_at IS NULL ORDER BY name, stream_id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    /// A provider-style playlist whose every URL carries this download's
    /// token, in the query and (for the Xtream export) in the path.
    fn tokened(token: &str) -> String {
        format!(
            "#EXTM3U\n\
             #EXTINF:-1 tvg-id=\"nw1.uk\" group-title=\"UK\",Northwind One HD\n\
             http://cdn.example/nw1/index.m3u8?token={token}&e=1700000000\n\
             #EXTINF:-1 group-title=\"News\",Eurowire\n\
             http://p.example:8080/live/user-{token}/pass-{token}/77.ts\n\
             http://cdn.example/unnamed/feed.ts?sig={token}\n"
        )
    }

    #[test]
    fn rotated_tokens_keep_every_id() {
        let c = open_in_memory().unwrap();
        write(&c, &tokened("aaa111")).unwrap();
        let before = ids_by_name(&c);
        let s = write(&c, &tokened("zzz999")).unwrap();
        assert_eq!(ids_by_name(&c), before, "a new token minted new channels");
        assert_eq!((s.channels.inserted, s.channels.deleted), (0, 0));
        // And the new URL is what plays.
        let url: String = c
            .query_row(
                "SELECT stream_url FROM channels WHERE name = 'Eurowire'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(url.contains("zzz999"), "{url}");
    }

    #[test]
    fn reordering_channels_keeps_their_ids() {
        let a = "#EXTM3U\n#EXTINF:-1 group-title=\"G\",Alpha\nhttp://s/a\n\
                 #EXTINF:-1 group-title=\"G\",Beta\nhttp://s/b\n";
        let b = "#EXTM3U\n#EXTINF:-1 group-title=\"G\",Beta\nhttp://s/b\n\
                 #EXTINF:-1 group-title=\"G\",Alpha\nhttp://s/a\n";
        let c = open_in_memory().unwrap();
        write(&c, a).unwrap();
        let before = ids_by_name(&c);
        write(&c, b).unwrap();
        assert_eq!(ids_by_name(&c), before);
    }

    #[test]
    fn same_named_duplicates_get_distinct_ids_that_stay_put() {
        let text = |t: &str| {
            format!(
                "#EXTM3U\n#EXTINF:-1 group-title=\"Sport\",Cobalt Sports\nhttp://s/main?token={t}\n\
                 #EXTINF:-1 group-title=\"Sport\",Other\nhttp://s/o\n\
                 #EXTINF:-1 group-title=\"Sport\",Cobalt Sports\nhttp://s/backup?token={t}\n"
            )
        };
        let c = open_in_memory().unwrap();
        let s = write(&c, &text("one")).unwrap();
        assert_eq!(s.channels.inserted, 3);
        let first = ids_by_name(&c);
        let cobalt: Vec<i64> = first
            .iter()
            .filter(|(n, _)| n == "Cobalt Sports")
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(cobalt.len(), 2);
        assert_ne!(cobalt[0], cobalt[1]);
        write(&c, &text("two")).unwrap();
        assert_eq!(ids_by_name(&c), first);
        // The first-listed copy keeps the first ordinal: it is the main feed
        // that history most likely belongs to.
        let main: i64 = c
            .query_row(
                "SELECT stream_id FROM channels WHERE stream_url LIKE 'http://s/main%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ident = crate::m3u::ids::identity(&crate::m3u::Entry {
            name: "Cobalt Sports".into(),
            named: true,
            group: Some("Sport".into()),
            ..Default::default()
        });
        assert_eq!(main, crate::m3u::ids::stream_id(&ident, 0));
    }

    #[test]
    fn catchup_is_only_flagged_where_a_url_can_be_built() {
        let c = open_in_memory().unwrap();
        let s = write(&c, FIXTURE).unwrap();
        // Eurowire (default + source), Meridian 24 (shift), Xtream Channel (xc). Not the
        // "vod" one, and not the ones that never asked.
        assert_eq!(s.with_catchup, 3);
        let (archive, days, mode): (i64, Option<i64>, Option<String>) = c
            .query_row(
                "SELECT has_archive, archive_days, catchup_mode FROM channels WHERE name = 'Meridian 24'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        // Days from the header's default.
        assert_eq!(
            (archive, days, mode.as_deref()),
            (1, Some(3), Some("shift"))
        );
        let vod: (i64, Option<i64>) = c
            .query_row(
                "SELECT has_archive, archive_days FROM channels WHERE name = 'Unsupported Catchup'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(vod, (0, None));
    }

    #[test]
    fn per_channel_headers_are_stored_for_playback() {
        let c = open_in_memory().unwrap();
        write(&c, FIXTURE).unwrap();
        let id: i64 = c
            .query_row(
                "SELECT stream_id FROM channels WHERE name = 'Meridian 24'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let row = stream_row(&c, id).unwrap().unwrap();
        assert_eq!(row.url, "http://stream.example/m24/index.m3u8");
        assert_eq!(row.user_agent.as_deref(), Some("Mozilla/5.0 zapper-test"));
        assert_eq!(row.referrer.as_deref(), Some("https://meridian24.example/"));
        assert_eq!(stream_row(&c, 1).unwrap(), None);
    }

    #[test]
    fn vanished_entries_are_soft_deleted_and_an_empty_playlist_is_refused() {
        let c = open_in_memory().unwrap();
        write(&c, FIXTURE).unwrap();
        // The same Northwind One HD entry as the fixture's (same tvg-id, name and
        // group, so the same channel) and nothing else.
        let s = write(
            &c,
            "#EXTM3U\n#EXTINF:-1 tvg-id=\"nw1.uk\" group-title=\"UK\",Northwind One HD\n\
             http://stream.example/live/nw1.m3u8\n",
        )
        .unwrap();
        assert_eq!((s.channels.updated, s.channels.deleted), (1, 7));
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM channels WHERE deleted_at IS NULL"),
            1
        );
        // A group nobody is in is no longer a category.
        assert_eq!(count(&c, "SELECT COUNT(*) FROM categories"), 1);

        let err = write(&c, "#EXTM3U\n").unwrap_err().to_string();
        assert!(err.contains("no channels"), "{err}");
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM channels WHERE deleted_at IS NULL"),
            1
        );
    }

    /// Two channels, an episode's file, and two films in one group.
    const WITH_MOVIES: &str = "#EXTM3U\n\
        #EXTINF:-1 group-title=\"News\",Eurowire\nhttp://p.example/live/u/p/1.ts\n\
        #EXTINF:-1 group-title=\"News\",Meridian 24\nhttp://p.example/live/u/p/2.ts\n\
        #EXTINF:-1 group-title=\"Shows\",A Show S01E01\nhttp://p.example/series/u/p/3.mkv\n\
        #EXTINF:-1 group-title=\"Films\" tvg-logo=\"http://img.example/a.jpg\",A Film (2021)\nhttp://p.example/movie/u/p/4.mkv\n\
        #EXTINF:-1 group-title=\"Films\",Another Film\nhttp://p.example/movie/u/p/5.mp4\n";

    fn names(c: &Connection, sql: &str) -> Vec<String> {
        c.prepare(sql)
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn movies_in_a_playlist_go_to_movies_not_channels() {
        let c = open_in_memory().unwrap();
        let s = write(&c, WITH_MOVIES).unwrap();
        // The episode is a show's, not a channel.
        assert_eq!(s.channels.inserted, 2);
        assert_eq!(s.movies.inserted, 2);
        assert_eq!(s.movie_categories, 1);
        assert_eq!(
            names(&c, "SELECT name FROM vod_categories WHERE kind = 'movie'"),
            ["Films"]
        );
        let cats = names(&c, "SELECT name FROM categories ORDER BY sort");
        assert_eq!(cats, ["News"]);
        let (year, poster, url): (Option<i32>, Option<String>, Option<String>) = c
            .query_row(
                "SELECT year, poster_url, stream_url FROM movies WHERE name = 'A Film (2021)'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(year, Some(2021));
        assert_eq!(poster.as_deref(), Some("http://img.example/a.jpg"));
        assert_eq!(url.as_deref(), Some("http://p.example/movie/u/p/4.mkv"));
    }

    #[test]
    fn a_playlist_of_only_movies_is_not_refused() {
        let c = open_in_memory().unwrap();
        let s = write(
            &c,
            "#EXTM3U\n#EXTINF:-1 group-title=\"Films\",One\nhttp://h/movie/u/p/1.mkv\n\
             #EXTINF:-1 group-title=\"Films\",Two\nhttp://h/movie/u/p/2.mkv\n",
        )
        .unwrap();
        assert_eq!(s.movies.inserted, 2);
        assert_eq!(s.channels.inserted, 0);
    }

    #[test]
    fn ids_of_playlist_movies_survive_a_reparse() {
        let c = open_in_memory().unwrap();
        write(&c, WITH_MOVIES).unwrap();
        let ids = |c: &Connection| -> Vec<i64> {
            c.prepare("SELECT movie_id FROM movies ORDER BY movie_id")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let before = ids(&c);
        // A new download's token does not make new titles.
        let s = write(&c, &WITH_MOVIES.replace("/u/p/", "/u2/p2/")).unwrap();
        assert_eq!(ids(&c), before);
        assert_eq!(
            (s.movies.inserted, s.movies.updated, s.movies.deleted),
            (0, 2, 0)
        );
    }

    #[test]
    fn a_playlist_that_drops_its_movies_drops_them_from_the_mirror() {
        let c = open_in_memory().unwrap();
        write(&c, WITH_MOVIES).unwrap();
        let s = write(&c, FIXTURE).unwrap();
        assert_eq!(s.movies.deleted, 2);
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM movies WHERE deleted_at IS NULL"),
            0
        );
    }

    /// An episode as stored: its show's name, season, episode, last episode
    /// and title.
    type EpisodeRow = (String, i32, i32, Option<i32>, Option<String>);

    fn episode_rows(c: &Connection) -> Vec<EpisodeRow> {
        c.prepare(
            "SELECT s.name, e.season, e.episode, e.episode_end, e.title
               FROM episodes e JOIN series s USING (series_id)
              WHERE e.deleted_at IS NULL
              ORDER BY s.name, e.season, e.episode",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    #[test]
    fn episodes_in_a_playlist_go_to_shows_not_channels() {
        let c = open_in_memory().unwrap();
        let s = write(&c, WITH_MOVIES).unwrap();
        assert_eq!(s.channels.inserted, 2);
        assert_eq!(s.movies.inserted, 2);
        assert_eq!(s.series.inserted, 1);
        assert_eq!((s.series_categories, s.episodes), (1, 1));
        assert_eq!(
            names(&c, "SELECT name FROM vod_categories WHERE kind = 'series'"),
            ["Shows"]
        );
        assert_eq!(episode_rows(&c), [("A Show".into(), 1, 1, None, None)]);
        let (url, episode_count): (Option<String>, Option<i64>) = c
            .query_row(
                "SELECT e.stream_url, s.episode_count FROM episodes e JOIN series s USING (series_id)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(url.as_deref(), Some("http://p.example/series/u/p/3.mkv"));
        assert_eq!(episode_count, Some(1));
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM channels WHERE name LIKE 'A Show%'"),
            0
        );
    }

    #[test]
    fn the_same_show_in_two_groups_is_two_shows() {
        let c = open_in_memory().unwrap();
        let s = write(
            &c,
            "#EXTM3U\n\
             #EXTINF:-1 group-title=\"Drama\",A Show S01E01\nhttp://h/series/u/p/1.mkv\n\
             #EXTINF:-1 group-title=\"Drama\",a  show S01E02 Night Shift\nhttp://h/series/u/p/2.mkv\n\
             #EXTINF:-1 group-title=\"Kids\",A Show S01E01\nhttp://h/series/u/p/3.mkv\n",
        )
        .unwrap();
        assert_eq!((s.series.inserted, s.episodes), (2, 3));
        let shows: Vec<(String, String, i64)> = c
            .prepare(
                "SELECT s.name, v.name, s.episode_count FROM series s
                   JOIN vod_categories v ON v.kind = 'series' AND v.id = s.category_id
                  ORDER BY v.sort",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        // The first spelling names the show; case and spacing aside, the
        // second is the same one.
        assert_eq!(
            shows,
            [
                ("A Show".to_string(), "Drama".to_string(), 2),
                ("A Show".to_string(), "Kids".to_string(), 1),
            ]
        );
    }

    #[test]
    fn a_show_named_only_by_its_code_takes_its_groups_name() {
        let c = open_in_memory().unwrap();
        let s = write(
            &c,
            "#EXTM3U\n\
             #EXTINF:-1 group-title=\"Lantern Quay\",S01E01 Pilot\nhttp://h/vod/1.mkv\n\
             #EXTINF:-1 group-title=\"Lantern Quay\",S01E02\nhttp://h/vod/2.mkv\n\
             #EXTINF:-1,S01E03 No Group\nhttp://h/vod/3.mkv\n",
        )
        .unwrap();
        assert_eq!((s.series.inserted, s.episodes), (1, 2));
        assert_eq!(
            episode_rows(&c),
            [
                ("Lantern Quay".into(), 1, 1, None, Some("Pilot".into())),
                ("Lantern Quay".into(), 1, 2, None, None),
            ]
        );
        // With no group either, there is no show to put it in: a film.
        assert_eq!(s.movies.inserted, 1);
    }

    #[test]
    fn a_multi_episode_entry_is_one_episode_with_its_range() {
        let c = open_in_memory().unwrap();
        let s = write(
            &c,
            "#EXTM3U\n#EXTINF:-1 group-title=\"Shows\",A Show S01E01-E02\nhttp://h/series/u/p/1.mkv\n",
        )
        .unwrap();
        assert_eq!(s.episodes, 1);
        assert_eq!(episode_rows(&c), [("A Show".into(), 1, 1, Some(2), None)]);
    }

    #[test]
    fn a_playlist_of_only_episodes_is_not_refused() {
        let c = open_in_memory().unwrap();
        let s = write(
            &c,
            "#EXTM3U\n#EXTINF:-1 group-title=\"Shows\",A Show S01E01\nhttp://h/series/u/p/1.mkv\n\
             #EXTINF:-1 group-title=\"Shows\",A Show S01E02\nhttp://h/series/u/p/2.mkv\n",
        )
        .unwrap();
        assert_eq!((s.series.inserted, s.episodes), (1, 2));
        assert_eq!((s.channels.inserted, s.movies.inserted), (0, 0));
    }

    #[test]
    fn ids_of_playlist_episodes_survive_a_reparse() {
        let c = open_in_memory().unwrap();
        write(&c, WITH_MOVIES).unwrap();
        let ids = |c: &Connection| -> Vec<(i64, i64)> {
            c.prepare("SELECT series_id, episode_id FROM episodes ORDER BY episode_id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let before = ids(&c);
        // A new download's token does not make new shows or episodes.
        let s = write(&c, &WITH_MOVIES.replace("/u/p/", "/u2/p2/")).unwrap();
        assert_eq!(ids(&c), before);
        assert_eq!(
            (s.series.inserted, s.series.updated, s.series.deleted),
            (0, 1, 0)
        );
        let url: String = c
            .query_row("SELECT stream_url FROM episodes", [], |r| r.get(0))
            .unwrap();
        assert!(url.contains("/u2/p2/"), "{url}");
    }

    #[test]
    fn a_playlist_that_drops_its_episodes_drops_them() {
        let c = open_in_memory().unwrap();
        write(&c, WITH_MOVIES).unwrap();
        let s = write(&c, FIXTURE).unwrap();
        assert_eq!(s.series.deleted, 1);
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM series WHERE deleted_at IS NULL"),
            0
        );
        assert_eq!(
            count(&c, "SELECT COUNT(*) FROM episodes WHERE deleted_at IS NULL"),
            0
        );
    }

    /// The show lands knowing which catalogue it is from: switching to
    /// another playlist that has a show of the same group and name (and so
    /// the same id) is seen as a switch in that very write, so nothing of
    /// the old catalogue's is carried over onto the new one's show.
    #[test]
    fn a_switch_to_a_playlist_with_the_same_show_is_seen_in_that_write() {
        let c = open_in_memory().unwrap();
        let with = |location: &str, text: &str| {
            let mut w = PlaylistWriter::begin(&c).unwrap();
            w.set_origin(origin::of_playlist(location));
            let header = parse::parse(text.as_bytes(), |e| w.push(e)).unwrap();
            w.finish(&header).unwrap()
        };
        let show_id = |c: &Connection| -> i64 {
            c.query_row("SELECT series_id FROM series WHERE deleted_at IS NULL", [], |r| r.get(0))
                .unwrap()
        };
        with(
            "http://a.example/list.m3u",
            "#EXTM3U\n#EXTINF:-1 group-title=\"Shows\",A Show S01E01\nhttp://a.example/vod/1.mkv\n\
             #EXTINF:-1 group-title=\"Shows\",A Show S01E02\nhttp://a.example/vod/2.mkv\n",
        );
        let before = show_id(&c);
        let s = with(
            "http://b.example/list.m3u",
            "#EXTM3U\n#EXTINF:-1 group-title=\"Shows\",A Show S01E01 Other\nhttp://b.example/vod/9.mkv\n",
        );
        assert_eq!(show_id(&c), before, "the same group and name is the same id");
        assert_eq!(s.series.updated, 1);
        // A write that saw the switch removes the old catalogue's episodes
        // outright rather than keeping them soft-deleted.
        assert_eq!(count(&c, "SELECT COUNT(*) FROM episodes"), 1);
        // And it saw it as this catalogue's: shows landed before the origin
        // was recorded would be filed under the old one, and judged by its
        // marks.
        assert_eq!(
            crate::catalog::series_sync::series_origin(&c).unwrap(),
            vod_sync::movies_origin(&c).unwrap(),
            "the shows were landed under the new catalogue"
        );
    }

    #[test]
    fn the_header_guide_urls_are_remembered() {
        let c = open_in_memory().unwrap();
        write(&c, FIXTURE).unwrap();
        assert_eq!(
            header_epg_urls(&c).unwrap(),
            [
                "http://epg.example/guide.xml.gz",
                "http://epg2.example/more.xml"
            ]
        );
    }

    #[test]
    fn epg_matching_resolves_by_id_then_name_and_survives_a_refresh() {
        let c = open_in_memory().unwrap();
        write(&c, FIXTURE).unwrap();
        c.execute_batch(
            "INSERT INTO epg_channel_names VALUES ('NW1.uk', 'Northwind One');
             INSERT INTO epg_channel_names VALUES ('meridian24.fr', 'Meridian 24');
             INSERT INTO epg_channel_names VALUES ('eurowire.eu', 'Eurowire, English');",
        )
        .unwrap();
        let (matched, total) = match_epg(&c).unwrap();
        // Two Northwind One copies (by case-insensitive tvg-id), Meridian 24 and Eurowire
        // by name.
        assert_eq!(matched, 4, "of {total}");
        let epg = |name: &str| -> Option<String> {
            c.query_row(
                "SELECT epg_channel_id FROM channels WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(epg("Northwind One HD").as_deref(), Some("NW1.uk"));
        assert_eq!(epg("Meridian 24").as_deref(), Some("meridian24.fr"));
        assert_eq!(epg("Radio Uno"), None);

        // A refresh with nothing changed keeps the match rather than falling
        // back to the raw tvg-id until the guide phase runs again.
        write(&c, FIXTURE).unwrap();
        assert_eq!(epg("Northwind One HD").as_deref(), Some("NW1.uk"));
        assert_eq!(epg("Meridian 24").as_deref(), Some("meridian24.fr"));
    }

    #[test]
    fn with_no_guide_known_matching_changes_nothing() {
        let c = open_in_memory().unwrap();
        write(&c, FIXTURE).unwrap();
        let (matched, total) = match_epg(&c).unwrap();
        assert_eq!((matched, total), (0, 8));
        let id: Option<String> = c
            .query_row(
                "SELECT epg_channel_id FROM channels WHERE name = 'Northwind One HD'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(id.as_deref(), Some("nw1.uk"));
    }
}
