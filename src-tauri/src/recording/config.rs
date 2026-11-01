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
//!
//! Other devices on the network sign in with a password. What the service
//! keeps for that (the password's hash, the devices it let in, the HTTPS
//! certificate's key) has files of its own beside these, readable by this
//! user only, so `server.json` holds no secret (see `recording::access`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::api::RecordingSettingsDto;
use super::naming::{self, Naming};

/// Chosen to stay clear of the common development ports and of zapper's own
/// fixture server (8788).
pub const DEFAULT_PORT: u16 = 8796;

/// The lowest port Settings offers: below it a Unix system wants root to
/// listen at all.
pub const MIN_PORT: u16 = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    /// Where the service listens: on 127.0.0.1 for the app, and on every
    /// interface too while `network.enabled`. One port for both, so the app
    /// finds the service at the same address either way.
    pub port: u16,
    pub network: NetworkConfig,
    pub output_dir: String,
    pub pad_before: i64,
    pub pad_after: i64,
    pub clock_track: bool,
    pub naming: Naming,
    pub naming_pattern: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            network: NetworkConfig::default(),
            output_dir: default_output_dir().to_string_lossy().into_owned(),
            pad_before: 60,
            pad_after: 5 * 60,
            clock_track: true,
            naming: Naming::default(),
            naming_pattern: String::new(),
        }
    }
}

/// Whether, and how, other devices on the network reach the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NetworkConfig {
    /// Listen on every interface rather than 127.0.0.1 only. Off unless
    /// turned on, and only with a sign-in password set.
    pub enabled: bool,
    /// Other devices are served HTTPS, with the service's own certificate.
    /// Off means plain HTTP on the network, which anyone on it can read.
    /// This computer's own requests (127.0.0.1) may be plain HTTP either way.
    pub https: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            https: true,
        }
    }
}

/// A port Settings may choose: one a user may listen on without root.
pub fn validate_port(port: u16) -> Result<(), String> {
    if port < MIN_PORT {
        return Err(format!("the port must be between {MIN_PORT} and 65535"));
    }
    Ok(())
}

impl ServerConfig {
    pub fn settings(&self) -> RecordingSettingsDto {
        RecordingSettingsDto {
            output_dir: self.output_dir.clone(),
            pad_before: self.pad_before,
            pad_after: self.pad_after,
            clock_track: self.clock_track,
            naming: self.naming,
            naming_pattern: self.naming_pattern.clone(),
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
        // A custom pattern is checked even while a preset is chosen, so the
        // one on file is always one that can be switched to.
        let pattern = s.naming_pattern.trim();
        if s.naming == Naming::Custom || !pattern.is_empty() {
            naming::validate(pattern).map_err(|e| format!("file name pattern: {e}"))?;
        }
        self.output_dir = dir.to_string();
        self.pad_before = s.pad_before;
        self.pad_after = s.pad_after;
        self.clock_track = s.clock_track;
        self.naming = s.naming;
        self.naming_pattern = pattern.to_string();
        Ok(())
    }

    /// The pattern new recordings are named by.
    pub fn pattern(&self) -> &str {
        naming::pattern_for(self.naming, &self.naming_pattern)
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
pub fn write_token(path: &Path, token: &str) -> Result<()> {
    write_private(path, token.as_bytes())
}

/// Writes a file readable and writable by this user alone, whole: through a
/// temporary file beside it, so a reader never sees half of it.
///
/// The temporary file is created with that mode rather than chmod-ed after,
/// so its contents are never readable by others, even for a moment.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    let mut f = opts
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    #[cfg(unix)]
    {
        // One left by an interrupted write keeps its old mode through
        // `open`; make sure before anything goes into it.
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(contents)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
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
        assert_eq!(
            (c.network.enabled, c.network.https),
            (false, true),
            "network access is off, and HTTPS when it is on, unless chosen"
        );
        std::fs::write(&p, r#"{"network": {"enabled": true}}"#).unwrap();
        let n = load_from(&p).network;
        assert_eq!((n.enabled, n.https), (true, true));
    }

    #[test]
    fn ports_below_1024_are_refused() {
        assert!(validate_port(80).is_err());
        assert!(validate_port(1023).is_err());
        assert!(validate_port(1024).is_ok());
        assert!(validate_port(DEFAULT_PORT).is_ok());
        assert!(validate_port(65535).is_ok());
    }

    #[test]
    fn unworkable_settings_are_refused() {
        let mut c = ServerConfig::default();
        let mut s = c.settings();
        s.output_dir = "relative/dir".into();
        assert!(c.apply(s.clone()).is_err());
        // Absolute on this platform: `/srv/tv` has no drive on Windows.
        let absolute = if cfg!(windows) { r"D:\TV" } else { "/srv/tv" };
        s.output_dir = absolute.into();
        s.pad_after = -1;
        assert!(c.apply(s.clone()).is_err());
        s.pad_after = 600;
        c.apply(s.clone()).unwrap();
        assert_eq!((c.output_dir.as_str(), c.pad_after), (absolute, 600));
        s.output_dir = String::new();
        c.apply(s).unwrap();
        assert_eq!(c.output_dir, c.settings().default_output_dir, "empty is the default");
    }

    #[test]
    fn a_naming_pattern_is_saved_only_when_it_works() {
        let mut c = ServerConfig::default();
        assert_eq!(c.pattern(), naming::MEDIA_SERVER);
        let mut s = c.settings();
        s.naming = Naming::Custom;
        s.naming_pattern = "../{show}".into();
        let err = c.apply(s.clone()).unwrap_err();
        assert!(err.starts_with("file name pattern: "), "{err}");
        assert!(err.contains("output folder"), "{err}");
        s.naming_pattern = String::new();
        assert!(c.apply(s.clone()).unwrap_err().contains("empty"));
        s.naming_pattern = " {channel}/{show} {date} ".into();
        c.apply(s.clone()).unwrap();
        assert_eq!(c.naming_pattern, "{channel}/{show} {date}");
        assert_eq!(c.pattern(), "{channel}/{show} {date}");
        // A preset leaves the custom pattern on file for later.
        s.naming = Naming::ByDate;
        c.apply(s.clone()).unwrap();
        assert_eq!(c.pattern(), Naming::ByDate.preset().unwrap());
        assert_eq!(c.settings().naming_pattern, "{channel}/{show} {date}");
        // But not a broken one, even then.
        s.naming_pattern = "{nope}".into();
        assert!(c.apply(s).is_err());
        // A broken one edited into the file by hand is not followed.
        let d = scratch("pat");
        let p = d.join("server.json");
        std::fs::write(&p, r#"{"naming": "custom", "namingPattern": "/etc/{show}"}"#).unwrap();
        let c = load_from(&p);
        assert_eq!((c.naming, c.pattern()), (Naming::Custom, naming::MEDIA_SERVER));
        std::fs::write(&p, r#"{"naming": "flat"}"#).unwrap();
        assert_eq!(load_from(&p).pattern(), Naming::Flat.preset().unwrap());
    }

    #[test]
    fn the_token_is_private_and_compared_whole() {
        let d = scratch("tok");
        let p = d.join("server-token");
        write_token(&p, "abc123").unwrap();
        assert_eq!(read_token_from(&p).as_deref(), Some("abc123"));
        write_token(&p, "def456").unwrap();
        assert_eq!(read_token_from(&p).as_deref(), Some("def456"));
        assert!(!d.join("server-token.tmp").exists());
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
