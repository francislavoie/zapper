//! A tolerant, streaming M3U parser.
//!
//! "Extended M3U" has no specification anyone follows. What arrives in
//! practice is a header line, then for each channel an `#EXTINF` line of
//! `key="value"` attributes and a display name, any number of directive lines
//! (`#EXTVLCOPT`, `#EXTGRP`, `#KODIPROP`, `#EXTHTTP`), and finally the URL.
//! Real playlists break every one of those rules somewhere in 100,000 lines:
//! commas inside names, commas inside quoted attributes, single quotes,
//! unquoted values, a UTF-8 byte-order mark, CRLF endings, Latin-1 bytes in a
//! file that is otherwise UTF-8, directives before the `#EXTINF` they belong
//! to rather than after, URLs with no `#EXTINF` at all.
//!
//! So the parser is a line-at-a-time state machine that never fails: a line
//! it cannot make sense of is skipped, and the entry around it still lands.
//! A single strict rule here would take down the whole catalogue, which is the
//! same lesson the Xtream wire models learned.
//!
//! Line-at-a-time is also what keeps memory flat: a 60 MB playlist is read
//! through a `BufRead` and each entry handed to the caller as soon as its URL
//! arrives. Nothing accumulates but the header.
//!
//! `group-title` sometimes carries several groups separated by `;`. A channel
//! has one category in this catalogue (the schema's `category_id` is a single
//! column, and the rail draws a tree, not tags), so the first is used and the
//! rest are dropped. The common case for `;` is a provider tagging a channel
//! with both its genre and its country; either alone is a reasonable home.

use std::io::{self, BufRead};

/// What the `#EXTM3U` line says about the whole playlist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// `x-tvg-url` and `url-tvg`, each of which may itself be a
    /// comma-separated list. Used when the account names no guide of its own.
    pub epg_urls: Vec<String>,
    /// Playlist-wide catch-up defaults, which an entry's own attributes
    /// override.
    pub catchup: Option<String>,
    pub catchup_source: Option<String>,
    pub catchup_days: Option<u32>,
    /// True once an HLS tag (`#EXT-X-…`) has been seen. A "playlist" made of
    /// those is a single stream someone pasted in, not a channel list.
    pub hls: bool,
}

/// One channel, as the playlist describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Whether `name` was stated (the `#EXTINF` title or `tvg-name`) rather
    /// than made up from the URL. Only a stated name may identify a channel:
    /// see [`crate::m3u::ids`].
    pub named: bool,
    pub url: String,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    pub logo: Option<String>,
    /// The first of `group-title`'s `;`-separated groups, or `#EXTGRP`.
    pub group: Option<String>,
    pub chno: Option<i64>,
    /// How the channel number is written, when `chno` cannot say it: a
    /// subchannel's `5.1`, which `chno` holds as the sortable `5001`. See
    /// [`channel_number`].
    pub chno_label: Option<String>,
    /// Raw catch-up attributes, with the header's defaults already applied.
    /// Interpreted by [`crate::m3u::catchup`].
    pub catchup: Option<String>,
    pub catchup_source: Option<String>,
    pub catchup_days: Option<u32>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

impl Entry {
    /// The playlist equivalent of the server's `###### NAME ######` rows.
    /// Some playlists carry them too, pointing at a dummy URL.
    pub fn is_separator(&self) -> bool {
        let t = self.name.trim();
        t.len() >= 4 && t.starts_with("##") && t.ends_with("##")
    }
}

/// What has been said about the entry whose URL has not arrived yet.
#[derive(Default)]
struct Pending {
    info: Option<Entry>,
    group: Option<String>,
    user_agent: Option<String>,
    referrer: Option<String>,
}

#[derive(Default)]
pub struct Parser {
    header: Header,
    pending: Pending,
    first_line: bool,
}

/// Splits `key="value" key2='v2' key3=v3` off the front of `s`, stopping at
/// the first comma outside quotes. Returns the attributes (keys lowercased)
/// and whatever follows that comma, if there was one.
fn attributes(s: &str) -> (Vec<(String, String)>, Option<&str>) {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return (out, None);
        }
        if bytes[i] == b',' {
            return (out, Some(&s[i + 1..]));
        }
        let key_start = i;
        while i < bytes.len() && !matches!(bytes[i], b'=' | b',' | b' ' | b'\t') {
            i += 1;
        }
        let key = s[key_start..i].to_ascii_lowercase();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            let value = if i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
                let q = bytes[i];
                i += 1;
                let v_start = i;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                let v = &s[v_start..i];
                // Past the closing quote. An unclosed quote has already run to
                // the end of the line, which loses the name but keeps the
                // attribute: the URL line still makes it a channel.
                i = (i + 1).min(bytes.len());
                v
            } else {
                let v_start = i;
                while i < bytes.len() && !matches!(bytes[i], b',' | b' ' | b'\t') {
                    i += 1;
                }
                &s[v_start..i]
            };
            out.push((key, value.trim().to_owned()));
        } else if !key.is_empty() {
            // A bare word: some writers put stray tokens between attributes.
            out.push((key, String::new()));
        }
    }
}

fn nonempty(v: &str) -> Option<String> {
    let t = v.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// How much a minor channel number is worth beside its major one: `5.1`
/// sorts as `5001`. ATSC minors stop at 999, so no two numbers meet.
pub const MINOR_SPAN: i64 = 1000;

/// A channel number as people write it, read into something that sorts.
///
/// `101` is 101 and needs no label. `5.1` — an over-the-air subchannel, which
/// is how an HDHomeRun or a Tvheadend server numbers broadcast TV — is
/// `5 * MINOR_SPAN + 1` with `5.1` kept to show, because the `num` column
/// the listing sorts by is an integer, and `5.1`, `5.2` and `5.10` must come
/// out in that order (as decimals they would not: `5.10` is `5.1`). A dash
/// (`5-1`) is the same thing written the way some lineups write it.
/// Anything else is not a number this can use, and `None`.
pub fn channel_number(raw: &str) -> Option<(i64, Option<String>)> {
    let t = raw.trim();
    if let Ok(n) = t.parse::<i64>() {
        return Some((n, None));
    }
    let (major, minor) = t.split_once(['.', '-'])?;
    let major: i64 = major.parse().ok()?;
    let minor: i64 = minor.parse().ok()?;
    if !(0..MINOR_SPAN).contains(&minor) || major < 0 {
        return None;
    }
    Some((major * MINOR_SPAN + minor, Some(format!("{major}.{minor}"))))
}

/// The name for an entry that has none: the last path segment of its URL,
/// without query or extension, which is usually the channel's slug.
pub fn name_from_url(url: &str) -> String {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let last = no_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(no_query);
    let stem = last.rsplit_once('.').map_or(last, |(s, _)| s);
    if stem.is_empty() {
        url.to_owned()
    } else {
        stem.to_owned()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            first_line: true,
            ..Self::default()
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn into_header(self) -> Header {
        self.header
    }

    /// Feeds one line. Returns an entry when the line completed one.
    pub fn line(&mut self, raw: &str) -> Option<Entry> {
        let mut line = raw.trim_end_matches(['\r', '\n']);
        if self.first_line {
            self.first_line = false;
            line = line.trim_start_matches('\u{feff}');
        }
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        if let Some(rest) = line.strip_prefix('#') {
            self.directive(rest);
            return None;
        }
        // A line with no `#EXTINF` before it is only a channel if it looks
        // like an address. Otherwise an HTML login page served with a 200 in
        // place of the playlist would arrive as a catalogue of markup.
        if self.pending.info.is_none() && !line.contains("://") {
            return None;
        }
        Some(self.finish(line))
    }

    fn directive(&mut self, rest: &str) {
        let (tag, body) = match rest.split_once(':') {
            Some((t, b)) => (t, b),
            None => (rest, ""),
        };
        // Tags are matched case-insensitively; `#extinf` exists in the wild.
        match tag.trim().to_ascii_uppercase().as_str() {
            "EXTM3U" => self.header_line(body),
            // `#EXTM3U x-tvg-url=…` with no colon arrives as the tag itself.
            t if t.starts_with("EXTM3U ") => self.header_line(&rest[6..]),
            "EXTINF" => self.pending.info = Some(Self::extinf(body)),
            "EXTGRP" => self.pending.group = nonempty(body),
            "EXTVLCOPT" => self.vlcopt(body),
            "EXTHTTP" => self.exthttp(body),
            t if t.starts_with("EXT-X-") => self.header.hls = true,
            // `#KODIPROP` (inputstream and DRM properties), comments, and
            // anything else: tolerated and ignored.
            _ => {}
        }
    }

    fn header_line(&mut self, body: &str) {
        let (attrs, _) = attributes(body);
        for (k, v) in attrs {
            match k.as_str() {
                "x-tvg-url" | "url-tvg" | "tvg-url" => {
                    for u in v.split(',') {
                        if let Some(u) = nonempty(u) {
                            if !self.header.epg_urls.contains(&u) {
                                self.header.epg_urls.push(u);
                            }
                        }
                    }
                }
                "catchup" | "catchup-type" => self.header.catchup = nonempty(&v),
                "catchup-source" => self.header.catchup_source = nonempty(&v),
                "catchup-days" => self.header.catchup_days = v.trim().parse().ok(),
                _ => {}
            }
        }
    }

    fn extinf(body: &str) -> Entry {
        // The duration comes first and is not an attribute: `-1`, `0`, `-1.0`.
        let body = body.trim_start();
        let after_duration = body
            .find(|c: char| c.is_whitespace() || c == ',')
            .map_or("", |i| &body[i..]);
        let (attrs, name) = attributes(after_duration);
        let mut e = Entry {
            name: name.map(str::trim).unwrap_or_default().to_owned(),
            ..Entry::default()
        };
        for (k, v) in attrs {
            match k.as_str() {
                "tvg-id" => e.tvg_id = nonempty(&v),
                "tvg-name" => e.tvg_name = nonempty(&v),
                "tvg-logo" | "logo" => e.logo = nonempty(&v),
                "group-title" => {
                    e.group = v.split(';').find_map(nonempty);
                }
                "tvg-chno" | "channel-number" | "tvg-num" => {
                    if let Some((n, label)) = channel_number(&v) {
                        e.chno = Some(n);
                        e.chno_label = label;
                    }
                }
                "catchup" | "catchup-type" => e.catchup = nonempty(&v),
                "catchup-source" => e.catchup_source = nonempty(&v),
                "catchup-days" | "timeshift" | "tvg-rec" => {
                    if let Ok(d) = v.trim().parse() {
                        e.catchup_days = Some(d);
                    }
                }
                "http-user-agent" | "user-agent" => e.user_agent = nonempty(&v),
                "http-referrer" | "http-referer" | "referrer" | "referer" => {
                    e.referrer = nonempty(&v)
                }
                _ => {}
            }
        }
        e
    }

    fn vlcopt(&mut self, body: &str) {
        let Some((k, v)) = body.split_once('=') else {
            return;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "http-user-agent" => self.pending.user_agent = nonempty(v),
            "http-referrer" | "http-referer" => self.pending.referrer = nonempty(v),
            _ => {}
        }
    }

    /// TiviMate's `#EXTHTTP:{"User-Agent":"…","Referer":"…"}`.
    fn exthttp(&mut self, body: &str) {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str(body) else {
            return;
        };
        for (k, v) in map {
            let Some(v) = v.as_str() else { continue };
            match k.to_ascii_lowercase().as_str() {
                "user-agent" => self.pending.user_agent = nonempty(v),
                "referer" | "referrer" => self.pending.referrer = nonempty(v),
                _ => {}
            }
        }
    }

    /// A URL line: whatever was pending becomes an entry.
    fn finish(&mut self, url: &str) -> Entry {
        let p = std::mem::take(&mut self.pending);
        let mut e = p.info.unwrap_or_default();
        e.url = url.to_owned();
        if e.group.is_none() {
            e.group = p.group;
        }
        // A directive line is more specific than an attribute on #EXTINF:
        // it exists only to say this.
        if p.user_agent.is_some() {
            e.user_agent = p.user_agent;
        }
        if p.referrer.is_some() {
            e.referrer = p.referrer;
        }
        if e.name.is_empty() {
            e.name = e.tvg_name.clone().unwrap_or_default();
        }
        e.named = !e.name.is_empty();
        if !e.named {
            e.name = name_from_url(url);
        }
        if e.catchup.is_none() {
            e.catchup = self.header.catchup.clone();
        }
        if e.catchup_source.is_none() {
            e.catchup_source = self.header.catchup_source.clone();
        }
        if e.catchup_days.is_none() {
            e.catchup_days = self.header.catchup_days;
        }
        e
    }
}

/// Decodes one line of bytes. UTF-8 when it is; otherwise Latin-1, which
/// cannot fail and is what an older playlist that is not UTF-8 nearly always
/// is. Per line, because a file that is mostly UTF-8 with one pasted-in
/// Latin-1 name should lose nothing but that name's accents.
pub fn decode_line(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(s) => std::borrow::Cow::Borrowed(s),
        Err(_) => std::borrow::Cow::Owned(bytes.iter().map(|&b| b as char).collect()),
    }
}

/// Reads a whole playlist, handing each entry to `on_entry` as it completes.
///
/// `on_entry` returning an error stops the read and returns it, so a database
/// write that fails ends the parse instead of being retried 100,000 times.
pub fn parse<R: BufRead>(
    mut src: R,
    mut on_entry: impl FnMut(Entry) -> anyhow::Result<()>,
) -> anyhow::Result<Header> {
    let mut parser = Parser::new();
    let mut buf = Vec::with_capacity(512);
    loop {
        buf.clear();
        let n = src.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        if let Some(e) = parser.line(&decode_line(&buf)) {
            on_entry(e)?;
        }
    }
    Ok(parser.into_header())
}

/// Reads at most `max_lines` lines and counts the entries in them. For the
/// connection test, which wants to know the playlist parses without paying
/// for all 60 MB of it.
pub fn sample<R: BufRead>(mut src: R, max_lines: usize) -> io::Result<(usize, Header, bool)> {
    let mut parser = Parser::new();
    let mut buf = Vec::with_capacity(512);
    let mut count = 0;
    for _ in 0..max_lines {
        buf.clear();
        match src.read_until(b'\n', &mut buf) {
            Ok(0) => return Ok((count, parser.into_header(), false)),
            Ok(_) => {}
            // A partial download ends mid-stream; what was read still counts.
            Err(e) if count > 0 => {
                tracing::debug!("playlist sample ended early: {e}");
                return Ok((count, parser.into_header(), true));
            }
            Err(e) => return Err(e),
        }
        if parser.line(&decode_line(&buf)).is_some() {
            count += 1;
        }
    }
    // More to read means the count is a lower bound, and the test says so.
    let truncated = src.fill_buf().map(|b| !b.is_empty()).unwrap_or(false);
    Ok((count, parser.into_header(), truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(text: &str) -> (Vec<Entry>, Header) {
        let mut v = Vec::new();
        let h = parse(text.as_bytes(), |e| {
            v.push(e);
            Ok(())
        })
        .unwrap();
        (v, h)
    }

    const FIXTURE: &str = include_str!("../../tests/fixtures/sample.m3u");

    #[test]
    fn the_fixture_parses_every_entry() {
        let (v, h) = all(FIXTURE);
        assert_eq!(v.len(), 8, "{v:#?}");
        assert_eq!(
            h.epg_urls,
            [
                "http://epg.example/guide.xml.gz",
                "http://epg2.example/more.xml"
            ]
        );
        assert_eq!(h.catchup_days, Some(3));
    }

    #[test]
    fn attributes_are_read_whatever_their_quoting() {
        let (v, _) = all(
            "#EXTM3U\n#EXTINF:-1 tvg-id=\"nwn1.uk\" tvg-name='NWN One' tvg-logo=http://l/x.png \
             group-title=\"UK;Entertainment\" tvg-chno=\"101\",NWN One HD\nhttp://s/1.ts\n",
        );
        let e = &v[0];
        assert_eq!(e.tvg_id.as_deref(), Some("nwn1.uk"));
        assert_eq!(e.tvg_name.as_deref(), Some("NWN One"));
        assert_eq!(e.logo.as_deref(), Some("http://l/x.png"));
        // `;`-separated groups: the first is the category.
        assert_eq!(e.group.as_deref(), Some("UK"));
        assert_eq!(e.chno, Some(101));
        assert_eq!(e.name, "NWN One HD");
        assert_eq!(e.url, "http://s/1.ts");
    }

    #[test]
    fn subchannel_numbers_sort_as_integers_and_keep_their_spelling() {
        assert_eq!(channel_number("101"), Some((101, None)));
        assert_eq!(channel_number(" 5.1 "), Some((5001, Some("5.1".into()))));
        assert_eq!(channel_number("5-10"), Some((5010, Some("5.10".into()))));
        // 5.2 before 5.10, which decimals would get wrong.
        assert!(channel_number("5.2").unwrap().0 < channel_number("5.10").unwrap().0);
        assert!(channel_number("5.99").unwrap().0 < channel_number("6.1").unwrap().0);
        for bad in ["", "five", "5.1.2", "5.", "5.1000", "-3.1"] {
            assert_eq!(channel_number(bad), None, "{bad:?}");
        }
        let (v, _) = all("#EXTM3U\n#EXTINF:-1 tvg-chno=\"7.2\",KXMO D2\nhttp://s/7.ts\n");
        assert_eq!((v[0].chno, v[0].chno_label.as_deref()), (Some(7002), Some("7.2")));
    }

    #[test]
    fn commas_in_names_and_in_quoted_attributes_survive() {
        let (v, _) = all(
            "#EXTM3U\n#EXTINF:-1 group-title=\"News, Weather\",Summit News, Live, 24/7\nhttp://s/2\n",
        );
        assert_eq!(v[0].group.as_deref(), Some("News, Weather"));
        assert_eq!(v[0].name, "Summit News, Live, 24/7");
    }

    #[test]
    fn a_bom_and_crlf_endings_are_invisible() {
        let (v, h) =
            all("\u{feff}#EXTM3U x-tvg-url=\"http://g/x.xml\"\r\n#EXTINF:-1,One\r\nhttp://s/1\r\n");
        assert_eq!(h.epg_urls, ["http://g/x.xml"]);
        assert_eq!(v[0].name, "One");
        assert_eq!(v[0].url, "http://s/1");
    }

    #[test]
    fn latin1_bytes_decode_rather_than_fail() {
        let mut bytes = b"#EXTM3U\n#EXTINF:-1,T\xe9l\xe9 Qu\xe9bec\nhttp://s/1\n".to_vec();
        bytes.extend_from_slice("#EXTINF:-1,Télé UTF\nhttp://s/2\n".as_bytes());
        let mut v = Vec::new();
        parse(&bytes[..], |e| {
            v.push(e);
            Ok(())
        })
        .unwrap();
        assert_eq!(v[0].name, "Télé Québec");
        assert_eq!(v[1].name, "Télé UTF");
    }

    #[test]
    fn missing_attributes_and_names_fall_back_sensibly() {
        let (v, _) = all(
            "#EXTM3U\n#EXTINF:-1 tvg-name=\"Only Tvg Name\",\nhttp://s/a\n\
             #EXTINF:-1\nhttp://s/path/arte.m3u8?token=1\nhttp://bare.example/live/raw.ts\n",
        );
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "Only Tvg Name");
        assert_eq!(v[1].name, "arte");
        assert_eq!(v[1].tvg_id, None);
        assert_eq!(v[1].group, None);
        // A URL with no #EXTINF is still a channel.
        assert_eq!(v[2].name, "raw");
    }

    #[test]
    fn vlcopt_and_exthttp_set_per_channel_headers() {
        let (v, _) = all(
            "#EXTM3U\n#EXTINF:-1,A\n#EXTVLCOPT:http-user-agent=Mozilla/5.0 (X11)\n\
             #EXTVLCOPT:http-referrer=https://site.example/\nhttp://s/a\n\
             #EXTINF:-1,B\n#EXTHTTP:{\"User-Agent\":\"Tivi\",\"Referer\":\"https://r/\"}\nhttp://s/b\n\
             #EXTINF:-1,C\nhttp://s/c\n",
        );
        assert_eq!(v[0].user_agent.as_deref(), Some("Mozilla/5.0 (X11)"));
        assert_eq!(v[0].referrer.as_deref(), Some("https://site.example/"));
        assert_eq!(v[1].user_agent.as_deref(), Some("Tivi"));
        assert_eq!(v[1].referrer.as_deref(), Some("https://r/"));
        // Headers belong to one entry, never the next.
        assert_eq!(v[2].user_agent, None);
        assert_eq!(v[2].referrer, None);
    }

    #[test]
    fn extgrp_fills_in_a_missing_group_but_does_not_override_one() {
        let (v, _) = all("#EXTM3U\n#EXTINF:-1,A\n#EXTGRP:Movies\nhttp://s/a\n\
             #EXTGRP:Ignored\n#EXTINF:-1 group-title=\"Kids\",B\nhttp://s/b\n");
        assert_eq!(v[0].group.as_deref(), Some("Movies"));
        assert_eq!(v[1].group.as_deref(), Some("Kids"));
    }

    #[test]
    fn kodiprop_and_unknown_directives_are_tolerated() {
        let (v, _) = all(
            "#EXTM3U\n#EXTINF:-1,A\n#KODIPROP:inputstream.adaptive.license_type=x\n\
             # a comment\n#EXTSIZE:9\nhttp://s/a\n",
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "A");
    }

    #[test]
    fn header_epg_urls_split_on_commas_and_merge_both_spellings() {
        let (_, h) =
            all("#EXTM3U url-tvg=\"http://a/1.xml,http://a/2.xml\" x-tvg-url=\"http://a/1.xml\"\n");
        assert_eq!(h.epg_urls, ["http://a/1.xml", "http://a/2.xml"]);
    }

    #[test]
    fn header_catchup_defaults_apply_unless_an_entry_overrides_them() {
        let (v, _) = all(
            "#EXTM3U catchup=\"shift\" catchup-days=\"2\"\n#EXTINF:-1,A\nhttp://s/a\n\
             #EXTINF:-1 catchup=\"append\" catchup-source=\"?t={utc}\" catchup-days=\"7\",B\nhttp://s/b\n",
        );
        assert_eq!(v[0].catchup.as_deref(), Some("shift"));
        assert_eq!(v[0].catchup_days, Some(2));
        assert_eq!(v[1].catchup.as_deref(), Some("append"));
        assert_eq!(v[1].catchup_source.as_deref(), Some("?t={utc}"));
        assert_eq!(v[1].catchup_days, Some(7));
    }

    #[test]
    fn an_hls_media_playlist_is_flagged_rather_than_read_as_channels() {
        let (v, h) =
            all("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg1.ts\n");
        assert!(h.hls);
        // The segment is still reported; the caller decides what HLS means.
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn separators_are_recognised() {
        let (v, _) = all("#EXTM3U\n#EXTINF:-1,##### SPORTS #####\nhttp://dummy\n");
        assert!(v[0].is_separator());
    }

    #[test]
    fn sampling_stops_at_the_line_limit() {
        let mut text = String::from("#EXTM3U\n");
        for i in 0..1000 {
            text.push_str(&format!("#EXTINF:-1,C{i}\nhttp://s/{i}\n"));
        }
        let (n, _, truncated) = sample(text.as_bytes(), 201).unwrap();
        assert_eq!(n, 100);
        assert!(truncated);
        let (n, _, truncated) = sample(text.as_bytes(), 10_000).unwrap();
        assert_eq!(n, 1000);
        assert!(!truncated);
    }
}
