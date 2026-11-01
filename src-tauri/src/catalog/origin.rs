//! Where the mirror's catalogue came from, so a different account replaces
//! it rather than leaving the last one's leftovers behind.
//!
//! An Xtream server's sync upserts categories and never deletes one, which is
//! right for a server that renames or reorders its own: deleting would
//! make the rail flicker on every refresh. But it means that after switching
//! from a playlist back to an Xtream server, every playlist group would sit in
//! the rail with nothing in it — the playlist's channels were soft-deleted by
//! the server's channel sync, its categories were not. The same goes for
//! switching from one server to another.
//!
//! So each pass knows which catalogue it is filling — a short hash of the
//! server's host and username, or of the playlist's location with its tokens
//! stripped — and the one that last filled the mirror is kept in `meta`. When
//! they differ, the server's categories phase also drops every category the
//! new server did not list. (A playlist's write replaces its categories
//! wholesale every time anyway; see [`crate::catalog::playlist`].) Channels
//! need nothing extra: both kinds already soft-delete what they did not see,
//! and favourites and history follow the existing rule for vanished channels
//! — kept, but not shown, until the channel comes back
//! ([`crate::catalog::store::watched`]).
//!
//! Hashes, never the values: the database is not owner-only the way the
//! account file is, and the origin needs only to be compared.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::m3u::ids::{fnv1a, stable_url};
use crate::xtream::Credentials;

const KEY: &str = "catalogue_origin";

/// The origin on record, as an SQL expression: for statements that key rows
/// on the catalogue (`channel_quality`) without a round trip to read it
/// first. The empty string when none is on record, which is a key like any
/// other — a mirror that has never synced has nothing to play anyway.
///
/// A macro so that it can be `concat!`ed into other constant SQL.
macro_rules! current_origin_sql {
    () => {
        "COALESCE((SELECT value FROM meta WHERE key = 'catalogue_origin'), '')"
    };
}
pub(crate) use current_origin_sql;

/// An Xtream server is its address and account. Not the password: changing it
/// is the same catalogue.
pub fn of_xtream(creds: &Credentials) -> String {
    let host = creds.host.trim().trim_end_matches('/').to_ascii_lowercase();
    let id = format!("{host}\u{0}{}", creds.username.trim());
    format!("xtream:{:016x}", fnv1a(id.as_bytes()))
}

/// A playlist is its location, with whatever rotates taken out.
pub fn of_playlist(location: &str) -> String {
    format!("m3u:{:016x}", fnv1a(stable_url(location).as_bytes()))
}

/// A tuner is its `DeviceID`, not its address: DHCP moving it to another
/// address is the same lineup.
pub fn of_hdhomerun(device_id: &str) -> String {
    let id = device_id.trim().to_ascii_uppercase();
    format!("hdhr:{:016x}", fnv1a(id.as_bytes()))
}

/// A Tvheadend server is its address and login, like an Xtream server: a
/// different user may be allowed different channels.
pub fn of_tvheadend(url: &str, username: &str) -> String {
    let host = url.trim().trim_end_matches('/').to_ascii_lowercase();
    let id = format!("{host}\u{0}{}", username.trim());
    format!("tvh:{:016x}", fnv1a(id.as_bytes()))
}

pub fn recorded(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [KEY], |r| r.get(0))
        .optional()?)
}

/// Notes that `origin` filled the mirror. Writes only on a change.
pub fn record(conn: &Connection, origin: &str) -> Result<()> {
    if recorded(conn)?.as_deref() == Some(origin) {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![KEY, origin],
    )?;
    Ok(())
}

/// Whether the mirror holds some other catalogue than `origin`'s.
///
/// A mirror with no origin on record predates this, or is empty. It counts
/// as a change only if it visibly holds a playlist's categories — which an
/// Xtream account cannot have put there — so an Xtream user upgrading sees
/// nothing different.
pub fn changed(conn: &Connection, origin: &str) -> Result<bool> {
    Ok(match recorded(conn)? {
        Some(r) => r != origin,
        None => conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM categories WHERE id LIKE 'm3u-%')",
            [],
            |r| r.get(0),
        )?,
    })
}

/// Drops every category not in `keep`. For an Xtream server's first pass after
/// the catalogue changed hands.
pub fn prune_categories(conn: &Connection, keep: &[&str]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let existing: Vec<String> = tx
        .prepare("SELECT id FROM categories")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut dropped = 0;
    for id in existing {
        if !keep.contains(&id.as_str()) {
            tx.execute("DELETE FROM categories WHERE id = ?1", [&id])?;
            dropped += 1;
        }
    }
    tx.commit()?;
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::open_in_memory;

    fn creds(host: &str, user: &str, pass: &str) -> Credentials {
        Credentials {
            host: host.into(),
            username: user.into(),
            password: pass.into(),
        }
    }

    #[test]
    fn origins_ignore_what_does_not_change_the_catalogue() {
        assert_eq!(
            of_xtream(&creds("http://P.example:8080/", "me", "old")),
            of_xtream(&creds("http://p.example:8080", "me", "new")),
        );
        assert_ne!(
            of_xtream(&creds("http://p.example", "me", "x")),
            of_xtream(&creds("http://p.example", "you", "x")),
        );
        assert_eq!(
            of_playlist("http://p.example/get.php?username=u&password=a&type=m3u"),
            of_playlist("http://p.example/get.php?username=u&password=b&type=m3u"),
        );
        assert_ne!(of_playlist("/home/me/a.m3u"), of_playlist("/home/me/b.m3u"));
        assert_eq!(of_hdhomerun("10abcdef"), of_hdhomerun(" 10ABCDEF"));
        assert_ne!(of_hdhomerun("10ABCDEF"), of_hdhomerun("20ABCDEF"));
        assert_eq!(
            of_tvheadend("http://TVH.lan:9981/", "me"),
            of_tvheadend("http://tvh.lan:9981", "me")
        );
        assert_ne!(of_tvheadend("http://tvh.lan:9981", "me"), of_tvheadend("http://tvh.lan:9981", ""));
    }

    #[test]
    fn origins_never_carry_the_account() {
        let o = of_xtream(&creds("http://p.example", "bob", "hunter2"));
        assert!(!o.contains("bob") && !o.contains("p.example"), "{o}");
    }

    #[test]
    fn an_unknown_origin_is_a_change_only_with_playlist_rows_present() {
        let c = open_in_memory().unwrap();
        assert!(!changed(&c, "xtream:1").unwrap());
        c.execute(
            "INSERT INTO categories (id, name) VALUES ('m3u-ab', 'Old group')",
            [],
        )
        .unwrap();
        assert!(changed(&c, "xtream:1").unwrap());
        record(&c, "xtream:1").unwrap();
        assert!(!changed(&c, "xtream:1").unwrap());
        assert!(changed(&c, "m3u:2").unwrap());
    }

    #[test]
    fn the_sql_expression_reads_what_was_recorded() {
        let c = open_in_memory().unwrap();
        let read = |c: &Connection| -> String {
            c.query_row(concat!("SELECT ", current_origin_sql!()), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(read(&c), "");
        record(&c, "xtream:abc").unwrap();
        assert_eq!(read(&c), "xtream:abc");
        assert!(current_origin_sql!().contains(KEY));
    }

    #[test]
    fn pruning_keeps_exactly_what_was_listed() {
        let c = open_in_memory().unwrap();
        for id in ["1", "2", "m3u-x"] {
            c.execute("INSERT INTO categories (id, name) VALUES (?1, ?1)", [id])
                .unwrap();
        }
        assert_eq!(prune_categories(&c, &["1", "2"]).unwrap(), 1);
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }
}
