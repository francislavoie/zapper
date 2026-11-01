//! Where the Xtream credentials come from.
//!
//! One file, zapper's own: `account.json` in the config directory
//! (`~/.config/zapper` on Linux, `%APPDATA%\zapper` on Windows; see
//! [`crate::paths`]), written 0600 where that means something and set through
//! Settings → Account.
//!
//! Under the config directory rather than beside the catalogue, because the
//! catalogue is a cache and a reasonable thing to delete, and deleting it
//! should not log you out.
//!
//! Nothing here may log a value. `Credentials` redacts itself in `Debug`;
//! keep it that way and never format the fields directly. The password never
//! leaves this process either: see [`summary`], which is what the interface
//! is allowed to see.

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

use crate::m3u::M3uAccount;
use crate::xtream::Credentials;

/// `~/.config/zapper/account.json`, or the platform's equivalent.
pub fn account_path() -> PathBuf {
    crate::paths::config_dir().join("account.json")
}

/// The file's shape.
///
/// Both kinds' fields live side by side and `kind` says which is in use, so
/// switching to a playlist and back does not cost the Xtream password (and
/// vice versa). Every field defaults, which is what keeps a file written
/// before playlists existed — `{"host","username","password"}` and nothing
/// else — loading exactly as it did: no `kind` means Xtream.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Stored {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default)]
    host: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    playlist: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    epg_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_agent: Option<String>,
}

impl Stored {
    fn xtream(&self) -> Option<Credentials> {
        if self.host.is_empty() || self.username.is_empty() {
            return None;
        }
        Some(Credentials {
            host: self.host.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
        })
    }

    fn m3u(&self) -> Option<M3uAccount> {
        if self.playlist.trim().is_empty() {
            return None;
        }
        Some(M3uAccount {
            playlist: self.playlist.clone(),
            epg_urls: self.epg_urls.clone(),
            user_agent: self.user_agent.clone(),
        })
    }

    fn kind(&self) -> Result<Kind> {
        match self.kind.as_deref().map(str::trim) {
            None | Some("") | Some("xtream") => Ok(Kind::Xtream),
            Some("m3u") => Ok(Kind::M3u),
            Some(other) => Err(anyhow!(
                "{} names an account kind this version does not know ({other})",
                account_path().display()
            )),
        }
    }
}

/// Which kind of account is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Xtream,
    M3u,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Xtream => "xtream",
            Kind::M3u => "m3u",
        }
    }
}

/// The account in use, whichever kind it is.
#[derive(Debug, Clone)]
pub enum Account {
    Xtream(Credentials),
    M3u(M3uAccount),
}

fn read_file() -> Result<Option<Stored>> {
    let path = account_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("reading {}: {e}", path.display())),
    };
    // No quoting of the content on failure: the content is a password.
    let s: Stored = serde_json::from_str(&raw)
        .map_err(|e| anyhow!("{} is not valid JSON ({e})", path.display()))?;
    Ok(Some(s))
}

/// Reads the account file. `Ok(None)` when there is not one yet, which is the
/// ordinary first-run state and not an error — and also when the account in
/// use is a playlist, since there are then no panel credentials to use.
pub fn load_stored() -> Result<Option<Credentials>> {
    Ok(match load_account_stored()? {
        Some(Account::Xtream(c)) => Some(c),
        _ => None,
    })
}

/// The account in use, of either kind. `Ok(None)` when there is none yet.
pub fn load_account_stored() -> Result<Option<Account>> {
    let Some(s) = read_file()? else {
        return Ok(None);
    };
    Ok(match s.kind()? {
        Kind::Xtream => s.xtream().map(Account::Xtream),
        Kind::M3u => s.m3u().map(Account::M3u),
    })
}

/// The account to use, of either kind.
pub fn load_account() -> Result<Account> {
    load_account_stored()?.ok_or_else(|| {
        anyhow!("no account configured. Set one in Settings → Account.")
    })
}

/// The Xtream credentials to use.
pub fn load() -> Result<Credentials> {
    match load_account()? {
        Account::Xtream(c) => Ok(c),
        Account::M3u(_) => Err(anyhow!(
            "the account is an M3U playlist, not an Xtream panel"
        )),
    }
}

/// Everything on file, of both kinds, for the settings pane: which kind is in
/// use, and what each kind's fields hold so switching shows them filled in.
pub struct OnFile {
    pub kind: Kind,
    pub xtream: Option<Credentials>,
    pub m3u: Option<M3uAccount>,
}

pub fn on_file() -> OnFile {
    match read_file() {
        Ok(Some(s)) => OnFile {
            kind: s.kind().unwrap_or(Kind::Xtream),
            xtream: s.xtream(),
            m3u: s.m3u(),
        },
        _ => OnFile {
            kind: Kind::Xtream,
            xtream: None,
            m3u: None,
        },
    }
}

/// Writes the account file, owner-readable only.
///
/// The mode is set before the content is written, not after: a file that is
/// briefly world-readable while holding a password is a file that was
/// world-readable.
///
/// Makes the Xtream server the account in use, keeping any playlist fields on
/// file.
pub fn save(creds: &Credentials) -> Result<()> {
    let mut s = existing_or_default();
    s.kind = Some(Kind::Xtream.as_str().to_owned());
    s.host = creds.host.clone();
    s.username = creds.username.clone();
    s.password = creds.password.clone();
    write(&s)
}

/// Makes the playlist the account in use, keeping any Xtream fields on file.
pub fn save_m3u(acct: &M3uAccount) -> Result<()> {
    let mut s = existing_or_default();
    s.kind = Some(Kind::M3u.as_str().to_owned());
    s.playlist = acct.playlist.clone();
    s.epg_urls = acct.epg_urls.clone();
    s.user_agent = acct.user_agent.clone();
    write(&s)
}

/// What is on file, to merge into. A file that will not parse is replaced
/// rather than merged: the save is the user fixing it.
fn existing_or_default() -> Stored {
    read_file().ok().flatten().unwrap_or_default()
}

fn write(stored: &Stored) -> Result<()> {
    let path = account_path();
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("account path has no directory"))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let body = serde_json::to_string_pretty(stored)?;

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
///
/// The Xtream fields on file are returned even while a playlist is the
/// account in use: they are what the Xtream form shows when switched back to,
/// and what an empty password field keeps.
pub fn summary() -> (Option<Credentials>, Source) {
    let f = on_file();
    let source = if f.xtream.is_some() || f.m3u.is_some() {
        Source::Zapper
    } else {
        Source::None
    };
    (f.xtream, source)
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

        // The variable the config directory is read from on this platform,
        // so that `cargo test` on Windows cannot touch the real %APPDATA%.
        let var = if cfg!(windows) {
            "APPDATA"
        } else if cfg!(target_os = "macos") {
            "HOME"
        } else {
            "XDG_CONFIG_HOME"
        };
        let previous = std::env::var_os(var);
        std::env::set_var(var, &dir);
        let out = f();
        match previous {
            Some(p) => std::env::set_var(var, p),
            None => std::env::remove_var(var),
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
    fn the_account_file_lives_under_the_config_directory() {
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

    fn write_raw(body: &str) {
        let path = account_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }

    fn playlist() -> M3uAccount {
        M3uAccount {
            playlist: "http://p.example/get.php?username=u&password=pw&type=m3u_plus".into(),
            epg_urls: vec!["http://p.example/xmltv.php".into()],
            user_agent: Some("TiviMate/4".into()),
        }
    }

    #[test]
    fn a_file_from_before_playlists_still_loads_as_a_panel() {
        // Exactly what the first release wrote: no `kind`, no playlist fields.
        in_temp_config(|| {
            write_raw(r#"{"host":"http://old.example","username":"me","password":"pw"}"#);
            match load_account().expect("old file loads") {
                Account::Xtream(c) => {
                    assert_eq!((c.host.as_str(), c.username.as_str()), ("http://old.example", "me"));
                    assert_eq!(c.password, "pw");
                }
                Account::M3u(_) => panic!("an old file is a panel account"),
            }
            assert_eq!(load().unwrap().username, "me");
            assert_eq!(on_file().kind, Kind::Xtream);
        });
    }

    #[test]
    fn a_playlist_round_trips() {
        in_temp_config(|| {
            save_m3u(&playlist()).unwrap();
            match load_account().unwrap() {
                Account::M3u(a) => assert_eq!(a, playlist()),
                Account::Xtream(_) => panic!("saved a playlist, read a panel"),
            }
            // Xtream-only callers are told why there is no Xtream server.
            let err = load().unwrap_err().to_string();
            assert!(err.contains("M3U"), "{err}");
            assert!(load_stored().unwrap().is_none());
        });
    }

    #[test]
    fn switching_kinds_keeps_the_other_kinds_fields() {
        in_temp_config(|| {
            save(&creds()).unwrap();
            save_m3u(&playlist()).unwrap();
            let f = on_file();
            assert_eq!(f.kind, Kind::M3u);
            assert_eq!(f.xtream.as_ref().unwrap().password, "secret");
            // The pane still knows the Xtream server, so switching back is one
            // click.
            assert_eq!(summary().0.unwrap().username, "user");
            assert_eq!(summary().1, Source::Zapper);

            save(&creds()).unwrap();
            let f = on_file();
            assert_eq!(f.kind, Kind::Xtream);
            assert_eq!(f.m3u, Some(playlist()));
        });
    }

    #[test]
    fn an_unknown_kind_is_an_error_that_names_the_file_not_its_contents() {
        in_temp_config(|| {
            write_raw(r#"{"kind":"stalker","host":"h","username":"u","password":"hunter2"}"#);
            let err = load_account().unwrap_err().to_string();
            assert!(err.contains("stalker"), "{err}");
            assert!(!err.contains("hunter2"), "{err}");
        });
    }

    #[test]
    fn a_playlist_file_is_readable_only_by_its_owner() {
        #[cfg(unix)]
        in_temp_config(|| {
            use std::os::unix::fs::PermissionsExt;
            save_m3u(&playlist()).unwrap();
            let mode = std::fs::metadata(account_path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        });
    }
}
