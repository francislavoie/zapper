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
    // HTTP Digest's own fields, for an `Authorization` or
    // `WWW-Authenticate` header that ends up quoted in an error (a
    // TVHeadend login): the response is a password hash, and the nonces
    // are what it was salted with. `cnonce=` is caught by `nonce=`.
    "response=",
    "nonce=",
    "opaque=",
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
///
/// Keys are matched without regard to case: an HDHomeRun's guide token
/// travels as `DeviceAuth=`, which `auth=` only catches that way, and a
/// provider that writes `Password=` is no less secret for it. A value in
/// double quotes — the shape a Digest header writes `username="bob"` in — is
/// redacted up to its closing quote, commas and all.
pub fn redact_query(text: &str) -> String {
    // ASCII lowercasing keeps every byte where it was, so offsets found in
    // the lowered copy index the original.
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while let Some((at, key_len)) = SECRET_KEYS
        .iter()
        .filter_map(|k| lower[pos..].find(k).map(|i| (pos + i, k.len())))
        .min_by_key(|(i, _)| *i)
    {
        let value_start = at + key_len;
        out.push_str(&text[pos..value_start]);
        let rest = &text[value_start..];
        // A query value ends at the next separator; everything after it —
        // including the closing bracket reqwest wraps the URL in — stays. A
        // quoted one ends at its closing quote, and the quotes stay so a
        // header still reads as one.
        let end = match rest.strip_prefix('"') {
            Some(quoted) => {
                out.push_str("\"<redacted>\"");
                quoted.find('"').map(|i| i + 2).unwrap_or(rest.len())
            }
            None => {
                out.push_str("<redacted>");
                rest.find(['&', ')', ' ', '"', '\n']).unwrap_or(rest.len())
            }
        };
        pos = value_start + end;
    }
    out.push_str(&text[pos..]);
    out
}

/// Redacts the credentials of a `Basic` authorization, which carry no key
/// for [`redact_query`] to find: the base64 token after the word is the
/// username and password.
pub fn redact_basic(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("basic ") {
        let token_start = pos + i + "basic ".len();
        let token_len = text[token_start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
            .unwrap_or(text.len() - token_start);
        out.push_str(&text[pos..token_start]);
        // "Basic auth" in a sentence is not a token; eight characters of
        // base64 is the shortest `a:b` there is.
        if token_len >= 8 {
            out.push_str("<redacted>");
        } else {
            out.push_str(&text[token_start..token_start + token_len]);
        }
        pos = token_start + token_len;
    }
    out.push_str(&text[pos..]);
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
    fn keys_match_whatever_their_case() {
        let (s, changed) =
            redact_url("https://api.hdhomerun.com/api/guide?DeviceAuth=abcDEF123&Start=1700000000");
        assert!(!s.contains("abcDEF123"), "{s}");
        assert!(s.contains("Start=1700000000"), "{s}");
        assert!(changed);
        let (s, _) = redact_url("http://p.example/get.php?USERNAME=bob&Password=hunter2");
        assert!(!s.contains("bob") && !s.contains("hunter2"), "{s}");
    }

    #[test]
    fn a_digest_header_keeps_its_shape_and_loses_its_values() {
        let h = r#"Digest username="bob", realm="tvheadend", nonce="abc,123", uri="/playlist/auth/channels", response="6629fae49393a05397450978507c4ef1", qop=auth, nc=00000001, cnonce="0a4f113b""#;
        let s = redact_query(h);
        for secret in ["bob", "abc,123", "6629fae4", "0a4f113b"] {
            assert!(!s.contains(secret), "{secret} leaked: {s}");
        }
        assert!(s.contains(r#"realm="tvheadend""#), "{s}");
        assert!(s.contains(r#"response="<redacted>""#), "{s}");
    }

    #[test]
    fn a_tvheadend_auth_code_in_a_stream_url_is_redacted() {
        let (s, _) = redact_url("http://tvh.lan:9981/stream/channelid/42?auth=Zk3x9QpL&profile=pass");
        assert_eq!(s, "http://tvh.lan:9981/stream/channelid/42?auth=<redacted>&profile=pass");
    }

    #[test]
    fn basic_credentials_are_redacted_but_the_phrase_is_not() {
        assert_eq!(
            redact_basic("Authorization: Basic Ym9iOmh1bnRlcjI="),
            "Authorization: Basic <redacted>"
        );
        assert_eq!(redact_basic("uses Basic auth"), "uses Basic auth");
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
