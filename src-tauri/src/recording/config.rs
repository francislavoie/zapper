//! The recording service's settings and its API token, on disk.
//!
//! Both live in zapper's config directory, so on one machine the app finds
//! the service's port and token where the service left them, with nothing to
//! pair or type:
//!
//! - `server.json`: the port and the recording settings. Written by the
//!   service alone (the app changes settings through the API), read by both.
//! - `server-token`: the shared secret every API request carries, made by
//!   the service on its first start, readable by this user only.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::api::RecordingSettingsDto;

/// Chosen to stay clear of the common development ports and of zapper's own
/// fixture server (8788).
pub const DEFAULT_PORT: u16 = 8796;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    /// On 127.0.0.1 only, for now.
    pub port: u16,
    pub output_dir: String,
    pub pad_before: i64,
    pub pad_after: i64,
    pub clock_track: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            output_dir: default_output_dir().to_string_lossy().into_owned(),
            pad_before: 60,
            pad_after: 5 * 60,
            clock_track: true,
        }
    }
}

impl ServerConfig {
    pub fn settings(&self) -> RecordingSettingsDto {
        RecordingSettingsDto {
            output_dir: self.output_dir.clone(),
            pad_before: self.pad_before,
            pad_after: self.pad_after,
            clock_track: self.clock_track,
            default_output_dir: default_output_dir().to_string_lossy().into_owned(),
        }
    }

    /// Takes new settings, refusing ones that cannot work. An empty folder
    /// means the default one.
    pub fn apply(&mut self, s: RecordingSettingsDto) -> Result<(), String> {
        let default = default_output_dir().to_string_lossy().into_owned();
        let dir = match s.output_dir.trim() {
            "" => default.as_str(),
            d => d,
        };
        if !Path::new(dir).is_absolute() {
            return Err("the output folder must be a full path".into());
        }
        const MAX_PAD: i64 = 6 * 3600;
        if !(0..=MAX_PAD).contains(&s.pad_before) || !(0..=MAX_PAD).contains(&s.pad_after) {
            return Err("padding must be between 0 and 6 hours".into());
        }
        self.output_dir = dir.to_string();
        self.pad_before = s.pad_before;
        self.pad_after = s.pad_after;
        self.clock_track = s.clock_track;
        Ok(())
    }
}

pub fn config_path() -> PathBuf {
    crate::paths::config_dir().join("server.json")
}

pub fn token_path() -> PathBuf {
    crate::paths::config_dir().join("server-token")
}

/// The settings on file, or the defaults where there are none.
pub fn load() -> ServerConfig {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> ServerConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("{} is not valid ({e}); using the defaults", path.display());
            ServerConfig::default()
        }),
        Err(_) => ServerConfig::default(),
    }
}

pub fn save(cfg: &ServerConfig) -> Result<()> {
    save_to(&config_path(), cfg)
}

pub fn save_to(path: &Path, cfg: &ServerConfig) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    // Through a temporary file, so a reader never sees half of it.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cfg)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// `~/Videos/zapper`, or under `ZAPPER_HOME` when that is set: a run pointed
/// at a scratch directory (a harness) must not write into the real videos.
pub fn default_output_dir() -> PathBuf {
    if cfg!(test) || std::env::var_os(crate::paths::HOME_OVERRIDE).is_some_and(|v| !v.is_empty()) {
        return crate::paths::data_dir().join("recordings");
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(crate::paths::data_dir);
    home.join("Videos").join("zapper")
}

/// The token on file, if there is one.
pub fn read_token() -> Option<String> {
    read_token_from(&token_path())
}

pub fn read_token_from(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Writes `token` readable and writable by this user alone.
///
/// Created with that mode rather than chmod-ed after, so it is never
/// readable by others, even for a moment.
pub fn write_token(path: &Path, token: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    let mut f = opts
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(token.as_bytes())?;
    #[cfg(unix)]
    {
        // An existing file keeps its old mode through `open`; make sure.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Compares without stopping at the first difference, so the time a wrong
/// guess takes says nothing about how close it was.
pub fn token_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zapper-reccfg-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn settings_round_trip_and_missing_fields_take_defaults() {
        let d = scratch("rt");
        let p = d.join("server.json");
        assert_eq!(load_from(&p), ServerConfig::default());
        let mut c = ServerConfig {
            port: 9000,
            ..Default::default()
        };
        c.clock_track = false;
        save_to(&p, &c).unwrap();
        assert_eq!(load_from(&p), c);
        std::fs::write(&p, r#"{"port": 9100}"#).unwrap();
        let c = load_from(&p);
        assert_eq!((c.port, c.pad_after, c.clock_track), (9100, 300, true));
    }

    #[test]
    fn unworkable_settings_are_refused() {
        let mut c = ServerConfig::default();
        let mut s = c.settings();
        s.output_dir = "relative/dir".into();
        assert!(c.apply(s.clone()).is_err());
        s.output_dir = "/srv/tv".into();
        s.pad_after = -1;
        assert!(c.apply(s.clone()).is_err());
        s.pad_after = 600;
        c.apply(s.clone()).unwrap();
        assert_eq!((c.output_dir.as_str(), c.pad_after), ("/srv/tv", 600));
        s.output_dir = String::new();
        c.apply(s).unwrap();
        assert_eq!(c.output_dir, c.settings().default_output_dir, "empty is the default");
    }

    #[test]
    fn the_token_is_private_and_compared_whole() {
        let d = scratch("tok");
        let p = d.join("server-token");
        write_token(&p, "abc123").unwrap();
        assert_eq!(read_token_from(&p).as_deref(), Some("abc123"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(token_matches("abc123", "abc123"));
        assert!(!token_matches("abc123", "abc124"));
        assert!(!token_matches("abc123", "abc12"));
    }
}
