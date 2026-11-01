//! The account in use, whichever kind it is, and how to play from it.
//!
//! zapper has two ways to learn a catalogue: asking an Xtream panel
//! (`xtream`), or reading an M3U playlist and an XMLTV guide (`m3u`). They
//! differ in nearly everything — a panel builds stream URLs from the account,
//! a playlist states each one; a panel's channels have ids, a playlist's are
//! derived; a panel has one catch-up scheme, a playlist several — so this is
//! an enum rather than a trait. Each caller matches on it where the kinds
//! genuinely behave differently, and the panel's arm is always the code that
//! existed before playlists did, unchanged.
//!
//! What is common is here: building one from the account file, and turning a
//! channel into something mpv can open ([`Playable`]). The sync passes live
//! in `catalog::sync_engine`, which owns the network schedule.

use anyhow::{anyhow, Context, Result};

use crate::catalog::credentials::{self, Account, Kind};
use crate::catalog::{db, playlist};
use crate::m3u::{catchup, fetch, M3uAccount};
use crate::xtream::XtreamClient;

/// A URL and the HTTP headers it must be opened with.
///
/// The headers are `None` for every Xtream channel and for most playlist
/// ones; only entries carrying `#EXTVLCOPT`/`#EXTHTTP`, or an account with
/// its own user agent, set them. `None` means mpv's default, which is the
/// same VLC agent the Xtream client sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playable {
    pub url: String,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
}

impl Playable {
    pub fn plain(url: String) -> Self {
        Self {
            url,
            user_agent: None,
            referrer: None,
        }
    }
}

pub enum Source {
    Xtream(XtreamClient),
    M3u(M3uSource),
}

impl Source {
    pub fn from_account(account: Account) -> Result<Self> {
        Ok(match account {
            Account::Xtream(c) => Source::Xtream(XtreamClient::new(c)?),
            Account::M3u(a) => Source::M3u(M3uSource::new(a)?),
        })
    }

    /// From the account file.
    pub fn load() -> Result<Self> {
        Self::from_account(credentials::load_account()?)
    }

    pub fn kind(&self) -> Kind {
        match self {
            Source::Xtream(_) => Kind::Xtream,
            Source::M3u(_) => Kind::M3u,
        }
    }

    /// The live stream for a channel.
    pub fn live(&self, stream_id: i64) -> Result<Playable> {
        match self {
            Source::Xtream(c) => Ok(Playable::plain(c.live_url(stream_id))),
            Source::M3u(m) => m.live(stream_id),
        }
    }
}

pub struct M3uSource {
    account: M3uAccount,
    http: reqwest::Client,
}

impl M3uSource {
    pub fn new(account: M3uAccount) -> Result<Self> {
        let http = fetch::http_client(account.user_agent.as_deref())?;
        Ok(Self { account, http })
    }

    pub fn account(&self) -> &M3uAccount {
        &self.account
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    fn row(&self, stream_id: i64) -> Result<playlist::StreamRow> {
        db::with_db(|c| playlist::stream_row(c, stream_id))?
            .ok_or_else(|| anyhow!("that channel is not in the playlist any more; try refreshing"))
    }

    fn playable(&self, row: &playlist::StreamRow, url: String) -> Playable {
        Playable {
            url,
            // The entry's own agent is more specific than the account's.
            user_agent: row
                .user_agent
                .clone()
                .or_else(|| self.account.user_agent.clone()),
            referrer: row.referrer.clone(),
        }
    }

    pub fn live(&self, stream_id: i64) -> Result<Playable> {
        let row = self.row(stream_id)?;
        let url = row.url.clone();
        Ok(self.playable(&row, url))
    }

    /// The replay of `[start, start + duration_minutes)`.
    ///
    /// `local_offset` is seconds east of UTC at `start`, for templates that
    /// want wall-clock fields; the caller has the platform's clock and this
    /// module stays pure enough to test.
    pub fn catchup(
        &self,
        stream_id: i64,
        start: i64,
        duration_minutes: u32,
        now: i64,
        local_offset: i64,
    ) -> Result<Playable> {
        let row = self.row(stream_id)?;
        let mode = row
            .catchup_mode
            .as_deref()
            .and_then(catchup::Mode::parse)
            .ok_or_else(|| anyhow!("this channel's playlist entry offers no catch-up"))?;
        let window = catchup::Window {
            start,
            duration_secs: i64::from(duration_minutes) * 60,
            now,
            local_offset,
        };
        let url = catchup::url(mode, row.catchup_source.as_deref(), &row.url, &window)
            .ok_or_else(|| anyhow!("could not build a catch-up address for this channel"))?;
        Ok(self.playable(&row, url))
    }

    /// The guide locations to read: the account's, or failing that the ones
    /// the playlist names for itself.
    pub fn guide_locations(&self) -> Vec<String> {
        if !self.account.epg_urls.is_empty() {
            return self.account.epg_urls.clone();
        }
        db::with_db(playlist::header_epg_urls).unwrap_or_default()
    }

    /// The connection test: fetches the start of the playlist and parses it,
    /// then checks each guide answers with something that looks like XMLTV.
    ///
    /// Only the first few hundred lines are read. A full playlist is tens of
    /// megabytes, and the question being answered is "is this a playlist I
    /// can reach", not "how many channels are in it" — the sync answers that.
    pub async fn probe(&self) -> Result<String> {
        const LINES: usize = 600;
        const MAX_BYTES: usize = 2 * 1024 * 1024;

        let (head, size, content_type) = read_prefix(
            &self.http,
            &self.account.playlist,
            "the playlist",
            MAX_BYTES,
        )
        .await?;
        if fetch::looks_like_stream(&head, content_type.as_deref()) {
            return Ok(
                "That is a single stream rather than a playlist; it will be listed as one channel."
                    .to_owned(),
            );
        }
        let (count, header, truncated) = tokio::task::spawn_blocking(move || {
            let r = fetch::decompressing(std::io::Cursor::new(head))?;
            crate::m3u::parse::sample(r, LINES)
        })
        .await
        .context("joining the playlist check")?
        .context("reading the playlist")?;
        if count == 0 {
            return Err(anyhow!(
                "no channels in the first {LINES} lines; is this an M3U playlist?"
            ));
        }
        let mut msg = if truncated {
            match size {
                Some(b) => format!(
                    "Playlist OK: {count} channels in the first {LINES} lines of {}",
                    human_size(b)
                ),
                None => format!("Playlist OK: {count} channels in the first {LINES} lines"),
            }
        } else {
            format!("Playlist OK: {count} channels")
        };

        let guides = if self.account.epg_urls.is_empty() {
            header.epg_urls
        } else {
            self.account.epg_urls.clone()
        };
        if guides.is_empty() {
            msg.push_str(" · no guide configured");
            return Ok(msg);
        }
        let mut ok = 0;
        let mut first_err = None;
        for g in &guides {
            match self.probe_guide(g).await {
                Ok(()) => ok += 1,
                Err(e) => {
                    first_err.get_or_insert(format!("{e:#}"));
                }
            }
        }
        match first_err {
            None if guides.len() == 1 => msg.push_str(" · guide OK"),
            None => msg.push_str(&format!(" · {ok} guides OK")),
            Some(e) => msg.push_str(&format!(" · {ok} of {} guides OK ({e})", guides.len())),
        }
        Ok(msg)
    }

    async fn probe_guide(&self, location: &str) -> Result<()> {
        let (head, _, _) = read_prefix(&self.http, location, "the guide", 64 * 1024).await?;
        let text = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
            use std::io::Read;
            let mut r = fetch::decompressing(std::io::Cursor::new(head))?;
            let mut buf = vec![0u8; 4096];
            let mut n = 0;
            // A truncated gzip stream errors at the cut; what came before it
            // is all this needs.
            while n < buf.len() {
                match r.read(&mut buf[n..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => n += k,
                }
            }
            Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
        })
        .await
        .context("joining the guide check")??;
        if text.contains("<tv") || text.contains("<?xml") {
            Ok(())
        } else {
            Err(anyhow!("the guide is not XMLTV"))
        }
    }
}

fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{} kB", bytes / 1024)
    }
}

/// Up to `max` bytes from the start of a location, its total size if known,
/// and its `Content-Type` if it has one. The rest is never downloaded.
async fn read_prefix(
    http: &reqwest::Client,
    location: &str,
    what: &str,
    max: usize,
) -> Result<(Vec<u8>, Option<u64>, Option<String>)> {
    match fetch::open(http, location, what).await? {
        fetch::Opened::Http(mut resp) => {
            let size = resp.content_length();
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let mut head = Vec::new();
            while head.len() < max {
                match fetch::next_chunk(&mut resp, what).await? {
                    Some(c) => {
                        head.extend_from_slice(&c);
                        // A stream sends forever; the first chunk is enough to
                        // know that it is one.
                        if fetch::looks_like_stream(&head, ct.as_deref()) {
                            break;
                        }
                    }
                    None => break,
                }
            }
            Ok((head, size, ct))
        }
        fetch::Opened::File(f) => tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let size = f.metadata().ok().map(|m| m.len());
            let mut head = Vec::new();
            f.take(max as u64).read_to_end(&mut head)?;
            Ok((head, size, None))
        })
        .await
        .context("joining a file read")?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_people_say_them() {
        assert_eq!(human_size(24 * 1024 * 1024), "24 MB");
        assert_eq!(human_size(300 * 1024), "300 kB");
    }

    fn temp_file(name: &str, body: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zapper-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[tokio::test]
    async fn the_probe_reads_a_local_playlist_and_reports_its_channels() {
        let p = temp_file("probe.m3u", include_bytes!("../tests/fixtures/sample.m3u"));
        let src = M3uSource::new(M3uAccount {
            playlist: p.display().to_string(),
            // A local guide too, gzipped, to exercise the sniffing path.
            epg_urls: vec![{
                use std::io::Write;
                let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                e.write_all(b"<?xml version=\"1.0\"?><tv></tv>").unwrap();
                temp_file("probe.xml", &e.finish().unwrap())
                    .display()
                    .to_string()
            }],
            user_agent: None,
        })
        .unwrap();
        let msg = src.probe().await.unwrap();
        assert_eq!(msg, "Playlist OK: 8 channels · guide OK");
    }

    #[tokio::test]
    async fn the_probe_says_when_a_file_is_not_a_playlist() {
        let p = temp_file("not.m3u", b"<html>hello</html>\n");
        let src = M3uSource::new(M3uAccount {
            playlist: p.display().to_string(),
            ..M3uAccount::default()
        })
        .unwrap();
        let err = src.probe().await.unwrap_err().to_string();
        assert!(err.contains("no channels"), "{err}");
    }

    #[tokio::test]
    async fn a_missing_file_is_a_plain_error() {
        let src = M3uSource::new(M3uAccount {
            playlist: "/nonexistent/zapper/nothing.m3u".into(),
            ..M3uAccount::default()
        })
        .unwrap();
        let err = format!("{:#}", src.probe().await.unwrap_err());
        assert!(err.contains("opening the playlist"), "{err}");
    }
}
