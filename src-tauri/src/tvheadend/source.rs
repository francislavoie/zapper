//! A Tvheadend server as the account in use.

use anyhow::{anyhow, Context, Result};

use super::client::TvhClient;
use super::TvhAccount;
use crate::source::Playable;

pub struct TvhSource {
    account: TvhAccount,
    client: TvhClient,
}

impl TvhSource {
    pub fn new(account: TvhAccount) -> Result<Self> {
        let client = TvhClient::new(&account)?;
        Ok(Self { account, client })
    }

    pub fn account(&self) -> &TvhAccount {
        &self.account
    }

    pub fn client(&self) -> &TvhClient {
        &self.client
    }

    /// The stored URL, which carries its own authentication (`?auth=`; see
    /// `client`), so mpv opens it with no help.
    pub fn live(&self, stream_id: i64) -> Result<Playable> {
        let row = crate::source::stored_row(stream_id)?;
        Ok(Playable::plain(row.url))
    }

    /// The connection test: log in, say what the server is, and count the
    /// channels in the playlist this login gets — which is also the check
    /// that persistent authentication is on, since that playlist is refused
    /// without it.
    pub async fn probe(&self) -> Result<String> {
        let server = self.client.server_info().await?;
        let body = self
            .client
            .playlist()
            .await?
            .bytes()
            .await
            .map_err(reqwest::Error::without_url)
            .context("reading the channel playlist")?;
        let (count, _, _) = crate::m3u::parse::sample(std::io::Cursor::new(body), usize::MAX)
            .context("reading the channel playlist")?;
        if count == 0 {
            return Err(anyhow!(
                "{server} lists no channels for this login. Check its access entries."
            ));
        }
        let mut msg = format!(
            "{server}: {count} channel{}",
            if count == 1 { "" } else { "s" }
        );
        if !self.client.has_login() {
            msg.push_str(" · no login");
        }
        Ok(msg)
    }
}
