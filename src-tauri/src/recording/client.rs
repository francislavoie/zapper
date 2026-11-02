//! Talking to the recording service over its API: the app's side.
//!
//! The service on this machine is found through its config file (the port)
//! and its token file, both in zapper's config directory; see [`super::config`].

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::backup::{BackupsDto, SetBackupRequest};
use super::api::{
    ConflictDto, ErrorDto, NetworkDto, NetworkSettingsDto, PartDto, PasswordRequest, RecordingDto,
    RecordingSettingsDto, ScheduleRequest, ShutdownRequest, StatusDto, VersionDto,
};

pub struct Client {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

/// Why a request got no answer, in words for the interface.
pub const NOT_RUNNING: &str = "The recording service is not running.";

/// What the app's player asks of `POST /api/watch`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerWatch {
    pub stream_id: i64,
    /// Connect even though the account looks full: the player has just let
    /// its own connection go, which a provider may still be counting.
    pub anyway: bool,
    /// Only a connection the service already has.
    pub only_shared: bool,
    /// The player's place it leaves for this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    /// Always set: the stream as it comes, for mpv.
    pub player: bool,
}

/// A place on the service's connection.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watching {
    pub viewer: String,
    /// `/api/stream/{viewer}`.
    pub stream: String,
    /// It joined a connection the service already had.
    #[serde(default)]
    pub shared: bool,
}

/// Why a place was not given.
#[derive(Debug, Clone)]
pub struct WatchRefused {
    /// The HTTP status, 0 when nothing answered.
    pub status: u16,
    /// The service's reason (`recording`, `inUse`, `feed`, `notOpen`, ...).
    pub reason: String,
    pub error: String,
    /// The provider refused the connection for the account's limit.
    pub limit: bool,
}

impl std::fmt::Display for WatchRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

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
            // Plain HTTP to 127.0.0.1, so no certificate authorities: the
            // system's would be loaded otherwise, and a computer without any
            // (a bare container) could not build this client at all, nor
            // set up or stop its own service.
            .tls_certs_only(std::iter::empty())
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

    /// Asks the service to stop, or restart, now or once it is idle. It
    /// answers before it goes, with its status.
    pub async fn shutdown(&self, req: &ShutdownRequest) -> Result<StatusDto> {
        self.send(self.http.post(self.url("/api/shutdown")).json(req))
            .await
    }

    pub async fn list(&self) -> Result<Vec<RecordingDto>> {
        self.send(self.http.get(self.url("/api/recordings"))).await
    }

    pub async fn schedule(&self, req: &ScheduleRequest) -> Result<RecordingDto> {
        self.send(self.http.post(self.url("/api/recordings")).json(req))
            .await
    }

    /// The conflicts recording `req` would bring, before it is scheduled.
    pub async fn preview(&self, req: &ScheduleRequest) -> Result<Vec<ConflictDto>> {
        self.send(
            self.http
                .post(self.url("/api/recordings/preview"))
                .json(req),
        )
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

    /// The channels recording `id` could fall back to, and its backup now.
    pub async fn backups(&self, id: i64) -> Result<BackupsDto> {
        self.send(
            self.http
                .get(self.url(&format!("/api/recordings/{id}/backups"))),
        )
        .await
    }

    /// Chooses recording `id`'s backup feed; `None` for none.
    pub async fn set_backup(&self, id: i64, stream_id: Option<i64>) -> Result<RecordingDto> {
        self.send(
            self.http
                .put(self.url(&format!("/api/recordings/{id}/backup")))
                .json(&SetBackupRequest { stream_id }),
        )
        .await
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

    /// Asks the service to refresh the catalogue now (`POST /api/sync`).
    pub async fn sync_now(&self) -> Result<serde_json::Value> {
        self.send(self.http.post(self.url("/api/sync"))).await
    }

    /// A place for the app's player on the service's connection to a
    /// channel (`POST /api/watch`, `recording::watch`).
    pub async fn watch(&self, req: &PlayerWatch) -> std::result::Result<Watching, WatchRefused> {
        let mut r = self.http.post(self.url("/api/watch")).json(req);
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        let resp = r.send().await.map_err(|e| WatchRefused {
            status: 0,
            reason: "unreachable".into(),
            error: if e.is_connect() || e.is_timeout() {
                NOT_RUNNING.into()
            } else {
                format!("the recording service did not answer: {e}")
            },
            limit: false,
        })?;
        let status = resp.status();
        if status.is_success() {
            return resp.json().await.map_err(|e| WatchRefused {
                status: status.as_u16(),
                reason: "unreadable".into(),
                error: format!("the recording service's answer was unreadable: {e}"),
                limit: false,
            });
        }
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        Err(WatchRefused {
            status: status.as_u16(),
            reason: body["reason"].as_str().unwrap_or_default().to_string(),
            error: body["error"]
                .as_str()
                .map_or_else(|| status.to_string(), str::to_string),
            limit: body["ending"]["limit"].as_bool().unwrap_or(false),
        })
    }

    /// Lets go of the player's place at once, rather than when nobody has
    /// read it for a while: its connection, when nobody else is on it, is
    /// then free for whatever the player opens next.
    pub async fn stop_watching(&self, viewer: &str) -> Result<()> {
        if viewer.is_empty() || !viewer.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(anyhow!("no such viewer"));
        }
        let mut r = self
            .http
            .delete(self.url(&format!("/api/watch/{viewer}")))
            .timeout(Duration::from_secs(3));
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        r.send().await?;
        Ok(())
    }

    /// Where the player reads a place's stream, and the header that lets it
    /// (`http-header-fields`, so the token is never part of an address that
    /// is logged).
    pub fn stream(&self, w: &Watching) -> (String, Option<String>) {
        (
            self.url(&w.stream),
            self.token.as_ref().map(|t| format!("Authorization: Bearer {t}")),
        )
    }

    /// Hands over what the player had buffered of the programme as recording
    /// `id`'s first part: the MPEG-TS file at `path`, whose first packet
    /// arrived at `first_ms`.
    pub async fn prelude(&self, id: i64, path: &std::path::Path, first_ms: i64) -> Result<PartDto> {
        let file = tokio::fs::File::open(path).await?;
        let len = file.metadata().await?.len();
        self.send(
            self.http
                .put(self.url(&format!("/api/recordings/{id}/prelude?firstMs={first_ms}")))
                .header(reqwest::header::CONTENT_TYPE, "video/mp2t")
                .header(reqwest::header::CONTENT_LENGTH, len)
                // Written to disk and read through once over loopback: a
                // large buffer takes seconds, not the usual answer's moment.
                .timeout(Duration::from_secs(300))
                .body(reqwest::Body::from(file)),
        )
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
