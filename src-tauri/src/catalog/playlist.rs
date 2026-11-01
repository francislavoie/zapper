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
use crate::catalog::sync::{clean_name, SyncStats};
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
        Ok(Self {
            tx,
            existing,
            seen: HashSet::new(),
            occurrences: HashMap::new(),
            groups: Vec::new(),
            group_set: HashSet::new(),
            position: 0,
            stats: PlaylistStats::default(),
        })
    }

    pub fn push(&mut self, e: Entry) -> Result<()> {
        // See `m3u::ids`: the id comes from what the entry is called, not
        // from its URL, which may carry a token that changes every download.
        let identity = ids::identity(&e);
        self.push_identified(e, &identity)
    }

    /// [`push`](Self::push), for an entry whose source knows better than a
    /// display name what makes it the same channel next time: an HDHomeRun's
    /// device and guide number, a Tvheadend channel's UUID. Those survive a
    /// rename and a retag, which a name and a group do not. The identity is
    /// hashed into the same id range as a playlist's, so the mirror cannot
    /// tell — and does not need to — which kind of source wrote a row.
    pub fn push_identified(&mut self, e: Entry, identity: &str) -> Result<()> {
        self.position += 1;
        let occurrence = {
            let n = self
                .occurrences
                .entry(ids::fnv1a(identity.as_bytes()))
                .or_insert(0);
            let this = *n;
            *n += 1;
            this
        };
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

    /// Commits the write. Refuses an empty playlist outright: an empty answer
    /// is far more likely to be a provider having a bad minute than a
    /// subscription with no channels, and accepting it would soft-delete the
    /// whole catalogue.
    pub fn finish(mut self, header: &Header) -> Result<PlaylistStats> {
        if self.seen.is_empty() {
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
