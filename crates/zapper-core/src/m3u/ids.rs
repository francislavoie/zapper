//! Ids a playlist does not carry.
//!
//! Everything keyed on a channel — watch history, favourites, quality
//! measurements, the channel remembered across a restart — is keyed on
//! `stream_id`, and an M3U entry has no such thing. So one is derived, and it
//! has to come out the same every time the playlist is read, or a refresh
//! would quietly orphan all of that.
//!
//! **What identifies an entry is what the viewer would call it**, not where
//! it streams from. Providers put session tokens in their stream URLs — in
//! the query (`?token=…&expires=…`) and, for Xtream exports, in the path
//! (`/live/<user>/<pass>/123.ts`) — and many rotate them on every download,
//! so a URL is the least stable thing in an entry. In order of preference:
//!
//! 1. `tvg-id`, the display name and the group, when the entry has a
//!    `tvg-id`. The id alone is not enough: the SD, HD and backup copies of a
//!    channel share it and are three channels to the viewer.
//! 2. The display name and the group.
//! 3. Only when the entry states no name at all: its URL, with the parts that
//!    rotate taken out ([`stable_url`]).
//!
//! The name is compared loosely ([`loose_name`]: case and punctuation
//! ignored) but not so loosely that `Northwind One HD` and `Northwind One SD` meet —
//! those are different pictures, and the quality word is how the playlist
//! says so.
//!
//! Two entries with the same identity — a channel listed twice under one
//! name, a backup copy with no label of its own — are told apart by their
//! order of appearance: the second gets ordinal 1 mixed in. That is stable as
//! long as the duplicates keep their order relative to each other; the rest
//! of the playlist can be reshuffled freely.
//!
//! The cost of all this: renaming a channel, or moving it to another group,
//! makes it a new channel. That is a deliberate edit by the playlist's author,
//! which a token rotation is not.
//!
//! FNV-1a rather than `DefaultHasher`: std makes no promise that its hasher
//! is stable across releases, and these ids live on disk.

use super::Entry;

/// 64-bit FNV-1a.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The lowest id a playlist channel can have.
///
/// Above every id an Xtream server issues (they are database row numbers, in
/// the millions at most), so a mirror that switches between the two kinds
/// never confuses one server's channel 123456 with a playlist entry.
pub const ID_FLOOR: i64 = 1 << 40;

/// One past the highest: 2^53, because the id crosses into JavaScript as a
/// `number` and must survive being a double.
pub const ID_CEILING: i64 = 1 << 53;

/// Query keys whose values rotate or carry the account. Matched whole and
/// case-insensitively, unlike redaction's substring match: dropping `e=` from
/// an id key must not also drop `type=`.
const VOLATILE_KEYS: &[&str] = &[
    "token",
    "access_token",
    "auth_token",
    "key",
    "apikey",
    "api_key",
    "auth",
    "password",
    "pass",
    "pwd",
    "username",
    "user",
    "sig",
    "signature",
    "e",
    "expires",
    "exp",
    "st",
    "md5",
    "hash",
    "hdnts",
    "hdnea",
    "session",
    "sid",
];

/// A URL with everything that rotates or names the account taken out:
/// userinfo, the fragment, [`VOLATILE_KEYS`] in the query, and the
/// `<user>/<pass>` segments of an Xtream path (`/live/u/p/123.ts`, or the
/// bare `/u/p/123`).
pub fn stable_url(url: &str) -> String {
    let url = url.trim();
    let url = url.split('#').next().unwrap_or(url);
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    let (scheme, rest) = match base.split_once("://") {
        Some((s, r)) => (Some(s), r),
        None => (None, base),
    };
    let (authority, path) = match (scheme, rest.split_once('/')) {
        (Some(_), Some((a, p))) => (a, p),
        (Some(_), None) => (rest, ""),
        (None, _) => ("", rest),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);

    let segs: Vec<&str> = path.split('/').collect();
    let numeric_tail = |s: &str| {
        let stem = s.split('.').next().unwrap_or(s);
        !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit())
    };
    let kept: Vec<&str> = match segs.as_slice() {
        ["live", _, _, tail @ ..] if !tail.is_empty() => std::iter::once("live")
            .chain(tail.iter().copied())
            .collect(),
        [_, _, last] if numeric_tail(last) => vec![*last],
        _ => segs,
    };

    let mut out = String::with_capacity(url.len());
    if let Some(s) = scheme {
        out.push_str(&s.to_ascii_lowercase());
        out.push_str("://");
        out.push_str(&authority.to_ascii_lowercase());
        out.push('/');
    }
    out.push_str(&kept.join("/"));
    if let Some(q) = query {
        let pairs: Vec<&str> = q
            .split('&')
            .filter(|p| !p.is_empty())
            .filter(|p| {
                let k = p.split('=').next().unwrap_or(p);
                !VOLATILE_KEYS.iter().any(|v| v.eq_ignore_ascii_case(k))
            })
            .collect();
        if !pairs.is_empty() {
            out.push('?');
            out.push_str(&pairs.join("&"));
        }
    }
    out
}

/// A display name with case and punctuation ignored: `Northwind One`, `northwind-one`
/// and `NORTHWIND  ONE` are one name; `Northwind One HD` is another.
pub fn loose_name(name: &str) -> String {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The identity an entry's id is derived from (see the module notes).
/// Prefixed by which rule produced it, so a name can never collide with a
/// URL that happens to spell the same thing.
pub fn identity(e: &Entry) -> String {
    let group = e.group.as_deref().map(loose_name).unwrap_or_default();
    let tvg_id = e.tvg_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let name = loose_name(&e.name);
    match tvg_id {
        Some(id) if e.named => format!("t\u{0}{id}\u{0}{name}\u{0}{group}"),
        _ if e.named && !name.is_empty() => format!("n\u{0}{name}\u{0}{group}"),
        _ => format!("u\u{0}{}", stable_url(&e.url)),
    }
}

/// The id for the `occurrence`th (0-based) entry with this identity.
pub fn stream_id(identity: &str, occurrence: u32) -> i64 {
    let h = if occurrence == 0 {
        fnv1a(identity.as_bytes())
    } else {
        fnv1a(format!("{identity}\u{0}#{occurrence}").as_bytes())
    };
    let span = (ID_CEILING - ID_FLOOR) as u64;
    ID_FLOOR + (h % span) as i64
}

/// A category's id: the group's name, hashed, so renaming nothing keeps it
/// and the rail's remembered selection survives a refresh.
///
/// Prefixed so it can never equal an Xtream server's numeric category id.
pub fn category_id(group: &str) -> String {
    format!("m3u-{:016x}", fnv1a(group.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_matches_the_reference_vectors() {
        // From the FNV reference: the empty string and "a".
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn ids_are_stable_and_in_range() {
        let a = stream_id("n\u{0}northwind one\u{0}uk", 0);
        assert_eq!(a, stream_id("n\u{0}northwind one\u{0}uk", 0));
        for id in [a, stream_id("x", 0), stream_id("", 7)] {
            assert!((ID_FLOOR..ID_CEILING).contains(&id), "{id} out of range");
        }
    }

    #[test]
    fn a_repeated_identity_gets_a_distinct_id_per_occurrence() {
        let k = "n\u{0}same\u{0}";
        assert_ne!(stream_id(k, 0), stream_id(k, 1));
        assert_ne!(stream_id(k, 1), stream_id(k, 2));
    }

    #[test]
    fn rotating_parts_are_taken_out_of_urls() {
        assert_eq!(
            stable_url("http://cdn.example/news/index.m3u8?token=abc&e=123&quality=hd"),
            "http://cdn.example/news/index.m3u8?quality=hd"
        );
        assert_eq!(
            stable_url("http://cdn.example/news/index.m3u8?token=xyz&e=999&quality=hd"),
            stable_url("http://cdn.example/news/index.m3u8?token=abc&e=123&quality=hd"),
        );
        // Xtream paths, both shapes.
        assert_eq!(
            stable_url("http://P.example:8080/live/bob/hunter2/123.ts"),
            "http://p.example:8080/live/123.ts"
        );
        assert_eq!(
            stable_url("http://p.example/bob/hunter2/123"),
            "http://p.example/123"
        );
        // Userinfo and fragments.
        assert_eq!(
            stable_url("http://a:b@h.example/x.ts#t=1"),
            "http://h.example/x.ts"
        );
        // A key that merely contains a volatile one survives.
        assert_eq!(
            stable_url("http://h/x?type=m3u&level=2"),
            "http://h/x?type=m3u&level=2"
        );
        // An ordinary three-segment path is not mistaken for Xtream.
        assert_eq!(
            stable_url("http://h/a/b/index.m3u8"),
            "http://h/a/b/index.m3u8"
        );
    }

    fn entry(
        name: &str,
        named: bool,
        tvg_id: Option<&str>,
        group: Option<&str>,
        url: &str,
    ) -> Entry {
        Entry {
            name: name.into(),
            named,
            tvg_id: tvg_id.map(Into::into),
            group: group.map(Into::into),
            url: url.into(),
            ..Entry::default()
        }
    }

    #[test]
    fn identity_prefers_tvg_id_then_name_then_url() {
        let with_id = entry(
            "Northwind One HD",
            true,
            Some("nw1.uk"),
            Some("UK"),
            "http://s/1?token=a",
        );
        let rotated = entry(
            "Northwind One HD",
            true,
            Some("nw1.uk"),
            Some("UK"),
            "http://s/9?token=b",
        );
        // The URL plays no part once there is a name.
        assert_eq!(identity(&with_id), identity(&rotated));
        let sd = entry(
            "Northwind One SD",
            true,
            Some("nw1.uk"),
            Some("UK"),
            "http://s/1",
        );
        assert_ne!(
            identity(&with_id),
            identity(&sd),
            "SD and HD are two channels"
        );
        let other_group = entry(
            "Northwind One HD",
            true,
            Some("nw1.uk"),
            Some("Backup"),
            "http://s/1",
        );
        assert_ne!(identity(&with_id), identity(&other_group));

        let no_id = entry("northwind-one  hd", true, None, Some("uk"), "http://s/1");
        assert!(identity(&no_id).starts_with("n\u{0}northwind one hd"));

        let unnamed_a = entry("1", false, None, None, "http://p/live/u/p/1.ts?token=a");
        let unnamed_b = entry("1", false, None, None, "http://p/live/u/p/1.ts?token=b");
        assert!(identity(&unnamed_a).starts_with("u\u{0}"));
        assert_eq!(identity(&unnamed_a), identity(&unnamed_b));
    }

    #[test]
    fn category_ids_never_look_like_xtream_ones() {
        let id = category_id("Sports");
        assert!(id.starts_with("m3u-"));
        assert_eq!(id, category_id("Sports"));
        assert_ne!(id, category_id("sports"));
    }
}
