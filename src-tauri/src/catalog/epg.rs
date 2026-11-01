//! Streaming XMLTV parser.
//!
//! A guide can run to tens of megabytes. It is parsed event-by-event
//! straight into SQLite;
//! nothing is accumulated in memory.

use anyhow::{anyhow, Result};
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};
use rusqlite::{params, Connection};
use std::io::BufRead;

// quick-xml 0.42 hands back `&str` for names and attribute keys, and it emits
// `&amp;`-style references as their own `GeneralRef` events rather than
// unescaping them into the surrounding text. Text is therefore accumulated
// across events and closed out at the element's end tag.
const XML: XmlVersion = XmlVersion::Implicit1_0;

/// XMLTV time is `YYYYMMDDHHMMSS ±HHMM`.
pub fn parse_xmltv_time(s: &str) -> Result<i64> {
    let s = s.trim();
    let (stamp, offset) = s.split_at(14.min(s.len()));
    if stamp.len() != 14 {
        return Err(anyhow!("bad xmltv timestamp: {s}"));
    }
    let num = |a: usize, b: usize| -> Result<i64> {
        stamp[a..b].parse::<i64>().map_err(|e| anyhow!("{e} in {s}"))
    };
    let (y, mo, d) = (num(0, 4)?, num(4, 6)?, num(6, 8)?);
    let (h, mi, sec) = (num(8, 10)?, num(10, 12)?, num(12, 14)?);

    // Days since the Unix epoch, via the civil-from-days algorithm.
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let mut t = days * 86_400 + h * 3600 + mi * 60 + sec;

    let off = offset.trim();
    if off.len() == 5 {
        let sign = if off.starts_with('-') { 1 } else { -1 };
        let oh: i64 = off[1..3].parse().unwrap_or(0);
        let om: i64 = off[3..5].parse().unwrap_or(0);
        t += sign * (oh * 3600 + om * 60);
    }
    Ok(t)
}

/// What to do with a guide's `<channel>` elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelNames {
    /// Skip them. An Xtream server's channels already say which guide channel
    /// they are, so there is nothing to match and nothing to store.
    Ignore,
    /// Record each `<display-name>` in `epg_channel_names`, for matching
    /// playlist entries by name. `replace` clears what an earlier pass
    /// stored first — inside this parse's transaction, so a guide that fails
    /// halfway leaves the previous names in place rather than none.
    Collect { replace: bool },
}

pub fn parse_xmltv<R: BufRead>(src: R, conn: &Connection) -> Result<usize> {
    parse_xmltv_into(src, conn, ChannelNames::Ignore)
}

pub fn parse_xmltv_into<R: BufRead>(
    src: R,
    conn: &Connection,
    names: ChannelNames,
) -> Result<usize> {
    // Text is deliberately not trimmed by the reader: an entity splits a
    // title into several text events, and trimming each one would weld
    // "Court &amp; Council" into "Court&Council". Fields are trimmed once, whole.
    let mut reader = Reader::from_reader(src);

    let tx = conn.unchecked_transaction()?;
    let collect = matches!(names, ChannelNames::Collect { .. });
    if names == (ChannelNames::Collect { replace: true }) {
        tx.execute("DELETE FROM epg_channel_names", [])?;
    }
    let mut name_stmt = if collect {
        Some(tx.prepare(
            "INSERT OR IGNORE INTO epg_channel_names (channel_id, name) VALUES (?1, ?2)",
        )?)
    } else {
        None
    };
    // The `<channel id>` whose display names are being read, if any.
    let mut in_channel: Option<String> = None;
    let mut stmt = tx.prepare(
        "INSERT INTO epg (channel_ref, start, stop, title, description)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(channel_ref, start) DO UPDATE SET
           stop=excluded.stop, title=excluded.title, description=excluded.description",
    )?;

    let mut buf = Vec::new();
    let mut count = 0usize;
    let (mut chan, mut start, mut stop) = (String::new(), 0i64, 0i64);
    let (mut title, mut desc) = (String::new(), None::<String>);
    let mut field: Option<&'static str> = None;
    let mut acc = String::new();
    let mut in_programme = false;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) if collect && e.name().as_ref() == "channel" => {
                in_channel = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key.as_ref() == "id")
                    .map(|a| a.normalized_value(XML).map(|v| v.into_owned()))
                    .transpose()?;
            }
            Event::Start(e) if in_channel.is_some() && e.name().as_ref() == "display-name" => {
                field = Some("display-name");
                acc.clear();
            }
            Event::Start(e) if e.name().as_ref() == "programme" => {
                in_programme = true;
                title.clear();
                desc = None;
                for a in e.attributes().flatten() {
                    let v = a.normalized_value(XML)?.into_owned();
                    match a.key.as_ref() {
                        "channel" => chan = v,
                        "start" => start = parse_xmltv_time(&v).unwrap_or(0),
                        "stop" => stop = parse_xmltv_time(&v).unwrap_or(0),
                        _ => {}
                    }
                }
            }
            Event::Start(e) if in_programme => {
                field = match e.name().as_ref() {
                    "title" => Some("title"),
                    "desc" => Some("desc"),
                    _ => None,
                };
                acc.clear();
            }
            Event::Text(t) if field.is_some() => acc.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if field.is_some() => {
                // `&amp;` and friends arrive as their own event; a title like
                // "Court & Council" would otherwise be silently truncated.
                if let Some(c) = r.resolve_char_ref()? {
                    acc.push(c);
                } else if let Some(s) = resolve_predefined_entity(&r.xml10_content()) {
                    acc.push_str(s);
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "programme" => {
                    if !chan.is_empty() && start > 0 {
                        stmt.execute(params![chan, start, stop, title, desc])?;
                        count += 1;
                    }
                    in_programme = false;
                    field = None;
                }
                "title" if field == Some("title") => {
                    title = acc.trim().to_owned();
                    acc.clear();
                    field = None;
                }
                "desc" if field == Some("desc") => {
                    let d = acc.trim().to_owned();
                    acc.clear();
                    desc = if d.is_empty() { None } else { Some(d) };
                    field = None;
                }
                "display-name" if field == Some("display-name") => {
                    let n = acc.trim();
                    if let (Some(id), Some(st)) = (&in_channel, name_stmt.as_mut()) {
                        if !n.is_empty() {
                            st.execute(params![id, n])?;
                        }
                    }
                    acc.clear();
                    field = None;
                }
                "channel" if collect => {
                    in_channel = None;
                    field = None;
                }
                _ => field = None,
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    drop(stmt);
    drop(name_stmt);
    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::schema::open_in_memory;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<tv>
  <programme start="20260908010000 +0000" stop="20260908020000 +0000" channel="nwn1.uk">
    <title>Breakfast</title>
    <desc>Morning news.</desc>
  </programme>
  <programme start="20260908020000 +0000" stop="20260908030000 +0000" channel="nwn1.uk">
    <title>Bargain Hunt</title>
  </programme>
</tv>"#;

    #[test]
    fn parses_programmes_into_sqlite() {
        let c = open_in_memory().unwrap();
        let n = parse_xmltv(SAMPLE.as_bytes(), &c).unwrap();
        assert_eq!(n, 2);
        let t: String = c
            .query_row(
                "SELECT title FROM epg WHERE channel_ref='nwn1.uk' ORDER BY start LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(t, "Breakfast");
    }

    #[test]
    fn missing_description_is_allowed() {
        let c = open_in_memory().unwrap();
        parse_xmltv(SAMPLE.as_bytes(), &c).unwrap();
        let d: Option<String> = c
            .query_row(
                "SELECT description FROM epg WHERE title='Bargain Hunt'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(d, None);
    }

    #[test]
    fn reparsing_replaces_rather_than_duplicating() {
        let c = open_in_memory().unwrap();
        parse_xmltv(SAMPLE.as_bytes(), &c).unwrap();
        parse_xmltv(SAMPLE.as_bytes(), &c).unwrap();
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM epg", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn entities_in_titles_survive() {
        // quick-xml reports `&amp;` as a separate event, so a title split
        // around one must be reassembled rather than truncated.
        let xml = r#"<tv><programme start="20260908010000 +0000" stop="20260908020000 +0000" channel="c">
            <title>Bread &amp; Butter</title></programme></tv>"#;
        let c = open_in_memory().unwrap();
        parse_xmltv(xml.as_bytes(), &c).unwrap();
        let t: String = c
            .query_row("SELECT title FROM epg", [], |r| r.get(0))
            .unwrap();
        assert_eq!(t, "Bread & Butter");
    }

    /// Parses a real slice of a provider's guide. Fixtures cannot cover what
    /// a full-size real XMLTV file contains - entity refs mid-title, odd
    /// encodings, missing stops. Ignored by default; point it at a saved sample:
    ///
    ///   ZAPPER_EPG_FIXTURE=/path/to/epg_sample.xml \
    ///     cargo test --lib real_epg_sample -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_epg_sample_parses() {
        let Ok(path) = std::env::var("ZAPPER_EPG_FIXTURE") else {
            panic!("set ZAPPER_EPG_FIXTURE to a saved xmltv sample");
        };
        let f = std::fs::File::open(&path).expect("opening fixture");
        let c = open_in_memory().unwrap();
        let n = parse_xmltv(std::io::BufReader::new(f), &c).unwrap();
        println!("parsed {n} programmes from {path}");
        assert!(n > 1000, "expected a substantial sample, got {n}");

        let empty: i64 = c
            .query_row("SELECT COUNT(*) FROM epg WHERE title = ''", [], |r| r.get(0))
            .unwrap();
        assert_eq!(empty, 0, "{empty} programmes parsed with an empty title");

        // An entity ref that ended the title early would turn
        // "Salt &amp; Stone" into "Salt". Titles legitimately *contain*
        // " & ", so truncation means ending on one, not holding one.
        let truncated: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM epg WHERE trim(title) LIKE '%&'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(truncated, 0, "{truncated} titles end at an entity ref");

        // Round-trip two real titles: &amp; mid-title and &apos; mid-word.
        for expected in ["Cats & Dogs", "Gosh! It's the Wilsons"] {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM epg WHERE title = ?1",
                    [expected],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(n > 0, "entity-bearing title did not round-trip: {expected:?}");
        }

        let sample: Vec<String> = c
            .prepare("SELECT title FROM epg WHERE title LIKE '%&%' LIMIT 5")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        println!("titles containing '&': {sample:?}");
    }

    const WITH_CHANNELS: &str = r#"<tv>
  <channel id="nwn1.uk"><display-name lang="en">NWN One</display-name>
    <display-name>NWN 1 London</display-name><icon src="x"/></channel>
  <channel id="r24.fr"><display-name>Riviera 24 &amp; Co</display-name></channel>
  <programme start="20260908010000 +0000" stop="20260908020000 +0000" channel="nwn1.uk">
    <title>Breakfast</title>
  </programme>
</tv>"#;

    fn names(c: &Connection) -> Vec<(String, String)> {
        c.prepare("SELECT channel_id, name FROM epg_channel_names ORDER BY 1, 2")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn display_names_are_collected_when_asked() {
        let c = open_in_memory().unwrap();
        let n = parse_xmltv_into(
            WITH_CHANNELS.as_bytes(),
            &c,
            ChannelNames::Collect { replace: true },
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            names(&c),
            [
                ("nwn1.uk".to_owned(), "NWN 1 London".to_owned()),
                ("nwn1.uk".to_owned(), "NWN One".to_owned()),
                ("r24.fr".to_owned(), "Riviera 24 & Co".to_owned()),
            ]
        );
    }

    #[test]
    fn a_panels_guide_stores_no_names() {
        // The Xtream path is unchanged: channels are read past, not stored.
        let c = open_in_memory().unwrap();
        assert_eq!(parse_xmltv(WITH_CHANNELS.as_bytes(), &c).unwrap(), 1);
        assert!(names(&c).is_empty());
    }

    #[test]
    fn replacing_clears_the_previous_guides_names_and_adding_does_not() {
        let c = open_in_memory().unwrap();
        c.execute("INSERT INTO epg_channel_names VALUES ('old', 'Old Name')", [])
            .unwrap();
        let keep = ChannelNames::Collect { replace: false };
        parse_xmltv_into(WITH_CHANNELS.as_bytes(), &c, keep).unwrap();
        assert!(names(&c).iter().any(|(id, _)| id == "old"));
        let replace = ChannelNames::Collect { replace: true };
        parse_xmltv_into(WITH_CHANNELS.as_bytes(), &c, replace).unwrap();
        assert!(!names(&c).iter().any(|(id, _)| id == "old"));
    }

    #[test]
    fn xmltv_timestamps_become_unix_seconds() {
        // Ground truth: `date -u -d '2026-09-08 01:00:00' +%s`.
        assert_eq!(parse_xmltv_time("20260908010000 +0000").unwrap(), 1_788_829_200);
    }

    #[test]
    fn utc_offsets_are_applied() {
        // 01:00 at +0200 is 23:00 the previous day in UTC.
        assert_eq!(
            parse_xmltv_time("20260908010000 +0200").unwrap(),
            1_788_829_200 - 7200
        );
        assert_eq!(
            parse_xmltv_time("20260908010000 -0500").unwrap(),
            1_788_829_200 + 18000
        );
    }
}
