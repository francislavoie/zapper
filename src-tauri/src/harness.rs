//! Keeping test runs away from the real account.
//!
//! Many providers allow one connection per account. A harness that opens a
//! stream through the account on the machine — or merely syncs against it —
//! competes with whoever is watching on it, and the provider settles that by
//! ending one of the two. So nothing unattended uses the real account unless
//! it is asked to, in so many words: `ZAPPER_ALLOW_REAL_ACCOUNT=1`.
//!
//! Without it, a run is pointed at a scratch directory ([`isolate`]), which
//! has no account and no catalogue in it, and streams come from the local
//! fixture server (`tools/fake_stream.py`) rather than a provider
//! ([`local_url`]).

use std::path::PathBuf;

/// The opt-in, and the only one.
pub const ALLOW_REAL_ACCOUNT: &str = "ZAPPER_ALLOW_REAL_ACCOUNT";

/// Where `tools/fake_stream.py` serves its fixtures by default.
pub const FIXTURE_BASE: &str = "http://127.0.0.1:8788";

/// The fixture a harness plays when it is not told otherwise.
pub const FIXTURE_URL: &str = "http://127.0.0.1:8788/sample1080.ts";

/// Whether this run may use the real account and catalogue.
///
/// Exactly `1`: a variable that is merely present, or set to `yes` by a
/// script that meant something else, is not consent.
pub fn real_account_allowed() -> bool {
    std::env::var(ALLOW_REAL_ACCOUNT).is_ok_and(|v| v.trim() == "1")
}

/// Points this process's config, data and cache at a scratch directory,
/// unless the real account is allowed. Returns whether it is.
///
/// Call first thing in `main`, before any thread exists and before anything
/// has read a path: the directories are read from the environment each time
/// (see [`crate::paths`]), so setting it here moves every later read.
///
/// A `ZAPPER_HOME` that is already set is left alone — a caller that chose a
/// directory, such as `tools/check_all.sh`, knows where it wants things.
pub fn isolate(what: &str) -> bool {
    if real_account_allowed() {
        eprintln!("{what}: {ALLOW_REAL_ACCOUNT}=1, so this run uses the real account and catalogue");
        return true;
    }
    if std::env::var_os(crate::paths::HOME_OVERRIDE).is_none_or(|v| v.is_empty()) {
        let dir = scratch_dir(what);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("{what}: could not create {}: {e}", dir.display());
        }
        // Single-threaded at this point by the contract above, which is what
        // makes changing the environment sound.
        std::env::set_var(crate::paths::HOME_OVERRIDE, &dir);
    }
    false
}

fn scratch_dir(what: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zapper-{what}-{}", std::process::id()))
}

/// Refuses a stream URL that is not on this machine, unless the real account
/// is allowed.
///
/// Anything off the loopback is presumed to be a provider: a harness has no
/// business anywhere else, and a URL passed in from a shell history may well
/// carry the account in its path.
pub fn local_url(url: &str) -> Result<(), String> {
    if is_local(url) || real_account_allowed() {
        return Ok(());
    }
    Err(format!(
        "refusing a stream that is not on this machine (it may be your provider, and \
         opening it could end someone's viewing). Serve a fixture with tools/fake_stream.py \
         and use {FIXTURE_URL}, or set {ALLOW_REAL_ACCOUNT}=1 to mean it."
    ))
}

/// The stream a URL-taking harness should open: the one it was given, or
/// the default fixture. Isolates the run first ([`isolate`]), and refuses a
/// URL off this machine unless the real account is allowed.
pub fn stream_url(what: &str, given: Option<String>) -> Result<String, String> {
    isolate(what);
    let url = given
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| FIXTURE_URL.to_owned());
    local_url(&url)?;
    Ok(url)
}

/// Whether a URL names this machine: the loopback, a local file, or one of
/// mpv's generated sources.
pub fn is_local(url: &str) -> bool {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        // No scheme: a path on disk. A Windows drive letter (`C:\…`) lands
        // here too, which is right.
        return !url.is_empty();
    };
    match scheme.to_ascii_lowercase().as_str() {
        "file" | "av" | "lavfi" => return true,
        "http" | "https" | "rtsp" | "rtmp" | "udp" | "tcp" | "rtp" => {}
        _ => return false,
    }
    // The authority: up to the first `/`, `?` or `#`, without a login.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_server_is_local() {
        assert!(is_local(FIXTURE_URL));
        assert!(is_local("http://localhost:9000/x.ts"));
        assert!(is_local("http://127.0.0.2/x.ts"));
        assert!(is_local("http://[::1]:8788/x.ts"));
        assert!(is_local("HTTP://LOCALHOST/x"));
    }

    #[test]
    fn files_and_generated_sources_are_local() {
        assert!(is_local("/home/u/.cache/zapper/fixtures/sample1080.ts"));
        assert!(is_local("file:///tmp/a.ts"));
        assert!(is_local("av://lavfi:anullsrc"));
    }

    #[test]
    fn a_provider_is_not() {
        assert!(!is_local("http://provider.example:8080/live/u/p/1.ts"));
        assert!(!is_local("https://10.0.0.5/stream"));
        assert!(!is_local("http://192.168.1.20:5004/auto/v5.1"));
        assert!(!is_local(""));
    }

    #[test]
    fn a_login_or_a_lookalike_does_not_fool_it() {
        // The host is after the `@`, not before it.
        assert!(!is_local("http://127.0.0.1@provider.example/x"));
        assert!(is_local("http://user:pw@127.0.0.1:8788/x"));
        // A name that merely starts with the loopback's is someone else.
        assert!(!is_local("http://127.0.0.1.provider.example/x"));
        assert!(!is_local("http://localhost.provider.example/x"));
        // And an unknown scheme is not presumed safe.
        assert!(!is_local("srt://127.0.0.1:9000"));
    }
}
