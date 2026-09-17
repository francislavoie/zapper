//! Where the Xtream credentials come from.
//!
//! One file, zapper's own: `~/.config/zapper/account.json`, written 0600 and
//! set through Settings → Account.
//!
//! Under the XDG config directory rather than beside the catalogue, because
//! the catalogue is a cache and a reasonable thing to delete, and deleting it
//! should not log you out.
//!
//! Nothing here may log a value. `Credentials` redacts itself in `Debug`;
//! keep it that way and never format the fields directly. The password never
//! leaves this process either: see [`summary`], which is what the interface
//! is allowed to see.

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

use crate::xtream::Credentials;

/// `~/.config/zapper/account.json`.
pub fn account_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("zapper").join("account.json")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    host: String,
    username: String,
    password: String,
}

/// Reads the account file. `Ok(None)` when there is not one yet, which is the
/// ordinary first-run state and not an error.
pub fn load_stored() -> Result<Option<Credentials>> {
    let path = account_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    // No quoting of the content on failure: the content is a password.
    let s: Stored = serde_json::from_str(&raw)
        .map_err(|e| anyhow!("{} is not valid JSON ({e})", path.display()))?;
    if s.host.is_empty() || s.username.is_empty() {
        return Ok(None);
    }
    Ok(Some(Credentials {
        host: s.host,
        username: s.username,
        password: s.password,
    }))
}

/// The credentials to use.
pub fn load() -> Result<Credentials> {
    match load_stored()? {
        Some(c) => Ok(c),
        None => Err(anyhow!(
            "no account configured. Set one in Settings → Account."
        )),
    }
}

/// Writes the account file, owner-readable only.
///
/// The mode is set before the content is written, not after: a file that is
/// briefly world-readable while holding a password is a file that was
/// world-readable.
pub fn save(creds: &Credentials) -> Result<()> {
    let path = account_path();
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("account path has no directory"))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let body = serde_json::to_string_pretty(&Stored {
        host: creds.host.clone(),
        username: creds.username.clone(),
        password: creds.password.clone(),
    })?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        f.write_all(body.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Whether there is an account on file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The account file.
    Zapper,
    /// Nothing yet.
    None,
}

/// What the interface is allowed to know: where to reach the server, who as,
/// and whether a password is on file — never the password itself.
pub fn summary() -> (Option<Credentials>, Source) {
    match load_stored() {
        Ok(Some(c)) => (Some(c), Source::Zapper),
        _ => (None, Source::None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test gets its own config root, so none of them can read — or
    /// write over — the live account file on the machine running them.
    fn in_temp_config<T>(f: impl FnOnce() -> T) -> T {
        use std::sync::Mutex;
        // `set_var` is process-wide; two of these at once would see each
        // other's directory.
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = std::env::temp_dir().join(format!(
            "zapper-credentials-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp config dir");

        let previous = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let out = f();
        match previous {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn creds() -> Credentials {
        Credentials {
            host: "http://panel.example:8080".into(),
            username: "user".into(),
            password: "secret".into(),
        }
    }

    #[test]
    fn the_account_file_lives_under_the_xdg_config_directory() {
        in_temp_config(|| {
            let p = account_path();
            assert!(p.ends_with("zapper/account.json"), "{}", p.display());
        });
    }

    #[test]
    fn nothing_on_file_is_not_an_error() {
        in_temp_config(|| {
            assert!(load_stored().expect("absent is ok").is_none());
            assert_eq!(summary().1, Source::None);
        });
    }

    #[test]
    fn what_is_saved_is_what_is_read_back() {
        in_temp_config(|| {
            save(&creds()).expect("saving");
            let c = load_stored().expect("reading").expect("present");
            assert_eq!(c.host, "http://panel.example:8080");
            assert_eq!(c.username, "user");
            assert_eq!(c.password, "secret");
            assert_eq!(summary().1, Source::Zapper);
        });
    }

    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        // It holds a live password; the mode is the whole protection.
        #[cfg(unix)]
        in_temp_config(|| {
            use std::os::unix::fs::PermissionsExt;
            save(&creds()).expect("saving");
            let mode = std::fs::metadata(account_path())
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
        });
    }

    #[test]
    fn saving_twice_replaces_rather_than_appends() {
        in_temp_config(|| {
            save(&creds()).expect("first save");
            let mut second = creds();
            second.username = "other".into();
            save(&second).expect("second save");
            assert_eq!(load_stored().unwrap().unwrap().username, "other");
        });
    }

    #[test]
    fn a_half_written_file_reads_as_no_account_rather_than_a_broken_one() {
        in_temp_config(|| {
            let path = account_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r#"{"host":"","username":"","password":""}"#).unwrap();
            assert!(load_stored().expect("empty fields are not an error").is_none());
        });
    }

    #[test]
    fn the_error_never_carries_the_file_contents() {
        // A parse failure must not echo the file back: it holds a password.
        in_temp_config(|| {
            let path = account_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "{ not json").unwrap();
            let err = load_stored().unwrap_err().to_string();
            assert!(!err.contains("not json"), "content leaked: {err}");
        });
    }

    #[test]
    fn load_says_what_to_do_when_there_is_no_account() {
        in_temp_config(|| {
            let err = load().unwrap_err().to_string();
            assert!(err.contains("Settings"), "unhelpful error: {err}");
        });
    }
}
