//! Settings › Data's backups: a JSON file holding the settings, the category
//! rail's choices, the watch history behind the favourites and, only when
//! asked, the account.
//!
//! The format itself — its version, what each section may hold, what an
//! import changes — is the frontend's (`src/logic/backup.ts`), since most of
//! what it carries lives there. What is here is what must not be: the file
//! (its path comes from the desktop's own dialog, never from the page), and
//! the account, so its password never crosses into the page. On export the
//! account is added to the document after it arrives; on import it is taken
//! out before the document goes back, kept here, and written only when the
//! import is applied.

use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use crate::catalog::credentials::{self, Portable};
use crate::api::types::{BackupAccountDto, BackupFileDto};

/// What the file says it is. Mirrored by `BACKUP_FORMAT` in
/// `src/logic/backup.ts`.
pub const FORMAT: &str = "zapper-backup";

/// The account read from the last backup opened, until it is imported or
/// another file is opened.
static STAGED: Mutex<Option<Portable>> = Mutex::new(None);

/// A backup is a few hundred kilobytes; anything far beyond that is not one,
/// and is not read into memory to find out.
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Writes `document` to `path`, with the account added when
/// `include_account`. The name of the file written.
pub fn export(path: &Path, document: Value, include_account: bool) -> Result<String, String> {
    let account = if include_account {
        credentials::portable().map_err(|e| e.to_string())?
    } else {
        None
    };
    let body = assemble(document, account)?;
    write(path, &body, include_account).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(display_name(path))
}

/// Reads the backup at `path`: its document for the page, and its account
/// kept here until [`import_account`] (or the next file opened).
pub fn open(path: &Path) -> Result<BackupFileDto, String> {
    let text = read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let (document, account) = split(&text)?;
    let (account, account_error) = match account.map(credentials::parse_portable) {
        None => (None, None),
        Some(Ok(p)) => (Some(p), None),
        Some(Err(e)) => (None, Some(e.to_string())),
    };
    let summary = account.as_ref().map(|p| BackupAccountDto {
        kind: p.kind().as_str().to_owned(),
        description: p.describe(),
        has_secret: p.has_secret(),
    });
    *STAGED.lock().unwrap_or_else(|e| e.into_inner()) = account;
    Ok(BackupFileDto {
        name: display_name(path),
        document,
        account: summary,
        account_error,
    })
}

/// Writes the account from the backup opened last, replacing the one on
/// file. The caller then drops what it built on the old one and asks for a
/// sync (`player::api::import_backup_account`).
pub fn import_account() -> Result<(), String> {
    let staged = STAGED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or("no account to import: open the backup again")?;
    credentials::save_portable(&staged).map_err(|e| e.to_string())
}

/// The document to write: the frontend's, with the account in place of any
/// the frontend might have put there.
fn assemble(document: Value, account: Option<Value>) -> Result<String, String> {
    let Value::Object(mut map) = document else {
        return Err("a backup must be a JSON object".into());
    };
    if map.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err("not a zapper backup document".into());
    }
    map.remove("account");
    if let Some(a) = account {
        map.insert("account".into(), a);
    }
    serde_json::to_string_pretty(&Value::Object(map)).map_err(|e| e.to_string())
}

/// A file's document and its account, apart. Refuses anything that does not
/// say it is a zapper backup, so an unrelated JSON file cannot stage an
/// account; the rest of the checking is the frontend's.
fn split(text: &str) -> Result<(Value, Option<Value>), String> {
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("this file is not JSON ({e})"))?;
    let Value::Object(mut map) = value else {
        return Err("this file is not a zapper backup".into());
    };
    if map.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err("this file is not a zapper backup".into());
    }
    let account = map.remove("account").filter(|a| !a.is_null());
    Ok((Value::Object(map), account))
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn read(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    let mut text = String::new();
    f.take(MAX_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::other("too large to be a zapper backup"));
    }
    Ok(text)
}

/// A file with a password in it is made owner-readable before anything is
/// written to it, like the account file itself.
fn write(path: &Path, body: &str, secret: bool) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = secret;
    let mut f = opts.open(path)?;
    // `mode` only applies to a file it creates; one being replaced keeps
    // whatever it had, so it is set explicitly too.
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(body.as_bytes())?;
    f.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_account_goes_in_only_from_here() {
        let doc = json!({"format": FORMAT, "version": 1, "account": {"password": "from the webview"}});
        let without: Value = serde_json::from_str(&assemble(doc.clone(), None).unwrap()).unwrap();
        assert!(without.get("account").is_none());
        let with: Value =
            serde_json::from_str(&assemble(doc, Some(json!({"host": "h"}))).unwrap()).unwrap();
        assert_eq!(with["account"], json!({"host": "h"}));
        assert_eq!(with["version"], 1);
    }

    #[test]
    fn only_a_backup_document_is_written() {
        assert!(assemble(json!([1, 2]), None).is_err());
        assert!(assemble(json!({"version": 1}), None).is_err());
    }

    #[test]
    fn an_opened_backup_hands_back_everything_but_its_account() {
        let text = format!(
            "\u{feff}{}",
            json!({"format": FORMAT, "version": 1, "settings": {}, "account": {"password": "pw"}})
        );
        let (doc, account) = split(&text).unwrap();
        assert!(doc.get("account").is_none());
        assert_eq!(doc["version"], 1);
        assert_eq!(account, Some(json!({"password": "pw"})));
        let (_, none) = split(&json!({"format": FORMAT, "account": null}).to_string()).unwrap();
        assert_eq!(none, None);
    }

    #[test]
    fn a_file_that_is_not_a_backup_is_refused_before_anything_is_staged() {
        for text in ["", "{", "[]", "42", r#"{"host":"h","password":"p"}"#, r#"{"format":"other"}"#] {
            assert!(split(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_file_with_a_password_is_owner_readable() {
        let dir = std::env::temp_dir().join(format!("zapper-backup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.json");
        std::fs::write(&path, "old").unwrap();
        write(&path, "{}", true).unwrap();
        assert_eq!(read(&path).unwrap(), "{}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
