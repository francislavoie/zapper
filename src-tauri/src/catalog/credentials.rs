//! Where the panel credentials come from, until zapper has settings of its
//! own: LegacyApp's profile file.
//!
//! Nothing here may log a value. `Credentials` redacts itself in `Debug`;
//! keep it that way and never format the fields directly.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

use crate::xtream::Credentials;

const PROFILE: &str = ".var/app/org.example.legacyapp/config/LegacyApp/profiles.json";

fn profile_path_under(home: &Path) -> PathBuf {
    home.join(PROFILE)
}

pub fn profile_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    profile_path_under(Path::new(&home))
}

/// Pulls the first profile that carries a host, a username and a password.
///
/// Deliberately tolerant about shape — this is another application's file and
/// nothing stops it changing — and deliberately silent about content: no
/// error from here may quote the JSON, because the JSON is a password.
pub fn parse(json: &str) -> Result<Credentials> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| anyhow!("profiles.json is not valid JSON ({e})"))?;

    let list = root
        .as_array()
        .or_else(|| root.get("profiles").and_then(|p| p.as_array()))
        .ok_or_else(|| anyhow!("profiles.json holds neither a list nor a `profiles` list"))?;

    let field = |p: &serde_json::Value, keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| p.get(*k).and_then(|v| v.as_str()))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };

    let candidate = |p: &serde_json::Value| {
        Some(Credentials {
            host: field(p, &["host", "url", "server"])?,
            username: field(p, &["username", "user"])?,
            password: field(p, &["password", "pass"])?,
        })
    };
    let is_xtream = |p: &&serde_json::Value| {
        p.get("type").and_then(|t| t.as_str()) == Some("xtream")
    };

    // A declared Xtream profile wins; an M3U profile can carry the same three
    // keys and would serve a playlist this panel refuses to export anyway.
    list.iter()
        .filter(is_xtream)
        .find_map(candidate)
        .or_else(|| list.iter().find_map(candidate))
        .ok_or_else(|| anyhow!("no Xtream profile with a host, username and password"))
}

pub fn load() -> Result<Credentials> {
    let path = profile_path();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    parse(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_path_is_the_legacyapp_flatpak_config() {
        let p = profile_path_under(Path::new("/home/x"));
        assert_eq!(
            p,
            PathBuf::from(
                "/home/x/.var/app/org.example.legacyapp/config/LegacyApp/profiles.json"
            )
        );
    }

    #[test]
    fn parses_the_first_xtream_profile_from_a_list() {
        let json = r#"[
            {"name":"vod thing","type":"m3u","url":"http://elsewhere/list.m3u"},
            {"name":"panel","type":"xtream","host":"http://panel.example:8080/",
             "username":"user","password":"secret"}
        ]"#;
        let c = parse(json).unwrap();
        // The trailing slash is normalised away by XtreamClient, not here.
        assert_eq!(c.host, "http://panel.example:8080/");
        assert_eq!(c.username, "user");
        assert_eq!(c.password, "secret");
    }

    #[test]
    fn a_declared_xtream_profile_beats_an_earlier_m3u_one() {
        let json = r#"[
            {"type":"m3u","url":"http://elsewhere","username":"wrong","password":"wrong"},
            {"type":"xtream","host":"http://panel","username":"right","password":"right"}
        ]"#;
        assert_eq!(parse(json).unwrap().username, "right");
    }

    #[test]
    fn accepts_a_wrapped_object_and_url_as_the_host_key() {
        let json = r#"{"profiles":[
            {"url":"http://panel.example","username":"u","password":"p"}
        ]}"#;
        let c = parse(json).unwrap();
        assert_eq!(c.host, "http://panel.example");
    }

    #[test]
    fn a_profile_without_credentials_is_not_a_candidate() {
        let json = r#"[{"type":"m3u","url":"http://elsewhere/list.m3u"}]"#;
        let err = parse(json).unwrap_err().to_string();
        assert!(
            err.contains("no Xtream profile"),
            "unhelpful error: {err}"
        );
    }

    #[test]
    fn the_error_never_carries_the_file_contents() {
        // A parse failure must not echo the file back: it holds a password.
        let err = parse("{ not json").unwrap_err().to_string();
        assert!(!err.contains("not json"), "profile content leaked: {err}");
    }

    /// Parses the real LegacyApp profile on this machine.
    ///
    /// Every other test here uses synthetic JSON, so a wrong assumption about
    /// the file's actual shape would pass all of them and fail only at
    /// runtime. Ignored because the file is machine-specific.
    ///
    ///   cargo test --lib real_legacyapp_profile -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_legacyapp_profile_parses() {
        let path = profile_path();
        if !path.exists() {
            panic!("no profile at {}", path.display());
        }
        let raw = std::fs::read_to_string(&path).expect("reading profile");
        let c = parse(&raw).expect("parsing the saved profile");

        assert!(c.host.starts_with("http"), "host looks wrong");
        assert!(!c.username.is_empty(), "username empty");
        assert!(!c.password.is_empty(), "password empty");
        // Print shape only. Never print the values: this file holds live
        // account credentials and test output ends up in logs.
        println!(
            "parsed ok: host={}… username={} chars, password={} chars",
            &c.host[..c.host.len().min(12)],
            c.username.len(),
            c.password.len()
        );
    }
}
