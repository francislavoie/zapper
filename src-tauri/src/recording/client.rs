//! Talking to the recording service over its API: the app's side.
//!
//! The service on this machine is found through its config file (the port)
//! and its token file, both in zapper's config directory; see [`super::config`].

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;

use super::api::{
    ErrorDto, NetworkDto, NetworkSettingsDto, PasswordRequest, RecordingDto, RecordingSettingsDto,
    ScheduleRequest, StatusDto, VersionDto,
};

pub struct Client {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

/// Why a request got no answer, in words for the interface.
pub const NOT_RUNNING: &str = "The recording service is not running.";

impl Client {
    /// The service on this machine, as its config file describes it.
    pub fn local() -> Result<Self> {
        let cfg = super::config::load();
        Self::new(
            &format!("http://127.0.0.1:{}", cfg.port),
            super::config::read_token(),
        )
    }

    pub fn new(base: &str, token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(15))
            // Loopback: no proxy may stand between the app and the service.
            .no_proxy()
            .build()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            http,
        })
    }

    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> Result<T> {
        let req = match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        };
        let resp = req.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                anyhow!(NOT_RUNNING)
            } else {
                anyhow!("the recording service did not answer: {e}")
            }
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp.json().await?);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!(
                "the recording service refused this app's token; restarting the service makes a new one available"
            ));
        }
        let why = resp
            .json::<ErrorDto>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| status.to_string());
        Err(anyhow!(why))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn version(&self) -> Result<VersionDto> {
        self.send(self.http.get(self.url("/api/version"))).await
    }

    pub async fn status(&self) -> Result<StatusDto> {
        self.send(self.http.get(self.url("/api/status"))).await
    }

    pub async fn list(&self) -> Result<Vec<RecordingDto>> {
        self.send(self.http.get(self.url("/api/recordings"))).await
    }

    pub async fn schedule(&self, req: &ScheduleRequest) -> Result<RecordingDto> {
        self.send(self.http.post(self.url("/api/recordings")).json(req))
            .await
    }

    /// Cancels a scheduled recording, or stops one being captured (keeping
    /// what it has).
    pub async fn cancel(&self, id: i64) -> Result<()> {
        self.send::<serde_json::Value>(
            self.http
                .post(self.url(&format!("/api/recordings/{id}/cancel"))),
        )
        .await
        .map(|_| ())
    }

    /// Forgets a recording; with `files`, deletes what it recorded too.
    pub async fn delete(&self, id: i64, files: bool) -> Result<serde_json::Value> {
        self.send(
            self.http
                .delete(self.url(&format!("/api/recordings/{id}?files={files}"))),
        )
        .await
    }

    pub async fn settings(&self) -> Result<RecordingSettingsDto> {
        self.send(self.http.get(self.url("/api/settings"))).await
    }

    pub async fn set_settings(&self, s: &RecordingSettingsDto) -> Result<RecordingSettingsDto> {
        self.send(self.http.put(self.url("/api/settings")).json(s))
            .await
    }

    /// Network access: whether other devices may reach the service, the
    /// devices signed in.
    pub async fn network(&self) -> Result<NetworkDto> {
        self.send(self.http.get(self.url("/api/network"))).await
    }

    pub async fn set_network(&self, s: &NetworkSettingsDto) -> Result<NetworkDto> {
        self.send(self.http.put(self.url("/api/network")).json(s))
            .await
    }

    /// Sets the password other devices sign in with, signing every device
    /// out.
    pub async fn set_password(&self, password: &str) -> Result<NetworkDto> {
        let body = PasswordRequest {
            password: password.to_string(),
        };
        self.send(self.http.put(self.url("/api/network/password")).json(&body))
            .await
    }

    pub async fn revoke_device(&self, id: &str) -> Result<NetworkDto> {
        // The id is the service's own hex; anything else is refused here
        // rather than spliced into a path.
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(anyhow!("no such device"));
        }
        self.send(self.http.delete(self.url(&format!("/api/devices/{id}"))))
            .await
    }
}
