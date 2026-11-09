//! The online services' settings: `online-services.json` in the config
//! directory, beside `account.json` and kept the same way (readable by this
//! user alone, replaced whole), because it holds the OpenSubtitles sign-in.
//! The password stays beside the token so an expired token is renewed
//! without asking again.
//!
//! Nothing here may print a credential: [`OpenSubtitlesConfig`]'s `Debug`
//! says only whether there is a sign-in, and [`status`] is what the
//! interface sees.

use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::api::types::{OnlineServicesDto, OpenSubtitlesStatusDto};

pub const FILE: &str = "online-services.json";

/// One section per service; TMDB's and TheIntroDB's join it beside
/// `opensubtitles`.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OnlineConfig {
    pub opensubtitles: OpenSubtitlesConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenSubtitlesConfig {
    /// On unless turned off: off, nothing is sent to OpenSubtitles.
    pub enabled: bool,
    /// What to search for, as language codes; empty means the interface's
    /// own preference.
    pub languages: Vec<String>,
    pub username: String,
    pub password: String,
    pub token: Option<String>,
    /// When the token was had, Unix seconds.
    pub token_at: Option<i64>,
    /// The API base the sign-in named (VIP accounts have their own).
    pub api_base: Option<String>,
    /// OpenSubtitles refused the username and password kept here.
    pub refused: bool,
}

impl Default for OpenSubtitlesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            languages: Vec::new(),
            username: String::new(),
            password: String::new(),
            token: None,
            token_at: None,
            api_base: None,
            refused: false,
        }
    }
}

impl OpenSubtitlesConfig {
    /// There is a sign-in: a username, with a token or the password to get
    /// one.
    pub fn signed_in(&self) -> bool {
        !self.username.is_empty() && (self.token.is_some() || !self.password.is_empty())
    }
}

impl std::fmt::Debug for OpenSubtitlesConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenSubtitlesConfig")
            .field("enabled", &self.enabled)
            .field("languages", &self.languages)
            .field("signed_in", &self.signed_in())
            .field("refused", &self.refused)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for OnlineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnlineConfig")
            .field("opensubtitles", &self.opensubtitles)
            .finish()
    }
}

/// One change at a time, so two don't each write what they read.
static WRITING: Mutex<()> = Mutex::new(());

/// The settings, or the defaults when there are none or they can't be read.
pub fn load() -> OnlineConfig {
    load_from(&crate::paths::config_dir().join(FILE))
}

pub fn load_from(path: &Path) -> OnlineConfig {
    read_from(path).unwrap_or_else(|e| {
        tracing::warn!("{e}; using the defaults");
        OnlineConfig::default()
    })
}

/// The settings at `path`: the defaults when there is no file or it is not
/// valid settings; an error when the file is there but could not be read at
/// all (it is then not to be written over).
fn read_from(path: &Path) -> anyhow::Result<OnlineConfig> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(OnlineConfig::default()),
        Err(e) => anyhow::bail!("{} could not be read ({:?})", path.display(), e.kind()),
    };
    // serde's message can quote the file; its kind and place cannot.
    Ok(serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!(
            "{} could not be read ({:?} at line {}, column {}); using the defaults",
            path.display(),
            e.classify(),
            e.line(),
            e.column()
        );
        OnlineConfig::default()
    }))
}

/// Reads the settings, lets `f` change them, and writes them back whole,
/// readable by this user alone. A file that is there but cannot be read is
/// left as it is (it may hold a sign-in) and the change refused; one that
/// is not valid settings is replaced.
pub fn update<R>(f: impl FnOnce(&mut OnlineConfig) -> R) -> anyhow::Result<(OnlineConfig, R)> {
    update_in(&crate::paths::config_dir().join(FILE), f)
}

pub fn update_in<R>(
    path: &Path,
    f: impl FnOnce(&mut OnlineConfig) -> R,
) -> anyhow::Result<(OnlineConfig, R)> {
    let _one_at_a_time = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut c = read_from(path)?;
    let r = f(&mut c);
    let text = serde_json::to_vec_pretty(&c)?;
    crate::recording::config::write_private(path, &text)?;
    Ok((c, r))
}

/// What Settings › Online services shows; never a password or a token.
/// `quota` is the downloads left and when they reset (Unix seconds), when
/// known.
pub fn status(c: &OnlineConfig, quota: Option<(u32, Option<i64>)>) -> OnlineServicesDto {
    let os = &c.opensubtitles;
    OnlineServicesDto {
        open_subtitles: OpenSubtitlesStatusDto {
            enabled: os.enabled,
            languages: os.languages.clone(),
            signed_in_as: os.signed_in().then(|| os.username.clone()),
            sign_in_refused: os.refused,
            downloads_left: quota.map(|(left, _)| left),
            downloads_reset_at: quota.and_then(|(_, at)| at),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zapper-online-config-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE)
    }

    fn signed_in(c: &mut OnlineConfig) {
        c.opensubtitles.username = "fake-os-user".into();
        c.opensubtitles.password = "fake-os-pass".into();
        c.opensubtitles.token = Some("fake-os-token-1".into());
        c.opensubtitles.token_at = Some(1_794_139_200);
    }

    #[test]
    fn a_missing_or_broken_file_reads_as_the_defaults() {
        let path = scratch("missing");
        let c = load_from(&path);
        assert!(c.opensubtitles.enabled, "on unless turned off");
        assert!(c.opensubtitles.languages.is_empty());
        assert!(c.opensubtitles.token.is_none());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"opensubtitles\": {\"enabled\": \"fake-os-pass\"").unwrap();
        assert!(load_from(&path).opensubtitles.enabled);
        // An older file without some fields keeps the defaults for them.
        std::fs::write(&path, r#"{"opensubtitles":{"languages":["fr"]}}"#).unwrap();
        let c = load_from(&path);
        assert!(c.opensubtitles.enabled);
        assert_eq!(c.opensubtitles.languages, ["fr"]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn an_unreadable_file_is_not_written_over_but_a_broken_one_is() {
        // There but unreadable: a directory where the file should be stands
        // for a permission or I/O failure.
        let path = scratch("unreadable");
        std::fs::create_dir_all(&path).unwrap();
        assert!(update_in(&path, |c| c.opensubtitles.enabled = false).is_err());
        assert!(path.is_dir(), "left as it was");
        assert!(
            load_from(&path).opensubtitles.enabled,
            "read as the defaults"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());

        // There and readable, but not valid settings: replaced.
        let path = scratch("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let (c, ()) = update_in(&path, |c| c.opensubtitles.enabled = false).unwrap();
        assert!(!c.opensubtitles.enabled);
        assert!(!load_from(&path).opensubtitles.enabled, "written");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_settings_file_is_private_and_debug_is_redacted() {
        let path = scratch("private");
        let (c, r) = update_in(&path, |c| {
            signed_in(c);
            c.opensubtitles.languages = vec!["en".into(), "fr".into()];
            7
        })
        .unwrap();
        assert_eq!(r, 7);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let back = load_from(&path);
        assert_eq!(back.opensubtitles.token.as_deref(), Some("fake-os-token-1"));
        assert_eq!(back.opensubtitles.password, "fake-os-pass");

        // A second change keeps the first.
        let (c2, ()) = update_in(&path, |c| c.opensubtitles.enabled = false).unwrap();
        assert!(!c2.opensubtitles.enabled);
        assert_eq!(c2.opensubtitles.languages, ["en", "fr"]);

        for printed in [
            format!("{c:?}"),
            format!("{:?}", c.opensubtitles),
            format!("{c2:#?}"),
        ] {
            for secret in ["fake-os-user", "fake-os-pass", "fake-os-token-1"] {
                assert!(!printed.contains(secret), "Debug printed a credential");
            }
            assert!(printed.contains("signed_in: true"), "{printed}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_status_never_carries_the_token_or_password() {
        let mut c = OnlineConfig::default();
        let s = status(&c, None);
        assert!(s.open_subtitles.enabled);
        assert_eq!(s.open_subtitles.signed_in_as, None);
        assert_eq!(s.open_subtitles.downloads_left, None);

        signed_in(&mut c);
        c.opensubtitles.refused = true;
        let s = status(&c, Some((4, Some(1_794_139_200))));
        assert_eq!(
            s.open_subtitles.signed_in_as.as_deref(),
            Some("fake-os-user")
        );
        assert!(s.open_subtitles.sign_in_refused);
        assert_eq!(s.open_subtitles.downloads_left, Some(4));
        assert_eq!(s.open_subtitles.downloads_reset_at, Some(1_794_139_200));
        let wire = serde_json::to_string(&s).unwrap();
        assert!(wire.contains("\"openSubtitles\""), "{wire}");
        assert!(!wire.contains("fake-os-pass") && !wire.contains("fake-os-token-1"));
    }
}
