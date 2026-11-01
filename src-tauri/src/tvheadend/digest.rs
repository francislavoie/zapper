//! HTTP Digest authentication (RFC 7616, and RFC 2617 before it), for
//! Tvheadend.
//!
//! Tvheadend's default is Digest, and `reqwest` only does Basic. The crates
//! that bolt Digest onto `reqwest` either pin an older `reqwest` or wrap the
//! client in a middleware stack, which is a lot of tree for one header; the
//! scheme itself is two hashes and a string format. So it is here, checked
//! against the RFCs' own worked examples.
//!
//! What is supported is what a server offers in practice: `qop=auth` (and
//! the legacy no-qop form), with MD5 or SHA-256. Tvheadend offers MD5 alone
//! by default; set to SHA-256 it sends two challenges, one with
//! `algorithm=SHA-256` and one without, and the stronger is taken. Not
//! supported: `auth-int` (it hashes the body, and these are all GETs anyway,
//! but no server here asks for it), the `-sess` variants and `userhash`.
//! A challenge that needs one of those is passed over, and if it was the
//! only one the request fails as refused — which is what it is.
//!
//! **Nothing here may be logged.** The response is a password hash a
//! dictionary attack can reverse, and the nonce is what it was salted with.
//! `Challenge`'s `Debug` leaves the nonce out, and `sync_engine::scrub`
//! redacts both should a header ever end up in an error.

use md5::Md5;
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Md5,
    Sha256,
}

impl Algorithm {
    fn name(self) -> &'static str {
        match self {
            Algorithm::Md5 => "MD5",
            Algorithm::Sha256 => "SHA-256",
        }
    }

    fn hash(self, data: &str) -> String {
        match self {
            Algorithm::Md5 => hex(&Md5::digest(data.as_bytes())),
            Algorithm::Sha256 => hex(&Sha256::digest(data.as_bytes())),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One `WWW-Authenticate: Digest …` challenge.
#[derive(Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    /// The server asked for `qop=auth`. When it named no qop at all, the
    /// RFC 2069 form is used: no `nc`, no `cnonce`.
    pub qop_auth: bool,
    pub algorithm: Algorithm,
    /// The server's way of saying "right password, old nonce": answer the
    /// new challenge rather than giving up.
    pub stale: bool,
}

impl fmt::Debug for Challenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Challenge")
            .field("realm", &self.realm)
            .field("qop_auth", &self.qop_auth)
            .field("algorithm", &self.algorithm)
            .field("stale", &self.stale)
            .finish_non_exhaustive()
    }
}

/// What a server will accept, best first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scheme {
    Digest(Challenge),
    Basic,
}

/// Picks the scheme to answer from a response's `WWW-Authenticate` values:
/// Digest over Basic, SHA-256 over MD5. Basic is only ever used when the
/// server offered it — never volunteered to a server that asked for Digest,
/// since that would send the password in the clear for nothing.
pub fn choose<'a>(headers: impl IntoIterator<Item = &'a str>) -> Option<Scheme> {
    let mut best: Option<Challenge> = None;
    let mut basic = false;
    for h in headers {
        let h = h.trim();
        let (scheme, rest) = h.split_once(char::is_whitespace).unwrap_or((h, ""));
        if scheme.eq_ignore_ascii_case("basic") {
            basic = true;
        } else if scheme.eq_ignore_ascii_case("digest") {
            if let Some(c) = parse_challenge(rest) {
                let better = match &best {
                    None => true,
                    Some(b) => b.algorithm == Algorithm::Md5 && c.algorithm == Algorithm::Sha256,
                };
                if better {
                    best = Some(c);
                }
            }
        }
    }
    best.map(Scheme::Digest)
        .or_else(|| basic.then_some(Scheme::Basic))
}

/// A Digest challenge's parameters, or `None` for one this cannot answer.
pub fn parse_challenge(params: &str) -> Option<Challenge> {
    let mut realm = None;
    let mut nonce = None;
    let mut opaque = None;
    let mut qop: Option<String> = None;
    let mut algorithm = Algorithm::Md5;
    let mut stale = false;
    for (k, v) in parse_params(params) {
        match k.as_str() {
            "realm" => realm = Some(v),
            "nonce" => nonce = Some(v),
            "opaque" => opaque = Some(v),
            "qop" => qop = Some(v),
            "stale" => stale = v.eq_ignore_ascii_case("true"),
            "algorithm" => {
                algorithm = match v.to_ascii_uppercase().as_str() {
                    "MD5" => Algorithm::Md5,
                    "SHA-256" => Algorithm::Sha256,
                    // -sess, SHA-512-256: not supported (see the module notes).
                    _ => return None,
                }
            }
            _ => {}
        }
    }
    let qop_auth = match qop {
        None => false,
        Some(q) => {
            if q.split(',').any(|t| t.trim().eq_ignore_ascii_case("auth")) {
                true
            } else {
                // `auth-int` only.
                return None;
            }
        }
    };
    Some(Challenge {
        realm: realm?,
        nonce: nonce?,
        opaque,
        qop_auth,
        algorithm,
        stale,
    })
}

/// `key=value, key="quoted, with commas"` into pairs, keys lowercased.
/// Quoted values may escape a quote or backslash with a backslash.
pub fn parse_params(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while matches!(chars.peek(), Some(c) if c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c == ',' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        if key.is_empty() {
            break;
        }
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek() == Some(&'=') {
            chars.next();
            while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
                chars.next();
            }
            if chars.peek() == Some(&'"') {
                chars.next();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = chars.next() {
                                value.push(n);
                            }
                        }
                        '"' => break,
                        _ => value.push(c),
                    }
                }
            } else {
                while let Some(&c) = chars.peek() {
                    if c == ',' || c.is_whitespace() {
                        break;
                    }
                    value.push(c);
                    chars.next();
                }
            }
        }
        out.push((key.to_ascii_lowercase(), value));
    }
    out
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The `response` value: `H(H(user:realm:pass):nonce:nc:cnonce:qop:H(method:uri))`,
/// or without the `nc:cnonce:qop` part for a challenge that named no qop.
pub fn response(
    ch: &Challenge,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    nc: u32,
    cnonce: &str,
) -> String {
    let h = |s: &str| ch.algorithm.hash(s);
    let ha1 = h(&format!("{username}:{}:{password}", ch.realm));
    let ha2 = h(&format!("{method}:{uri}"));
    if ch.qop_auth {
        h(&format!("{ha1}:{}:{nc:08x}:{cnonce}:auth:{ha2}", ch.nonce))
    } else {
        h(&format!("{ha1}:{}:{ha2}", ch.nonce))
    }
}

/// The whole `Authorization` header value for one request.
pub fn authorization(
    ch: &Challenge,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    nc: u32,
    cnonce: &str,
) -> String {
    let resp = response(ch, username, password, method, uri, nc, cnonce);
    let mut v = format!(
        "Digest username={}, realm={}, nonce={}, uri={}, algorithm={}, response=\"{resp}\"",
        quote(username),
        quote(&ch.realm),
        quote(&ch.nonce),
        quote(uri),
        ch.algorithm.name(),
    );
    if let Some(o) = &ch.opaque {
        v.push_str(&format!(", opaque={}", quote(o)));
    }
    if ch.qop_auth {
        v.push_str(&format!(", qop=auth, nc={nc:08x}, cnonce=\"{cnonce}\""));
    }
    v
}

/// A client nonce: unpredictable enough that a server cannot be fooled into
/// replaying one, which is all RFC 7616 asks of it. Hashed from the clock, a
/// counter and the process, so two in a row never match.
pub fn cnonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{nanos}:{n}:{}:{:p}", std::process::id(), &COUNTER);
    hex(&Sha256::digest(seed.as_bytes()))[..32].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 2617 §3.5: the "Mufasa" example, MD5 with qop=auth.
    #[test]
    fn rfc_2617_example() {
        let ch = parse_challenge(
            r#"realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        assert!(ch.qop_auth);
        assert_eq!(ch.algorithm, Algorithm::Md5);
        let r = response(&ch, "Mufasa", "Circle Of Life", "GET", "/dir/index.html", 1, "0a4f113b");
        assert_eq!(r, "6629fae49393a05397450978507c4ef1");
    }

    const RFC7616: &str = r#"realm="http-auth@example.org", qop="auth, auth-int", algorithm=ALG, nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#;
    const CNONCE_7616: &str = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";

    /// RFC 7616 §3.9.1, both algorithms.
    #[test]
    fn rfc_7616_examples() {
        for (alg, want) in [
            ("MD5", "8ca523f5e9506fed4657c9700eebdbec"),
            (
                "SHA-256",
                "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1",
            ),
        ] {
            let ch = parse_challenge(&RFC7616.replace("ALG", alg)).unwrap();
            let r = response(&ch, "Mufasa", "Circle of Life", "GET", "/dir/index.html", 1, CNONCE_7616);
            assert_eq!(r, want, "{alg}");
        }
    }

    #[test]
    fn the_header_carries_everything_the_server_checks() {
        let ch = parse_challenge(&RFC7616.replace("ALG", "SHA-256")).unwrap();
        let h = authorization(&ch, "Mufasa", "Circle of Life", "GET", "/dir/index.html", 1, CNONCE_7616);
        assert!(h.starts_with("Digest username=\"Mufasa\", realm=\"http-auth@example.org\""), "{h}");
        for part in [
            "uri=\"/dir/index.html\"",
            "algorithm=SHA-256",
            "response=\"753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1\"",
            "opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\"",
            "qop=auth",
            "nc=00000001",
        ] {
            assert!(h.contains(part), "{part} missing from {h}");
        }
        // And it reads back as the parameters it was written from.
        let params = parse_params(h.strip_prefix("Digest ").unwrap());
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("cnonce"), Some(CNONCE_7616));
        assert_eq!(get("nonce"), Some(ch.nonce.as_str()));
    }

    #[test]
    fn tvheadends_challenges_are_read_as_it_writes_them() {
        // Unquoted qop, and on a SHA-256 server a second, MD5 challenge.
        let sha = r#"Digest realm="tvheadend", qop=auth, algorithm=SHA-256, nonce="n2", opaque="o""#;
        let md5 = r#"Digest realm="tvheadend", qop=auth, nonce="n1", opaque="o""#;
        match choose([md5, sha]) {
            Some(Scheme::Digest(c)) => {
                assert_eq!((c.algorithm, c.nonce.as_str()), (Algorithm::Sha256, "n2"))
            }
            other => panic!("{other:?}"),
        }
        match choose([md5]) {
            Some(Scheme::Digest(c)) => assert!(c.qop_auth && c.algorithm == Algorithm::Md5),
            other => panic!("{other:?}"),
        }
        assert_eq!(choose([r#"Basic realm="tvheadend""#]), Some(Scheme::Basic));
        // Digest wins over Basic when both are on offer.
        assert!(matches!(
            choose([r#"Basic realm="x""#, md5]),
            Some(Scheme::Digest(_))
        ));
        assert_eq!(choose([r#"Bearer realm="x""#]), None);
    }

    #[test]
    fn challenges_this_cannot_answer_are_passed_over() {
        assert!(parse_challenge(r#"realm="r", nonce="n", qop="auth-int""#).is_none());
        assert!(parse_challenge(r#"realm="r", nonce="n", algorithm=MD5-sess"#).is_none());
        assert!(parse_challenge(r#"realm="r""#).is_none(), "no nonce");
        let stale = parse_challenge(r#"realm="r", nonce="n", stale=TRUE"#).unwrap();
        assert!(stale.stale);
    }

    #[test]
    fn the_legacy_form_without_qop_still_answers() {
        // RFC 2069: response = H(HA1:nonce:HA2), no nc or cnonce.
        let ch = parse_challenge(r#"realm="testrealm@host.com", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093""#).unwrap();
        assert!(!ch.qop_auth);
        let h = authorization(&ch, "Mufasa", "Circle Of Life", "GET", "/dir/index.html", 1, "x");
        assert!(!h.contains("cnonce") && !h.contains("nc="), "{h}");
        let ha1 = Algorithm::Md5.hash("Mufasa:testrealm@host.com:Circle Of Life");
        let ha2 = Algorithm::Md5.hash("GET:/dir/index.html");
        let want = Algorithm::Md5.hash(&format!("{ha1}:dcd98b7102dd2f0e8b11d0f600bfb0c093:{ha2}"));
        assert!(h.contains(&want), "{h}");
    }

    #[test]
    fn quotes_in_a_username_are_escaped_and_read_back() {
        let ch = parse_challenge(r#"realm="r", nonce="n", qop=auth"#).unwrap();
        let h = authorization(&ch, r#"we"ird\name"#, "p", "GET", "/", 1, "c");
        let params = parse_params(h.strip_prefix("Digest ").unwrap());
        assert_eq!(params[0], ("username".into(), r#"we"ird\name"#.into()));
    }

    #[test]
    fn client_nonces_differ_and_the_challenge_debug_hides_the_nonce() {
        assert_ne!(cnonce(), cnonce());
        let ch = parse_challenge(r#"realm="r", nonce="SECRETNONCE", qop=auth"#).unwrap();
        assert!(!format!("{ch:?}").contains("SECRETNONCE"));
    }
}
