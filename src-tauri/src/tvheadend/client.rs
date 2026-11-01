//! Requests to a Tvheadend server, logged in.
//!
//! Every request goes out bare first. A server that lets anyone in answers
//! it; one that wants a login answers `401` with its challenge, and the
//! request is sent again answering it — Digest if offered, Basic only if
//! that is all the server offered (see [`super::digest`]). A challenge is
//! never cached across requests: these are a handful per sync, on a LAN,
//! and a stale nonce is one more thing to get wrong.
//!
//! **Which playlist.** Tvheadend writes its playlist with a stream URL per
//! channel, and how those URLs authenticate is chosen by the path asked for:
//!
//! - `/playlist/channels` — no authentication in the URLs. Right for a
//!   server that lets anyone stream; anywhere else mpv gets `401`.
//! - `/playlist/ticket/channels` — `?ticket=` on each URL, a one-off token
//!   that expires after the server's ticket lifetime (five minutes by
//!   default, an hour at most). The catalogue is synced every few hours, so
//!   by the time anybody zaps, every ticket in it has expired.
//! - `/playlist/auth/channels` — `?auth=` on each URL: the user's
//!   *persistent authentication code*, which works until it is reset. It
//!   exists only when that user has "Persistent authentication" enabled
//!   (Configuration → Users → Passwords), and without it the server refuses
//!   this path outright.
//!
//! So a login means `/playlist/auth/`: the URLs it gives mpv keep working
//! for as long as the catalogue does, and zapper never has to authenticate
//! on mpv's behalf. When the server refuses it, the plain playlist is asked
//! for with the same login, purely to tell the two refusals apart — a wrong
//! password, or a right one without persistent authentication — so the
//! message can say which, and exactly what to switch on. Without a login,
//! `/playlist/channels`.

use anyhow::{anyhow, Context, Result};
use reqwest::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use std::collections::HashMap;
use std::time::Duration;

use super::digest::{self, Scheme};
use super::TvhAccount;
use crate::source::Refused;

/// What to switch on, word for word as Tvheadend's interface labels it.
pub const NEEDS_PERSISTENT_AUTH: &str = "Tvheadend accepted this login but will not give out \
    stream links that keep working. In Tvheadend, open Configuration → Users → Passwords, \
    edit this user, set “Persistent authentication” to Enable, and save; then sync again.";

pub const NEEDS_LOGIN: &str =
    "Tvheadend wants a username and password. Add them in Settings → Account.";

fn bad_login(status: u16) -> String {
    format!(
        "Tvheadend refused this username and password (HTTP {status}). Check them in Settings → Account."
    )
}

/// The server's root, from whatever was typed: `http://` added to a bare
/// host, a trailing slash taken off. A login typed into the address is
/// refused rather than used: it would sit in the account file and in every
/// error beside the host, when the form has fields for it.
pub fn base_url(url: &str) -> Result<String> {
    let t = url.trim().trim_end_matches('/');
    if t.is_empty() {
        return Err(anyhow!("a Tvheadend address is required"));
    }
    let lower = t.to_ascii_lowercase();
    let base = if lower.starts_with("http://") || lower.starts_with("https://") {
        t.to_owned()
    } else if lower.contains("://") {
        return Err(anyhow!("Tvheadend is reached over http:// or https://"));
    } else {
        format!("http://{t}")
    };
    let authority = base
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(rest))
        .unwrap_or("");
    if authority.contains('@') {
        return Err(anyhow!(
            "put the username and password in their own fields, not in the address"
        ));
    }
    reqwest::Url::parse(&base).context("that is not a web address")?;
    Ok(base)
}

/// The HTTP status a failure carries, if a server sent one.
pub fn status_of(e: &anyhow::Error) -> Option<u16> {
    e.chain()
        .find_map(|c| c.downcast_ref::<reqwest::Error>())
        .and_then(|r| r.status())
        .map(|s| s.as_u16())
        .or_else(|| e.chain().find_map(|c| c.downcast_ref::<Refused>()).map(|r| r.status))
}

pub struct TvhClient {
    base: String,
    username: String,
    password: String,
    http: reqwest::Client,
}

impl TvhClient {
    pub fn new(account: &TvhAccount) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("zapper/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            // A redirect would change the URI the Digest response was
            // computed over, and Tvheadend has no reason to send one.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("building http client")?;
        Ok(Self {
            base: base_url(&account.url)?,
            username: account.username.trim().to_owned(),
            password: account.password.clone(),
            http,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn has_login(&self) -> bool {
        !self.username.is_empty()
    }

    /// `GET` a path on the server, logging in if it asks. `what` names it in
    /// errors, which never carry the URL.
    pub async fn get(&self, path: &str, what: &str) -> Result<reqwest::Response> {
        let url = format!("{}{path}", self.base);
        let parsed = reqwest::Url::parse(&url).context("building a Tvheadend address")?;
        // What the Digest response is computed over: the request target,
        // exactly as sent.
        let uri = match parsed.query() {
            Some(q) => format!("{}?{q}", parsed.path()),
            None => parsed.path().to_owned(),
        };

        let first = self.send(self.http.get(parsed.clone()), what).await?;
        if first.status() != reqwest::StatusCode::UNAUTHORIZED {
            return finish(first, what);
        }
        if !self.has_login() {
            return Err(Refused::new(401, NEEDS_LOGIN).into());
        }
        let mut scheme = challenge(&first);
        drop(first);
        // Two goes at most: the second only when the first nonce went stale
        // between the challenge and the answer.
        for attempt in 0..2 {
            let resp = match &scheme {
                Some(Scheme::Digest(ch)) => {
                    let header = digest::authorization(
                        ch,
                        &self.username,
                        &self.password,
                        "GET",
                        &uri,
                        1,
                        &digest::cnonce(),
                    );
                    self.send(self.http.get(parsed.clone()).header(AUTHORIZATION, header), what)
                        .await?
                }
                Some(Scheme::Basic) => {
                    self.send(
                        self.http
                            .get(parsed.clone())
                            .basic_auth(&self.username, Some(&self.password)),
                        what,
                    )
                    .await?
                }
                None => {
                    return Err(anyhow!(
                        "Tvheadend asked for a kind of login zapper cannot give ({what})"
                    ))
                }
            };
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                if let Some(Scheme::Digest(next)) = challenge(&resp) {
                    if next.stale {
                        scheme = Some(Scheme::Digest(next));
                        continue;
                    }
                }
            }
            return finish(resp, what);
        }
        unreachable!("the loop returns on its second pass")
    }

    async fn send(&self, req: reqwest::RequestBuilder, what: &str) -> Result<reqwest::Response> {
        req.send()
            .await
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("could not reach Tvheadend at {} ({what})", self.base))
    }

    /// The channel playlist whose stream URLs mpv can open on its own. See
    /// the module notes for which one, and why.
    pub async fn playlist(&self) -> Result<reqwest::Response> {
        const WHAT: &str = "the channel playlist";
        if !self.has_login() {
            return self.get("/playlist/channels", WHAT).await;
        }
        let refused = match self.get("/playlist/auth/channels", WHAT).await {
            Ok(r) => return Ok(r),
            Err(e) => match status_of(&e) {
                Some(401 | 403) => e,
                _ => return Err(e),
            },
        };
        // Only to tell the refusals apart; this playlist is not used.
        match self.get("/playlist/channels", WHAT).await {
            Ok(_) => Err(Refused::new(403, NEEDS_PERSISTENT_AUTH).into()),
            Err(e) => match status_of(&e) {
                Some(s @ (401 | 403)) => Err(Refused::new(s, bad_login(s)).into()),
                _ => Err(refused),
            },
        }
    }

    /// `/api/serverinfo`: the name and version, for the connection test.
    pub async fn server_info(&self) -> Result<String> {
        let v: serde_json::Value = self
            .get("/api/serverinfo", "the server's details")
            .await?
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("reading the server's details")?;
        let name = v["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("Tvheadend");
        Ok(match v["sw_version"].as_str().filter(|s| !s.is_empty()) {
            Some(ver) => format!("{name} {ver}"),
            None => name.to_owned(),
        })
    }

    /// Which tag each channel is filed under, best effort.
    ///
    /// The playlist names no groups — Tvheadend writes `tvg-id`, `tvg-chno`
    /// and a logo, and nothing else — but its tags are how its owner sorted
    /// the channels, so they are read from the API: the tags in their own
    /// order (the `index` the administrator gave them), and each channel's
    /// list of tag UUIDs. A channel goes under the first of its tags in that
    /// order. A server that will not show the API to this user (it is a
    /// separate permission from streaming) gets `None`, and one category.
    pub async fn tags(&self) -> Option<Tags> {
        let tags: serde_json::Value = self
            .get("/api/channeltag/grid?limit=10000", "the channel tags")
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let channels: serde_json::Value = self
            .get("/api/channel/grid?limit=100000", "the channel list")
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        Some(Tags::from_api(&tags, &channels))
    }
}

fn challenge(resp: &reqwest::Response) -> Option<Scheme> {
    digest::choose(
        resp.headers()
            .get_all(WWW_AUTHENTICATE)
            .iter()
            .filter_map(|v| v.to_str().ok()),
    )
}

fn finish(resp: reqwest::Response, what: &str) -> Result<reqwest::Response> {
    resp.error_for_status()
        .map_err(reqwest::Error::without_url)
        .with_context(|| format!("Tvheadend refused {what}"))
}

/// Channel UUID to tag name, and the tags in the order to show them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    pub by_channel: HashMap<String, String>,
    pub order: Vec<String>,
}

impl Tags {
    pub fn from_api(tags: &serde_json::Value, channels: &serde_json::Value) -> Self {
        let mut list: Vec<(i64, String, String)> = tags["entries"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|t| t["enabled"].as_bool().unwrap_or(true))
            // Internal tags are Tvheadend's own bookkeeping, hidden from its
            // clients by design.
            .filter(|t| !t["internal"].as_bool().unwrap_or(false))
            .filter_map(|t| {
                let uuid = t["uuid"].as_str()?.to_owned();
                let name = t["name"].as_str()?.trim().to_owned();
                (!name.is_empty()).then(|| (t["index"].as_i64().unwrap_or(0), name, uuid))
            })
            .collect();
        list.sort();
        let rank: HashMap<&str, usize> = list
            .iter()
            .enumerate()
            .map(|(i, (_, _, uuid))| (uuid.as_str(), i))
            .collect();
        let mut by_channel = HashMap::new();
        for ch in channels["entries"]
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
        {
            let Some(uuid) = ch["uuid"].as_str() else { continue };
            let first = ch["tags"]
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .filter_map(|t| t.as_str().and_then(|t| rank.get(t)))
                .min();
            if let Some(&i) = first {
                by_channel.insert(uuid.to_owned(), list[i].1.clone());
            }
        }
        let mut order: Vec<String> = Vec::new();
        for (_, name, _) in &list {
            if !order.contains(name) {
                order.push(name.clone());
            }
        }
        Tags { by_channel, order }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_read_however_they_are_typed() {
        assert_eq!(base_url("tvh.lan:9981").unwrap(), "http://tvh.lan:9981");
        assert_eq!(base_url(" https://tv.example/tvh/ ").unwrap(), "https://tv.example/tvh");
        assert!(base_url("").is_err());
        assert!(base_url("htsp://tvh.lan:9982").is_err());
        let err = base_url("http://bob:hunter2@tvh.lan:9981").unwrap_err().to_string();
        assert!(err.contains("own fields") && !err.contains("hunter2"), "{err}");
    }

    #[test]
    fn channels_go_under_their_first_tag_in_the_servers_order() {
        let tags = serde_json::json!({"entries": [
            {"uuid": "t-news", "name": "News", "index": 2, "enabled": true},
            {"uuid": "t-tv", "name": "TV channels", "index": 1, "enabled": true},
            {"uuid": "t-off", "name": "Disabled", "index": 0, "enabled": false},
            {"uuid": "t-int", "name": "Internal", "index": 0, "internal": true},
        ]});
        let chans = serde_json::json!({"entries": [
            {"uuid": "c1", "tags": ["t-news", "t-tv"]},
            {"uuid": "c2", "tags": ["t-news"]},
            {"uuid": "c3", "tags": ["t-off", "t-int"]},
            {"uuid": "c4"},
        ]});
        let t = Tags::from_api(&tags, &chans);
        assert_eq!(t.order, ["TV channels", "News"]);
        assert_eq!(t.by_channel.get("c1").map(String::as_str), Some("TV channels"));
        assert_eq!(t.by_channel.get("c2").map(String::as_str), Some("News"));
        assert_eq!(t.by_channel.get("c3"), None);
        assert_eq!(t.by_channel.get("c4"), None);
    }
}
