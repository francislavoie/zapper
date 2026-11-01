//! Taking the account out of a URL before anybody sees it.
//!
//! An Xtream URL has a known shape, so its credentials are easy to find. A
//! playlist URL can have any shape the provider likes, and in practice the
//! account turns up in three places: the query string (`username=`,
//! `password=`, `token=`…), the userinfo (`http://user:pass@host/`), and —
//! for playlists exported from an Xtream server — the path itself
//! (`/live/user/pass/123.ts`). The first two are recognisable and redacted
//! here. The third is not distinguishable from an ordinary path, which is why
//! every M3U request also drops its URL from `reqwest`'s error before the
//! error goes anywhere (see `fetch`), rather than relying on this alone.

/// Query keys whose values are credentials, or close enough that showing
/// them is not worth the risk. Matched as `key=` anywhere in the text, so
/// `apikey=` is caught by `key=` too; over-redaction costs nothing.
pub const SECRET_KEYS: &[&str] = &[
    "username=",
    "password=",
    "pass=",
    "user=",
    "passwd=",
    "pwd=",
    "token=",
    "auth=",
    "key=",
    "signature=",
    "sig=",
];

/// Redacts `scheme://user:pass@host` down to `scheme://<redacted>@host`,
/// wherever such a URL appears in `text`.
pub fn redact_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let auth_start = at + 3;
        out.push_str(&rest[..auth_start]);
        let tail = &rest[auth_start..];
        // The authority ends where the path, query or fragment begins — or,
        // in prose, where the URL itself does.
        let auth_end = tail
            .find(['/', '?', '#', ' ', ')', '"', '\n'])
            .unwrap_or(tail.len());
        let authority = &tail[..auth_end];
        match authority.rfind('@') {
            Some(i) => {
                out.push_str("<redacted>");
                out.push_str(&authority[i..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[auth_end..];
    }
    out.push_str(rest);
    out
}

/// Redacts the values of [`SECRET_KEYS`] wherever they appear in `text`.
pub fn redact_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((at, key_len)) = SECRET_KEYS
        .iter()
        .filter_map(|k| rest.find(k).map(|i| (i, k.len())))
        .min_by_key(|(i, _)| *i)
    {
        let value_start = at + key_len;
        out.push_str(&rest[..value_start]);
        out.push_str("<redacted>");
        // A query value ends at the next separator; everything after it —
        // including the closing bracket reqwest wraps the URL in — stays.
        let end = rest[value_start..]
            .find(['&', ')', ' ', '"', '\n'])
            .map(|i| value_start + i)
            .unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A URL fit to show, and whether anything had to be taken out of it.
///
/// The flag is what lets the settings pane treat a secret-bearing playlist
/// URL like a password — shown as set, never sent back — while an ordinary
/// URL is shown as typed.
pub fn redact_url(url: &str) -> (String, bool) {
    let safe = redact_query(&redact_userinfo(url));
    let changed = safe != url;
    (safe, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_is_redacted_and_the_host_kept() {
        let (s, changed) = redact_url("http://bob:hunter2@epg.example:8080/guide.xml");
        assert_eq!(s, "http://<redacted>@epg.example:8080/guide.xml");
        assert!(changed);
    }

    #[test]
    fn query_credentials_are_redacted() {
        let (s, changed) =
            redact_url("http://p.example/get.php?username=bob&password=hunter2&type=m3u_plus");
        assert_eq!(
            s,
            "http://p.example/get.php?username=<redacted>&password=<redacted>&type=m3u_plus"
        );
        assert!(changed);
    }

    #[test]
    fn tokens_are_redacted_too() {
        let (s, _) = redact_url("https://cdn.example/list.m3u?token=abc123&x=1");
        assert!(!s.contains("abc123"), "{s}");
        let (s, _) = redact_url("https://cdn.example/list.m3u?apikey=abc123");
        assert!(!s.contains("abc123"), "{s}");
    }

    #[test]
    fn a_url_with_nothing_secret_is_left_alone() {
        let url = "https://iptv-org.github.io/iptv/index.m3u";
        assert_eq!(redact_url(url), (url.to_owned(), false));
        assert_eq!(
            redact_url("/home/me/tv.m3u"),
            ("/home/me/tv.m3u".to_owned(), false)
        );
    }

    #[test]
    fn urls_inside_prose_are_found() {
        let s = redact_userinfo("error sending request for url (http://a:b@h/x): timed out");
        assert_eq!(
            s,
            "error sending request for url (http://<redacted>@h/x): timed out"
        );
    }
}
