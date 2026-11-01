//! The running service: its state, the scheduler loop, and what the API's
//! requests do.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use tokio::sync::{watch, Notify};

use super::access::Access;
use super::api::{
    ActiveDto, NetworkDto, NetworkSettingsDto, RecordingDto, RecordingSettingsDto, RecordingState,
    ScheduleRequest, StatusDto, API_VERSION,
};
use super::budget::Budget;
use super::config::{self, NetworkConfig, ServerConfig};
use super::listen::{self, Listening};
use super::naming::{self, Programme};
use super::schedule::{plan, sleep_for};
use super::{capture, http, now, store, tls};
use crate::localtime::Civil;

/// A capture's live state, for status and for stopping it.
pub struct Live {
    pub title: String,
    pub stream_id: i64,
    pub bytes: u64,
    pub since: i64,
    pub waiting: Option<String>,
    pub stop: watch::Sender<bool>,
}

pub struct Server {
    db: Mutex<Connection>,
    config: Mutex<ServerConfig>,
    config_path: PathBuf,
    pub token: String,
    live: Mutex<HashMap<i64, Live>>,
    /// Streams this service holds open right now.
    pub streams_open: AtomicUsize,
    wake: Notify,
    pub budget: Box<dyn Budget>,
    /// The sign-in password, the devices signed in, failed sign-ins.
    pub access: Access,
    /// The listeners, while the API is served; one change of network
    /// settings at a time.
    listening: tokio::sync::Mutex<Option<Open>>,
    /// Why network access is not on as set, when it is not.
    listen_error: Mutex<Option<String>>,
}

impl Server {
    /// The password, devices and certificate are read from beside
    /// `config_path`.
    pub fn new(
        db: Connection,
        config: ServerConfig,
        config_path: PathBuf,
        token: String,
        budget: Box<dyn Budget>,
    ) -> Arc<Self> {
        let dir = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Arc::new(Self {
            db: Mutex::new(db),
            config: Mutex::new(config),
            access: Access::load(&dir),
            config_path,
            token,
            live: Mutex::new(HashMap::new()),
            streams_open: AtomicUsize::new(0),
            wake: Notify::new(),
            budget,
            listening: tokio::sync::Mutex::new(None),
            listen_error: Mutex::new(None),
        })
    }

    /// Where the password, devices and certificate are kept.
    fn dir(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// Runs `f` on the recordings database.
    pub fn db<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let c = self.db.lock().unwrap_or_else(|e| e.into_inner());
        f(&c)
    }

    pub fn config(&self) -> ServerConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn live<R>(&self, f: impl FnOnce(&mut HashMap<i64, Live>) -> R) -> R {
        f(&mut self.live.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Updates a capture's live state, if it is still running.
    pub fn update_live(&self, id: i64, f: impl FnOnce(&mut Live)) {
        self.live(|m| {
            if let Some(l) = m.get_mut(&id) {
                f(l)
            }
        });
    }

    pub fn note(&self, id: i64, note: &str) {
        tracing::info!("recording {id}: {note}");
        if let Err(e) = self.db(|c| store::add_note(c, id, note)) {
            tracing::warn!("recording {id}: could not keep a note: {e:#}");
        }
    }

    /// Looks at the schedule again now, rather than at the next wake.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    /// The scheduler: start what is due, give up on what was missed, sleep.
    pub async fn run_scheduler(self: Arc<Self>) {
        loop {
            let t = now();
            let slots = match self.db(store::slots) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("reading the schedule: {e:#}");
                    Vec::new()
                }
            };
            let active: HashSet<i64> = self.live(|m| m.keys().copied().collect());
            let p = plan(t, &slots, &active);
            for id in p.missed {
                let _ = self.db(|c| {
                    store::set_state(
                        c,
                        id,
                        RecordingState::Failed,
                        Some("missed: the recording service was not running"),
                    )
                });
            }
            for id in p
                .start
                .into_iter()
                .map(|id| (id, true))
                .chain(p.finish.into_iter().map(|id| (id, false)))
            {
                self.spawn_capture(id.0, id.1);
            }
            let wait = sleep_for(t, p.next);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(wait.max(1) as u64)) => {}
                _ = self.wake.notified() => {}
            }
        }
    }

    /// Starts capturing (or, with `capture` false, only finishes) a
    /// recording, registered as live first so the next look does not start
    /// it twice.
    fn spawn_capture(self: &Arc<Self>, id: i64, capture: bool) {
        let Ok(Some(rec)) = self.db(|c| store::get(c, id)) else {
            return;
        };
        let (tx, rx) = watch::channel(false);
        self.live(|m| {
            m.insert(
                id,
                Live {
                    title: rec.title.clone(),
                    stream_id: rec.stream_id,
                    bytes: 0,
                    since: now(),
                    waiting: None,
                    stop: tx,
                },
            )
        });
        let me = Arc::clone(self);
        tokio::spawn(async move {
            if capture {
                capture::run(&me, id, rx).await;
            }
            capture::finish(&me, id).await;
            me.live(|m| m.remove(&id));
        });
    }

    // ------------------------------------------------------------- requests

    pub fn list(&self) -> Result<Vec<RecordingDto>> {
        let mut list = self.db(store::list)?;
        self.live(|m| {
            for r in &mut list {
                if let Some(l) = m.get(&r.id) {
                    r.waiting = l.waiting.clone();
                }
            }
        });
        Ok(list)
    }

    pub fn get(&self, id: i64) -> Result<RecordingDto> {
        let mut r = self
            .db(|c| store::get(c, id))?
            .ok_or_else(|| anyhow!("no such recording"))?;
        r.waiting = self.live(|m| m.get(&id).and_then(|l| l.waiting.clone()));
        Ok(r)
    }

    pub fn schedule(&self, req: ScheduleRequest) -> Result<RecordingDto> {
        let t = now();
        if req.title.trim().is_empty() {
            bail!("a recording needs a title");
        }
        if req.stop <= req.start {
            bail!("the programme ends before it starts");
        }
        if req.stop <= t {
            bail!("that programme is over");
        }
        if let Some(id) = self.db(|c| store::find(c, req.stream_id, req.start))? {
            return self.get(id);
        }
        let cfg = self.config();
        let backup = match req.backup_stream_id {
            Some(b) if b == req.stream_id => None,
            Some(b) => Some(b),
            None => backup_for(req.stream_id).unwrap_or(None),
        };
        let channel_name = match &req.channel_name {
            Some(n) if !n.trim().is_empty() => n.clone(),
            _ => {
                channel_name(req.stream_id).unwrap_or_else(|| format!("Channel {}", req.stream_id))
            }
        };
        let programme = Programme {
            title: &req.title,
            episode_title: req.episode_title.as_deref(),
            channel: &channel_name,
            season: req.season,
            episode: req.episode,
        };
        // Decided now and kept with the recording, so a change of settings
        // never splits one recording across two places.
        let place = naming::place(cfg.pattern(), &programme, Civil::local(req.start));
        let folder = place
            .folders
            .iter()
            .fold(PathBuf::from(&cfg.output_dir), |p, f| p.join(f));
        let base = place.base;
        let id = self.db(|c| {
            store::insert(
                c,
                &store::NewRecording {
                    req: &req,
                    backup_stream_id: backup,
                    channel_name: &channel_name,
                    pad_before: req.pad_before.unwrap_or(cfg.pad_before).clamp(0, 6 * 3600),
                    pad_after: req.pad_after.unwrap_or(cfg.pad_after).clamp(0, 6 * 3600),
                    folder: &folder.to_string_lossy(),
                    base: &base,
                    clock_track: cfg.clock_track,
                    now: t,
                },
            )
        })?;
        self.poke();
        self.get(id)
    }

    /// A scheduled recording is forgotten; one being captured stops, keeping
    /// what it has, and is finished as usual.
    pub fn cancel(&self, id: i64) -> Result<Option<RecordingDto>> {
        let stopping = self.live(|m| m.get(&id).map(|l| l.stop.send(true)).is_some());
        if stopping {
            self.note(id, "stopped before its end");
            return self.get(id).map(Some);
        }
        let rec = self.get(id)?;
        if rec.state == RecordingState::Scheduled {
            self.db(|c| store::delete(c, id))?;
            return Ok(None);
        }
        Ok(Some(rec))
    }

    /// Forgets a finished recording, and with `files` deletes its files.
    pub fn delete(&self, id: i64, files: bool) -> Result<()> {
        if self.live(|m| m.contains_key(&id)) {
            bail!("it is being recorded; stop it first");
        }
        let rec = self.get(id)?;
        if files {
            for p in &rec.parts {
                remove_if_present(Path::new(&p.path))?;
                remove_if_present(&capture::log_path(Path::new(&p.path)))?;
            }
            // Its folders go too once nothing else is in them, up to the
            // output folder, which stays.
            let output = PathBuf::from(&self.config().output_dir);
            for dir in empty_folders(Path::new(&rec.folder), &output) {
                if std::fs::remove_dir(dir).is_err() {
                    break;
                }
            }
        }
        self.db(|c| store::delete(c, id))?;
        Ok(())
    }

    pub fn settings(&self) -> RecordingSettingsDto {
        self.config().settings()
    }

    pub fn set_settings(&self, s: RecordingSettingsDto) -> Result<RecordingSettingsDto> {
        let mut cfg = self.config();
        cfg.apply(s).map_err(|e| anyhow!(e))?;
        config::save_to(&self.config_path, &cfg)?;
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = cfg.clone();
        Ok(cfg.settings())
    }

    pub fn status(&self) -> StatusDto {
        let cfg = self.config();
        let (free, total) = disk_space(Path::new(&cfg.output_dir));
        let active = self.live(|m| {
            let mut v: Vec<ActiveDto> = m
                .iter()
                .map(|(&id, l)| ActiveDto {
                    id,
                    title: l.title.clone(),
                    stream_id: l.stream_id,
                    bytes: l.bytes,
                    since: l.since,
                    waiting: l.waiting.clone(),
                })
                .collect();
            v.sort_by_key(|a| a.id);
            v
        });
        StatusDto {
            version: env!("CARGO_PKG_VERSION").to_string(),
            api_version: API_VERSION,
            active,
            output_dir: cfg.output_dir,
            disk_free: free,
            disk_total: total,
            now: now(),
        }
    }

    pub fn open_streams(&self) -> usize {
        self.streams_open.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------------ listening

    /// Starts serving the API as the settings say. Network access that
    /// cannot be had (no password, the port taken, no certificate) falls
    /// back to 127.0.0.1, with the reason kept for Settings to show; only
    /// that failing too is an error.
    pub async fn listen(self: &Arc<Self>) -> Result<()> {
        let cfg = self.config();
        let mut net = cfg.network;
        let mut error = None;
        if net.enabled && !self.access.password_set() {
            net.enabled = false;
            error = Some("network access is off: no sign-in password is set".to_string());
        }
        let mut slot = self.listening.lock().await;
        let open = match self.open(net, cfg.port) {
            Ok(l) => l,
            Err(e) if net.enabled => {
                tracing::error!("network access: {e:#}");
                error = Some(format!("network access is off: {e:#}"));
                net.enabled = false;
                self.open(net, cfg.port)?
            }
            Err(e) => return Err(e),
        };
        *slot = Some(open);
        *self.listen_error.lock().unwrap_or_else(|e| e.into_inner()) = error;
        Ok(())
    }

    /// Where the API is served now, ports resolved.
    pub async fn listening_on(&self) -> Vec<std::net::SocketAddr> {
        self.listening
            .lock()
            .await
            .as_ref()
            .map(|o| o.listening.addrs.clone())
            .unwrap_or_default()
    }

    /// Binds and serves as `net` says, on `port`.
    fn open(self: &Arc<Self>, net: NetworkConfig, port: u16) -> Result<Open> {
        let acceptor = if net.enabled && net.https {
            let id = tls::load_or_create(&self.dir(), &tls::names_for(&listen::interface_ips()))
                .context("the HTTPS certificate")?;
            Some(tls::acceptor(&id)?)
        } else {
            None
        };
        let listeners = listen::bind(&listen::wanted(net.enabled, port))?;
        let listening = listen::start(listeners, http::router(Arc::clone(self)), acceptor);
        for a in &listening.addrs {
            let scheme = if listening.tls { "http(s)" } else { "http" };
            tracing::info!("recording service API on {scheme}://{a}");
        }
        Ok(Open { listening, net })
    }

    /// Network access as Settings shows it.
    pub fn network(&self) -> NetworkDto {
        let cfg = self.config();
        NetworkDto {
            enabled: cfg.network.enabled,
            port: cfg.port,
            https: cfg.network.https,
            password_set: self.access.password_set(),
            fingerprint: tls::load(&self.dir()).map(|i| i.fingerprint()),
            addresses: listen::addresses(cfg.port, cfg.network.https),
            error: self
                .listen_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            devices: self.access.devices(|d| d.list()),
        }
    }

    /// Changes where and how the API is served, without restarting the
    /// service: the listeners are replaced, connections already open finish
    /// their request (this one included), and recordings carry on. When the
    /// new settings cannot be served (the port is taken), the old ones stay,
    /// and the reason is the error.
    pub async fn set_network(self: &Arc<Self>, s: NetworkSettingsDto) -> Result<NetworkDto> {
        config::validate_port(s.port).map_err(|e| anyhow!(e))?;
        if s.enabled && !self.access.password_set() {
            bail!("set a sign-in password before turning on network access");
        }
        let mut slot = self.listening.lock().await;
        let old = self.config();
        let net = NetworkConfig {
            enabled: s.enabled,
            https: s.https,
        };
        let mut cfg = old.clone();
        cfg.port = s.port;
        cfg.network = net;
        let had_error = self
            .listen_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let effective = slot.as_ref().map(|o| o.net);
        let listening_changes = s.port != old.port
            || effective
                .is_none_or(|e| e.enabled != net.enabled || (net.enabled && e.https != net.https))
            || had_error;
        // The new rules apply from now, before the new listeners are up: a
        // request in between is judged by them, never by the more open of
        // the two.
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = cfg.clone();
        if listening_changes {
            match self.reopen(slot.take(), old.port, net, s.port).await {
                Ok(open) => {
                    *slot = Some(open);
                    *self.listen_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
                Err((back, e)) => {
                    *slot = back;
                    *self.config.lock().unwrap_or_else(|e| e.into_inner()) = old;
                    return Err(e);
                }
            }
        }
        config::save_to(&self.config_path, &cfg)?;
        drop(slot);
        Ok(self.network())
    }

    /// Replaces `current` with listeners for `net` on `port`. On failure,
    /// what is listening then comes back with the error: `current` itself,
    /// or the same reopened.
    async fn reopen(
        self: &Arc<Self>,
        current: Option<Open>,
        old_port: u16,
        net: NetworkConfig,
        port: u16,
    ) -> std::result::Result<Open, (Option<Open>, anyhow::Error)> {
        let Some(current) = current else {
            return self.open(net, port).map_err(|e| (None, e));
        };
        if port != old_port {
            // Another port: bound before the old one closes, so a port that
            // is taken changes nothing.
            return match self.open(net, port) {
                Ok(new) => {
                    current.listening.stop().await;
                    Ok(new)
                }
                Err(e) => Err((Some(current), e)),
            };
        }
        // The same port: the old sockets must close before it can be bound
        // with other addresses.
        let was = current.net;
        current.listening.stop().await;
        match self.open(net, port) {
            Ok(new) => Ok(new),
            Err(e) => {
                let back = self.open(was, port).or_else(|_| {
                    self.open(
                        NetworkConfig {
                            enabled: false,
                            ..was
                        },
                        port,
                    )
                });
                match back {
                    Ok(b) => Err((Some(b), e)),
                    Err(e2) => {
                        tracing::error!("the API is not served: {e2:#}");
                        Err((None, e))
                    }
                }
            }
        }
    }

    /// Sets the sign-in password for other devices, which signs every
    /// device out.
    pub async fn set_password(&self, pw: &str) -> Result<NetworkDto> {
        self.access.set_password(pw).await?;
        tracing::info!("the sign-in password was changed; every device is signed out");
        Ok(self.network())
    }

    /// Settings as given, for tests of the rules that read them.
    #[cfg(test)]
    pub fn replace_config(&self, cfg: ServerConfig) {
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = cfg;
    }

    #[cfg(test)]
    pub fn dir_for_test(&self) -> PathBuf {
        self.dir()
    }

    pub fn revoke_device(&self, id: &str) -> Result<NetworkDto> {
        if !self.access.devices(|d| d.revoke(id))? {
            bail!("no such device");
        }
        Ok(self.network())
    }
}

/// Listeners, and the network settings they were opened with (network
/// access may be off while the settings say on, when it could not be had).
struct Open {
    listening: Listening,
    net: NetworkConfig,
}

fn remove_if_present(p: &Path) -> Result<()> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("removing {}: {e}", p.display())),
    }
}

/// The folders worth removing, innermost first, once a recording's files are
/// deleted: `folder` and those around it, up to but never including the
/// output folder. A recording made under an earlier output folder gets its
/// own folder looked at, and nothing above it.
fn empty_folders<'a>(folder: &'a Path, output: &Path) -> Vec<&'a Path> {
    if folder == output {
        return Vec::new();
    }
    if !folder.starts_with(output) {
        return vec![folder];
    }
    folder
        .ancestors()
        .take_while(|d| *d != output && d.starts_with(output))
        .collect()
}

/// Another live channel carrying the same guide channel: the same programme
/// from another feed, which is what a backup is for.
fn backup_for(stream_id: i64) -> Result<Option<i64>> {
    crate::catalog::db::with_db(|c| backup_in(c, stream_id))
}

pub fn backup_in(c: &Connection, stream_id: i64) -> Result<Option<i64>> {
    Ok(c.query_row(
        "SELECT b.stream_id FROM channels a JOIN channels b
           ON b.epg_channel_id = a.epg_channel_id AND b.stream_id != a.stream_id
         WHERE a.stream_id = ?1 AND a.epg_channel_id IS NOT NULL AND a.epg_channel_id != ''
           AND b.deleted_at IS NULL AND b.is_separator = 0
         ORDER BY b.num IS NULL, b.num, b.stream_id LIMIT 1",
        [stream_id],
        |r| r.get(0),
    )
    .optional()?)
}

fn channel_name(stream_id: i64) -> Option<String> {
    crate::catalog::db::with_db(|c| {
        Ok(c.query_row(
            "SELECT name FROM channels WHERE stream_id = ?1",
            [stream_id],
            |r| r.get(0),
        )
        .optional()?)
    })
    .ok()
    .flatten()
}

/// Free and total bytes on the disk holding `dir` (or its nearest existing
/// parent).
#[cfg(unix)]
fn disk_space(dir: &Path) -> (Option<u64>, Option<u64>) {
    let mut d = dir;
    while !d.exists() {
        match d.parent() {
            Some(p) => d = p,
            None => return (None, None),
        }
    }
    let Ok(c) = std::ffi::CString::new(d.to_string_lossy().as_bytes()) else {
        return (None, None);
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a zeroed struct for statvfs to fill.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return (None, None);
    }
    let frag = st.f_frsize as u64;
    (
        Some(st.f_bavail as u64 * frag),
        Some(st.f_blocks as u64 * frag),
    )
}

#[cfg(not(unix))]
fn disk_space(_dir: &Path) -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backup_is_another_channel_with_the_same_guide_channel() {
        let c = crate::catalog::open_in_memory().unwrap();
        c.execute_batch(
            "INSERT INTO channels (stream_id, name, epg_channel_id, num) VALUES
               (1, 'One HD', 'one.uk', 1), (2, 'One SD', 'one.uk', 101),
               (3, 'One gone', 'one.uk', 50), (4, 'Two', 'two.uk', 2), (5, 'Nothing', NULL, 3);
             UPDATE channels SET deleted_at = 1 WHERE stream_id = 3;",
        )
        .unwrap();
        assert_eq!(backup_in(&c, 1).unwrap(), Some(2));
        assert_eq!(backup_in(&c, 2).unwrap(), Some(1));
        assert_eq!(backup_in(&c, 4).unwrap(), None);
        assert_eq!(backup_in(&c, 5).unwrap(), None);
    }

    #[test]
    fn deleting_tidies_folders_up_to_the_output_folder() {
        fn got(f: &'static str) -> Vec<&'static Path> {
            empty_folders(Path::new(f), Path::new("/v"))
        }
        assert_eq!(got("/v/News"), [Path::new("/v/News")]);
        assert_eq!(
            got("/v/Northwind One/Harbour Lights/Season 02"),
            [
                Path::new("/v/Northwind One/Harbour Lights/Season 02"),
                Path::new("/v/Northwind One/Harbour Lights"),
                Path::new("/v/Northwind One"),
            ]
        );
        assert!(got("/v").is_empty(), "a flat layout keeps the output folder");
        assert_eq!(got("/old/News"), [Path::new("/old/News")]);
    }
}
