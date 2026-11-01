//! An HDHomeRun as the account in use: playing a channel, testing the
//! connection, and saying why a channel would not open.

use anyhow::Result;
use std::time::Duration;

use super::device::{self, Device};
use super::guide::GUIDE_API;
use super::HdhrAccount;
use crate::source::Playable;

pub struct HdhrSource {
    account: HdhrAccount,
    device: Device,
    /// SiliconDust's guide service; a local server in tests.
    guide_api: String,
}

impl HdhrSource {
    pub fn new(account: HdhrAccount) -> Result<Self> {
        let device = Device::new(&account.device)?;
        Ok(Self {
            account,
            device,
            guide_api: GUIDE_API.to_owned(),
        })
    }

    /// Points the guide at somewhere other than SiliconDust. For tests.
    pub fn with_guide_api(mut self, api: &str) -> Self {
        self.guide_api = api.to_owned();
        self
    }

    pub fn account(&self) -> &HdhrAccount {
        &self.account
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn guide_api(&self) -> &str {
        &self.guide_api
    }

    /// The lineup's URL, as the tuner gave it. No headers: the tuner serves
    /// anyone on the network, and mpv's defaults are what it expects.
    pub fn live(&self, stream_id: i64) -> Result<Playable> {
        let row = crate::source::stored_row(stream_id)?;
        Ok(Playable::plain(row.url))
    }

    /// The connection test: what the tuner is, how many channels it can
    /// tune, and how many tuners it has to tune them with.
    pub async fn probe(&self) -> Result<String> {
        let d = self.device.discover().await?;
        let lineup = self.device.lineup(&d).await?;
        let l = device::entries(&d.device_id, &lineup);
        let mut msg = format!("{}: {}", d.title(), count(l.entries.len(), "channel"));
        if let Some(t) = d.tuners() {
            msg.push_str(&format!(", {}", count(t as usize, "tuner")));
        }
        if l.protected > 0 {
            msg.push_str(&format!(" · {} copy-protected, not listed", l.protected));
        }
        if self.account.epg_urls.is_empty() && d.device_auth.trim().is_empty() {
            msg.push_str(" · no guide: this tuner offers no SiliconDust token, so add an XMLTV guide");
        }
        Ok(msg)
    }

    /// How many tuners there are and how many are busy, for the connection
    /// budget. Two small reads on the LAN; `None` if either fails.
    pub async fn tuner_status(&self) -> Option<crate::connections::Status> {
        let max = self.device.discover().await.ok()?.tuners()?;
        let status = self.device.status().await.ok()?;
        Some(crate::connections::Status {
            max: Some(max),
            active: Some(crate::connections::tuners_in_use(&status)),
            expires: None,
        })
    }

    /// Why a channel that just failed to open failed, when the tuner can say.
    ///
    /// mpv reports a refused stream as "loading failed", which is true and
    /// useless. The tuner knows better: it answers `503` with
    /// `X-HDHomeRun-Error: 805` when every tuner is busy — somebody else
    /// watching, a DVR recording — and `806`/`807` when it tuned and found
    /// nothing. So after a failure the same URL is asked once more, and only
    /// the status line is read. If a tuner has come free in the meantime the
    /// answer is a stream, which is dropped at once; that hands the tuner
    /// straight back.
    pub async fn explain_failure(&self, stream_id: i64) -> Option<String> {
        let row = crate::source::stored_row(stream_id).ok()?;
        self.explain_url(&row.url).await
    }

    pub async fn explain_url(&self, url: &str) -> Option<String> {
        let resp = self
            .device
            .http()
            .get(url)
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .ok()?;
        let status = resp.status().as_u16();
        let code = resp
            .headers()
            .get("x-hdhomerun-error")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        drop(resp);
        let reason = refusal(status, &code)?;
        Some(match reason {
            Refusal::TunersBusy => {
                // Asked only now, and only for the count: the message is the
                // one place it matters.
                let d = self.device.discover().await.ok();
                let name = d.as_ref().map_or_else(|| "the HDHomeRun".to_owned(), |d| d.title());
                match d.and_then(|d| d.tuners()) {
                    Some(1) => format!(
                        "The only tuner on {name} is in use. Stop whatever is watching or recording on it, then try again."
                    ),
                    Some(n) => format!(
                        "All {n} tuners on {name} are in use. Stop something that is watching or recording on another device, then try again."
                    ),
                    None => format!(
                        "Every tuner on {name} is in use. Stop something that is watching or recording on another device, then try again."
                    ),
                }
            }
            Refusal::NoSignal => {
                "The tuner found no signal on this channel. It may be off the air, or too weak to receive here.".to_owned()
            }
            Refusal::TuneFailed => {
                "The tuner could not tune this channel. Rescanning channels in the HDHomeRun app may help.".to_owned()
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    TunersBusy,
    TuneFailed,
    NoSignal,
}

/// What a tuner's answer means. `None` for anything that is not a refusal
/// this can explain — including a stream, which means it would play now.
fn refusal(status: u16, error_header: &str) -> Option<Refusal> {
    let code = error_header.split_whitespace().next().unwrap_or("");
    match code {
        "805" => Some(Refusal::TunersBusy),
        "806" => Some(Refusal::TuneFailed),
        "807" => Some(Refusal::NoSignal),
        // A tuner that says 503 without saying why is out of tuners: it is
        // the only reason the firmware gives that status for a channel URL.
        _ if status == 503 => Some(Refusal::TunersBusy),
        _ => None,
    }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tuners_refusals_are_told_apart() {
        assert_eq!(refusal(503, "805 All Tuners In Use"), Some(Refusal::TunersBusy));
        assert_eq!(refusal(503, ""), Some(Refusal::TunersBusy));
        assert_eq!(refusal(503, "807 No Video Data"), Some(Refusal::NoSignal));
        assert_eq!(refusal(503, "806 Tune Failed"), Some(Refusal::TuneFailed));
        assert_eq!(refusal(200, ""), None);
        assert_eq!(refusal(404, ""), None);
    }

    #[test]
    fn counts_read_as_english() {
        assert_eq!(count(1, "tuner"), "1 tuner");
        assert_eq!(count(4, "tuner"), "4 tuners");
        assert_eq!(count(0, "channel"), "0 channels");
    }
}
