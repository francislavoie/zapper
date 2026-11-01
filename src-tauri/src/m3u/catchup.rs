//! Catch-up for playlist channels: which entries have it, and the URL that
//! replays a given moment.
//!
//! An Xtream server has one way to do this. Playlists have several, named by
//! the entry's `catchup` attribute, and the conventions here follow Kodi's
//! IPTV Simple client because that is what playlist authors test against:
//!
//! | `catchup`                 | URL for a past programme                                   |
//! |---------------------------|------------------------------------------------------------|
//! | `default`                 | `catchup-source` with placeholders filled in. A relative source is appended instead. |
//! | `append`                  | the live URL + `catchup-source`, placeholders filled in     |
//! | `shift`, `timeshift`      | the live URL + `?utc={utc}&lutc={lutc}` (or `&…`)          |
//! | `flussonic`, `-hls`, `-ts`, `fs` | Flussonic's path forms: `…/index-{utc}-{duration}.m3u8`, `…/timeshift_abs-{utc}.ts` |
//! | `xc`                      | Xtream's `/timeshift/user/pass/{minutes}/{YYYY-MM-DD:HH-MM}/id.ts`, when the live URL has the Xtream shape |
//!
//! Anything else (`vod`, unknown words, `default` or `append` with no
//! source, `xc` on a URL that is not an Xtream one) means the channel has no
//! catch-up here: it is stored with `has_archive = 0`, which is the same
//! state as an Xtream channel without an archive, and the interface already
//! offers nothing in that case. An entry with a `catchup-source` but no
//! `catchup` is treated as `default`.
//!
//! **Placeholders**, all optional, unknown ones left verbatim:
//!
//! | placeholder                          | value                                   |
//! |--------------------------------------|-----------------------------------------|
//! | `{utc}` `{start}` `${start}`         | programme start, Unix seconds           |
//! | `{utcend}` `{end}` `${end}`          | start + duration, Unix seconds          |
//! | `{lutc}` `{now}` `{timestamp}` `${now}` `${timestamp}` | now, Unix seconds     |
//! | `{duration}` `${duration}`           | duration in seconds                     |
//! | `{duration:N}`                       | duration divided by N (`{duration:60}` = minutes) |
//! | `{offset}` `{offset:N}`              | seconds since the start, optionally divided by N |
//! | `{Y}` `{m}` `{d}` `{H}` `{M}` `{S}`  | start, broken down in **local** time, zero-padded |
//! | `{utc:FMT}` `{start:FMT}` `{utcend:FMT}` `{end:FMT}` `{lutc:FMT}` `{now:FMT}` | that instant in **UTC**, with `Y m d H M S` in FMT replaced by its fields (`{utc:Y-m-dTH:M:S}`) |
//!
//! Local time for the bare fields is IPTV Simple's choice and the one
//! templates are written against: an archive server that wants wall-clock
//! paths wants the wall clock of the people watching it. The `:FMT` forms
//! say `utc` in their name and mean it.

/// How an entry's archive is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Default,
    Append,
    Shift,
    Flussonic,
    Xc,
}

impl Mode {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Default => "default",
            Mode::Append => "append",
            Mode::Shift => "shift",
            Mode::Flussonic => "flussonic",
            Mode::Xc => "xc",
        }
    }

    /// Reads the stored spelling, or any of the playlist's aliases.
    pub fn parse(raw: &str) -> Option<Mode> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Mode::Default),
            "append" => Some(Mode::Append),
            "shift" | "timeshift" => Some(Mode::Shift),
            "flussonic" | "flussonic-hls" | "flussonic-ts" | "fs" => Some(Mode::Flussonic),
            "xc" => Some(Mode::Xc),
            _ => None,
        }
    }
}

/// Decides whether an entry has usable catch-up, and how.
///
/// Called once at sync, so that `has_archive` is only ever true for a channel
/// whose replay URL can actually be built — an archive badge on a channel
/// that then cannot replay anything is worse than no badge.
pub fn resolve(mode: Option<&str>, source: Option<&str>, live_url: &str) -> Option<Mode> {
    let source = source.map(str::trim).filter(|s| !s.is_empty());
    let mode = match mode {
        Some(m) => Mode::parse(m)?,
        None if source.is_some() => Mode::Default,
        None => return None,
    };
    let usable = match mode {
        Mode::Default | Mode::Append => source.is_some(),
        Mode::Shift => true,
        Mode::Flussonic => flussonic_template(live_url).is_some(),
        Mode::Xc => xc_template(live_url).is_some(),
    };
    usable.then_some(mode)
}

/// The moment being replayed, and the clock it is being replayed against.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    /// Programme start, Unix seconds.
    pub start: i64,
    pub duration_secs: i64,
    /// Unix seconds.
    pub now: i64,
    /// Seconds east of UTC at `start`, for the local-time fields.
    pub local_offset: i64,
}

/// The URL that replays `w` on a channel whose live URL is `live_url`.
/// `None` when the combination cannot produce one (see [`resolve`]).
pub fn url(mode: Mode, source: Option<&str>, live_url: &str, w: &Window) -> Option<String> {
    let source = source.map(str::trim).filter(|s| !s.is_empty());
    let template = match mode {
        Mode::Default => {
            let s = source?;
            if is_absolute(s) {
                s.to_owned()
            } else {
                format!("{live_url}{s}")
            }
        }
        Mode::Append => format!("{live_url}{}", source?),
        Mode::Shift => {
            let sep = if live_url.contains('?') { '&' } else { '?' };
            format!("{live_url}{sep}utc={{utc}}&lutc={{lutc}}")
        }
        Mode::Flussonic => flussonic_template(live_url)?,
        Mode::Xc => xc_template(live_url)?,
    };
    Some(expand(&template, w))
}

fn is_absolute(s: &str) -> bool {
    s.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    })
}

/// Splits `url` into everything before the last path segment, the segment,
/// and the query (with its `?`), so a template can replace the segment.
fn split_last_segment(url: &str) -> Option<(&str, &str, &str)> {
    let (no_query, query) = match url.find('?') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, ""),
    };
    let slash = no_query.rfind('/')?;
    // The slash must be in the path, not the `//` after the scheme.
    if no_query[..slash].ends_with('/') || !no_query[..slash].contains("://") {
        return None;
    }
    Some((&no_query[..=slash], &no_query[slash + 1..], query))
}

fn flussonic_template(live_url: &str) -> Option<String> {
    let (base, last, query) = split_last_segment(live_url)?;
    if let Some(stem) = last.strip_suffix(".m3u8") {
        return Some(format!("{base}{stem}-{{utc}}-{{duration}}.m3u8{query}"));
    }
    if last == "mpegts" || last.ends_with(".ts") {
        return Some(format!("{base}timeshift_abs-{{utc}}.ts{query}"));
    }
    None
}

/// `scheme://host[:port]/[live/]user/pass/id[.ext]` to the Xtream timeshift
/// path. The duration is in minutes and the stamp in local wall-clock time,
/// both because that is what the server reads (see the Xtream client's
/// `timeshift_url`, which this mirrors in path form).
fn xc_template(live_url: &str) -> Option<String> {
    let (scheme, rest) = live_url.split_once("://")?;
    let rest = rest.split(['?', '#']).next()?;
    let (authority, path) = rest.split_once('/')?;
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (user, pass, file) = match segs.as_slice() {
        ["live", u, p, f] => (*u, *p, *f),
        [u, p, f] => (*u, *p, *f),
        _ => return None,
    };
    let id = file.split('.').next().unwrap_or(file);
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "{scheme}://{authority}/timeshift/{user}/{pass}/{{duration:60}}/{{Y}}-{{m}}-{{d}}:{{H}}-{{M}}/{id}.ts"
    ))
}

/// Unix seconds to `(year, month, day, hour, minute, second)` in UTC.
pub fn civil(unix: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        m,
        d,
        (secs / 3600) as u32,
        ((secs % 3600) / 60) as u32,
        (secs % 60) as u32,
    )
}

fn format_fields(fmt: &str, unix: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(unix);
    let mut out = String::with_capacity(fmt.len() + 8);
    for c in fmt.chars() {
        match c {
            'Y' => out.push_str(&format!("{y:04}")),
            'm' => out.push_str(&format!("{mo:02}")),
            'd' => out.push_str(&format!("{d:02}")),
            'H' => out.push_str(&format!("{h:02}")),
            'M' => out.push_str(&format!("{mi:02}")),
            'S' => out.push_str(&format!("{s:02}")),
            other => out.push(other),
        }
    }
    out
}

fn divided(v: i64, by: &str) -> Option<String> {
    let n: i64 = by.trim().parse().ok().filter(|n| *n > 0)?;
    Some((v / n).to_string())
}

/// The value of one placeholder, without its braces (and `$`), or `None` if
/// it is not one this knows.
fn placeholder(token: &str, w: &Window) -> Option<String> {
    let end = w.start + w.duration_secs;
    let local = w.start + w.local_offset;
    let (name, arg) = match token.split_once(':') {
        Some((n, a)) => (n, Some(a)),
        None => (token, None),
    };
    let instant = |n: &str| match n {
        "utc" | "start" => Some(w.start),
        "utcend" | "end" => Some(end),
        "lutc" | "now" | "timestamp" => Some(w.now),
        _ => None,
    };
    match (name, arg) {
        (n, None) if instant(n).is_some() => instant(n).map(|t| t.to_string()),
        (n, Some(fmt)) if instant(n).is_some() => instant(n).map(|t| format_fields(fmt, t)),
        ("duration", None) => Some(w.duration_secs.to_string()),
        ("duration", Some(by)) => divided(w.duration_secs, by),
        ("offset", None) => Some((w.now - w.start).to_string()),
        ("offset", Some(by)) => divided(w.now - w.start, by),
        ("Y" | "m" | "d" | "H" | "M" | "S", None) => Some(format_fields(name, local)),
        _ => None,
    }
}

/// Fills in every placeholder [`placeholder`] knows. `{…}` and `${…}` are the
/// same thing; the `$` form is what Perfect Player and TiviMate write.
pub fn expand(template: &str, w: &Window) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|i| open + i) else {
            break;
        };
        let token = &rest[open + 1..close];
        let dollar = open > 0 && rest.as_bytes()[open - 1] == b'$';
        match placeholder(token, w) {
            Some(v) => {
                out.push_str(&rest[..if dollar { open - 1 } else { open }]);
                out.push_str(&v);
            }
            None => out.push_str(&rest[..=close]),
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-08 01:30:00 UTC, an hour long, watched at 03:00.
    fn w() -> Window {
        Window {
            start: 1_788_831_000,
            duration_secs: 3600,
            now: 1_788_836_400,
            local_offset: 0,
        }
    }

    #[test]
    fn civil_time_is_right_at_known_points() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_788_831_000), (2026, 9, 8, 1, 30, 0));
        // A leap day, a second before midnight.
        assert_eq!(civil(1_709_251_199), (2024, 2, 29, 23, 59, 59));
    }

    #[test]
    fn unix_placeholders_in_both_spellings() {
        let s = expand(
            "a={utc}&b=${start}&e={utcend}&e2=${end}&n={lutc}&d={duration}",
            &w(),
        );
        assert_eq!(
            s,
            "a=1788831000&b=1788831000&e=1788834600&e2=1788834600&n=1788836400&d=3600"
        );
        assert_eq!(expand("${timestamp}|{now}", &w()), "1788836400|1788836400");
    }

    #[test]
    fn divided_durations_and_offsets() {
        assert_eq!(
            expand("{duration:60}m {offset} {offset:60}", &w()),
            "60m 5400 90"
        );
        // A zero or nonsense divisor is not a placeholder; it stays as written.
        assert_eq!(expand("{duration:0}", &w()), "{duration:0}");
    }

    #[test]
    fn calendar_fields_are_local_and_zero_padded() {
        assert_eq!(
            expand("{Y}/{m}/{d}/{H}-{M}-{S}", &w()),
            "2026/09/08/01-30-00"
        );
        let mut east = w();
        east.local_offset = 2 * 3600;
        assert_eq!(expand("{Y}{m}{d}{H}{M}", &east), "202609080330");
    }

    #[test]
    fn formatted_instants_are_utc() {
        let mut east = w();
        east.local_offset = 2 * 3600;
        assert_eq!(expand("{utc:Y-m-dTH:M:S}", &east), "2026-09-08T01:30:00");
        assert_eq!(expand("{utcend:YmdHMS}", &east), "20260908023000");
    }

    #[test]
    fn unknown_placeholders_survive_untouched() {
        assert_eq!(
            expand("x={catchup-id}&y={utc}", &w()),
            "x={catchup-id}&y=1788831000"
        );
        assert_eq!(expand("unclosed {utc", &w()), "unclosed {utc");
    }

    #[test]
    fn default_mode_uses_an_absolute_source_and_appends_a_relative_one() {
        let abs = url(
            Mode::Default,
            Some("http://arch.example/{Y}{m}{d}/{H}.ts?d={duration}"),
            "http://live.example/c.ts",
            &w(),
        );
        assert_eq!(
            abs.as_deref(),
            Some("http://arch.example/20260908/01.ts?d=3600")
        );
        let rel = url(
            Mode::Default,
            Some("?start={utc}"),
            "http://live.example/c.ts",
            &w(),
        );
        assert_eq!(
            rel.as_deref(),
            Some("http://live.example/c.ts?start=1788831000")
        );
    }

    #[test]
    fn append_mode_appends() {
        let u = url(Mode::Append, Some("&archive={utc}"), "http://l/c?x=1", &w());
        assert_eq!(u.as_deref(), Some("http://l/c?x=1&archive=1788831000"));
    }

    #[test]
    fn shift_mode_adds_utc_and_lutc_with_the_right_separator() {
        assert_eq!(
            url(Mode::Shift, None, "http://l/c.m3u8", &w()).as_deref(),
            Some("http://l/c.m3u8?utc=1788831000&lutc=1788836400")
        );
        assert_eq!(
            url(Mode::Shift, None, "http://l/c.m3u8?tok=1", &w()).as_deref(),
            Some("http://l/c.m3u8?tok=1&utc=1788831000&lutc=1788836400")
        );
    }

    #[test]
    fn flussonic_paths() {
        assert_eq!(
            url(
                Mode::Flussonic,
                None,
                "http://f.example/ch1/index.m3u8?token=t",
                &w()
            )
            .as_deref(),
            Some("http://f.example/ch1/index-1788831000-3600.m3u8?token=t")
        );
        assert_eq!(
            url(Mode::Flussonic, None, "http://f.example/ch1/mpegts", &w()).as_deref(),
            Some("http://f.example/ch1/timeshift_abs-1788831000.ts")
        );
        assert!(resolve(Some("flussonic"), None, "http://f.example/ch1/").is_none());
    }

    #[test]
    fn xc_mode_builds_the_xtream_timeshift_path() {
        let u = url(
            Mode::Xc,
            None,
            "http://p.example:8080/live/u/p/12345.ts",
            &w(),
        );
        assert_eq!(
            u.as_deref(),
            Some("http://p.example:8080/timeshift/u/p/60/2026-09-08:01-30/12345.ts")
        );
        let short = url(Mode::Xc, None, "http://p.example/u/p/42", &w());
        assert_eq!(
            short.as_deref(),
            Some("http://p.example/timeshift/u/p/60/2026-09-08:01-30/42.ts")
        );
        // Not an Xtream URL: no catch-up, rather than a broken one.
        assert!(resolve(Some("xc"), None, "http://cdn.example/news/index.m3u8").is_none());
    }

    #[test]
    fn resolve_decides_which_channels_have_catchup() {
        let live = "http://l/c.ts";
        assert_eq!(
            resolve(Some("default"), Some("http://a/{utc}"), live),
            Some(Mode::Default)
        );
        assert_eq!(
            resolve(Some("DEFAULT"), Some("?x={utc}"), live),
            Some(Mode::Default)
        );
        // A source with no mode means default.
        assert_eq!(resolve(None, Some("?x={utc}"), live), Some(Mode::Default));
        assert_eq!(resolve(Some("timeshift"), None, live), Some(Mode::Shift));
        // Modes that need a source and have none, and modes nobody supports.
        assert_eq!(resolve(Some("default"), None, live), None);
        assert_eq!(resolve(Some("append"), Some("  "), live), None);
        assert_eq!(resolve(Some("vod"), Some("http://a"), live), None);
        assert_eq!(resolve(None, None, live), None);
    }

    #[test]
    fn stored_spellings_round_trip() {
        for m in [
            Mode::Default,
            Mode::Append,
            Mode::Shift,
            Mode::Flussonic,
            Mode::Xc,
        ] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
    }
}
