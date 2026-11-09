//! Who may use the service, and from where (docs/recording.md, "Network
//! access").
//!
//! Two kinds of credential open the API:
//!
//! - **The app's token**, `server-token`, which only this computer's user can
//!   read. Accepted from this computer (a loopback address) and nowhere else,
//!   so a copy of it is no use from another machine.
//! - **A device token**, which `POST /api/login` gives whoever knows the
//!   sign-in password set in Settings. Accepted from anywhere the service
//!   listens. Each device has its own, listed in Settings and revoked there
//!   one at a time; a new password signs every device out.
//!
//! Nothing secret is kept as given: the password only as an Argon2id hash
//! (`server-password`), device tokens only as SHA-256 hashes
//! (`server-devices.json`; a token is 256 random bits, so a fast hash is as
//! good as a slow one), both readable by this user alone. Failed sign-ins
//! are counted per address and lock that address out for longer and longer.
//!
//! The rules are plain functions ([`admit`], [`identify`]) over a [`Peer`],
//! so they are tested without a socket; `recording::http` applies them.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::api::{DeviceDto, LoginDto};
use super::config::{token_matches, write_private, NetworkConfig};

/// Who is at the other end of a connection, as the listener saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub addr: SocketAddr,
    /// The connection is TLS.
    pub tls: bool,
}

impl Peer {
    /// This computer: a loopback address (`127.0.0.0/8`, `::1`, or either
    /// mapped into IPv6).
    pub fn is_local(&self) -> bool {
        is_loopback(self.addr.ip())
    }
}

pub fn is_loopback(ip: IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

/// Why a request is turned away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// From another machine while network access is off: possible only for a
    /// connection made before it was turned off.
    NetworkOff,
    /// From another machine over plain HTTP while the service wants HTTPS.
    NeedsHttps,
    NoToken,
    /// The app's token, from another machine.
    AppTokenRemote,
    /// A token the service does not know (or no longer does: revoked).
    BadToken,
    /// Only the app on this computer may do this.
    AppOnly,
}

impl Refusal {
    /// The HTTP status: 401 for a credential to fix, 403 for what no
    /// credential would change.
    pub fn status(self) -> u16 {
        match self {
            Self::NoToken | Self::BadToken | Self::AppTokenRemote => 401,
            Self::NetworkOff | Self::NeedsHttps | Self::AppOnly => 403,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::NetworkOff => "network access to the recording service is off",
            Self::NeedsHttps => "use https:// to reach the recording service from another device",
            Self::NoToken => "a valid token is required",
            Self::AppTokenRemote => {
                "this computer's app token is not accepted from another device; sign in with the password"
            }
            Self::BadToken => "a valid token is required; sign in again",
            Self::AppOnly => "only the zapper app on the service's computer can do this",
        }
    }
}

/// Who a request comes from, once its token was checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The app on this computer, with `server-token`.
    App,
    /// A signed-in device, by its id.
    Device(String),
}

/// The rule every request passes before anything else, `/api/version`
/// included: from another machine, only while network access is on, and
/// over TLS unless plain HTTP was chosen.
pub fn admit(peer: &Peer, net: &NetworkConfig) -> Result<(), Refusal> {
    if peer.is_local() {
        return Ok(());
    }
    if !net.enabled {
        return Err(Refusal::NetworkOff);
    }
    if net.https && !peer.tls {
        return Err(Refusal::NeedsHttps);
    }
    Ok(())
}

/// Whose `token` this is. The app's token counts only from this computer.
pub fn identify(
    peer: &Peer,
    token: Option<&str>,
    app_token: &str,
    devices: &mut Devices,
    now: i64,
) -> Result<Caller, Refusal> {
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    let Some(token) = token else {
        return Err(Refusal::NoToken);
    };
    if token_matches(app_token, token) {
        return if peer.is_local() {
            Ok(Caller::App)
        } else {
            Err(Refusal::AppTokenRemote)
        };
    }
    devices
        .verify(token, now, peer.addr.ip())
        .map(Caller::Device)
        .ok_or(Refusal::BadToken)
}

// ------------------------------------------------------------- password

/// Characters, not bytes: a passphrase in any script counts alike.
pub const MIN_PASSWORD: usize = 8;
/// Bytes: enough for any passphrase, and a bound on what Argon2 is given.
pub const MAX_PASSWORD: usize = 1024;

/// Why a password cannot be used, if it cannot.
pub fn password_problem(pw: &str) -> Option<String> {
    if pw.chars().count() < MIN_PASSWORD {
        return Some(format!(
            "the password needs at least {MIN_PASSWORD} characters"
        ));
    }
    if pw.len() > MAX_PASSWORD {
        return Some(format!(
            "the password is too long (at most {MAX_PASSWORD} bytes)"
        ));
    }
    if pw.trim().is_empty() {
        return Some("the password cannot be only spaces".into());
    }
    None
}

/// An Argon2id hash of `pw` in the PHC format (`$argon2id$v=19$m=…`), with a
/// random salt and the crate's defaults (19 MiB, two passes, one lane: the
/// OWASP recommendation). The parameters travel in the string, so a later
/// change of defaults still verifies older hashes.
pub fn hash_password(pw: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| anyhow!("no randomness for the salt: {e}"))?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| anyhow!("salt: {e}"))?;
    Ok(Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| anyhow!("hashing the password: {e}"))?
        .to_string())
}

pub fn verify_password(phc: &str, pw: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(pw.as_bytes(), &parsed)
        .is_ok()
}

// ------------------------------------------------------------- devices

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    id: String,
    name: String,
    #[serde(default)]
    user_agent: String,
    created: i64,
    #[serde(default)]
    last_used: Option<i64>,
    #[serde(default)]
    address: Option<String>,
    /// SHA-256 of the token, hex.
    token_hash: String,
}

impl Device {
    fn dto(&self) -> DeviceDto {
        DeviceDto {
            id: self.id.clone(),
            name: self.name.clone(),
            user_agent: self.user_agent.clone(),
            created: self.created,
            last_used: self.last_used,
            address: self.address.clone(),
        }
    }
}

/// More than anyone signs in from, and a bound on the file.
pub const MAX_DEVICES: usize = 100;

/// How often a device's last use is written to disk: every request would
/// be a write; this is close enough for "last used".
const LAST_USED_EVERY: i64 = 60;

/// The devices signed in, kept in `server-devices.json` when `path` is set.
pub struct Devices {
    list: Vec<Device>,
    path: Option<PathBuf>,
    /// `last_used` as last written, per device id.
    written: HashMap<String, Option<i64>>,
}

impl Devices {
    pub fn in_memory() -> Self {
        Self {
            list: Vec::new(),
            path: None,
            written: HashMap::new(),
        }
    }

    /// The devices on file; none when there is no file. A file that cannot
    /// be read signs every device out rather than stop the service.
    pub fn load(path: &Path) -> Self {
        let list: Vec<Device> = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(
                    "{} is not valid ({e}); no device is signed in",
                    path.display()
                );
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let written = list.iter().map(|d| (d.id.clone(), d.last_used)).collect();
        Self {
            list,
            path: Some(path.to_path_buf()),
            written,
        }
    }

    fn save(&mut self) -> Result<()> {
        if let Some(p) = &self.path {
            write_private(p, serde_json::to_string_pretty(&self.list)?.as_bytes())?;
        }
        self.written = self
            .list
            .iter()
            .map(|d| (d.id.clone(), d.last_used))
            .collect();
        Ok(())
    }

    /// Most recently used first.
    pub fn list(&self) -> Vec<DeviceDto> {
        let mut v: Vec<DeviceDto> = self.list.iter().map(Device::dto).collect();
        v.sort_by_key(|d| std::cmp::Reverse(d.last_used.unwrap_or(d.created)));
        v
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// A new device and its token, which is returned this once and kept
    /// only as its hash.
    pub fn issue(
        &mut self,
        name: &str,
        user_agent: &str,
        address: IpAddr,
        now: i64,
    ) -> Result<LoginDto> {
        if self.list.len() >= MAX_DEVICES {
            return Err(anyhow!(
                "{MAX_DEVICES} devices are signed in already; revoke some in Settings › Recording"
            ));
        }
        let token = random_hex(32)?;
        let device = Device {
            id: random_hex(8)?,
            name: clip(name.trim(), 60),
            user_agent: clip(user_agent.trim(), 300),
            created: now,
            last_used: Some(now),
            address: Some(address.to_canonical().to_string()),
            token_hash: token_hash(&token),
        };
        let dto = device.dto();
        self.list.push(device);
        if let Err(e) = self.save() {
            self.list.pop();
            return Err(e.context("keeping the new device"));
        }
        Ok(LoginDto { token, device: dto })
    }

    /// The id of the device `token` belongs to, noting that it was used now
    /// from `address`.
    pub fn verify(&mut self, token: &str, now: i64, address: IpAddr) -> Option<String> {
        let hash = token_hash(token);
        // Every entry is compared, whole, so how long it takes says nothing
        // about which one came close.
        let mut found = None;
        for (i, d) in self.list.iter().enumerate() {
            if token_matches(&d.token_hash, &hash) {
                found = Some(i);
            }
        }
        let i = found?;
        let address = address.to_canonical().to_string();
        let d = &mut self.list[i];
        d.last_used = Some(now);
        let moved = d.address.as_deref() != Some(address.as_str());
        d.address = Some(address);
        let id = d.id.clone();
        let written = self.written.get(&id).copied().flatten();
        if moved || written.is_none_or(|w| now - w >= LAST_USED_EVERY) {
            if let Err(e) = self.save() {
                tracing::warn!("noting a device's use: {e:#}");
            }
        }
        Some(id)
    }

    /// Signs a device out. False when there was no such device.
    pub fn revoke(&mut self, id: &str) -> Result<bool> {
        let before = self.list.len();
        self.list.retain(|d| d.id != id);
        if self.list.len() == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    pub fn revoke_all(&mut self) -> Result<()> {
        self.list.clear();
        self.save()
    }
}

pub(crate) fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

pub(crate) fn random_hex(n: usize) -> Result<String> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("no randomness for a token: {e}"))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// At most `max` bytes of `s`, cut on a character boundary.
fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// What to call a device that gave no name: its browser and system, as its
/// User-Agent says them ("Firefox on Android").
pub fn device_name(user_agent: &str) -> String {
    let ua = user_agent;
    // Order matters: Edge and Opera say Chrome too, and Chrome says Safari.
    let browser = if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        Some("Firefox")
    } else if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        Some("Edge")
    } else if ua.contains("OPR/") {
        Some("Opera")
    } else if ua.contains("Chrome/") || ua.contains("CriOS/") {
        Some("Chrome")
    } else if ua.contains("Safari/") {
        Some("Safari")
    } else {
        None
    };
    let system = if ua.contains("Android") {
        Some("Android")
    } else if ua.contains("iPhone") {
        Some("iPhone")
    } else if ua.contains("iPad") {
        Some("iPad")
    } else if ua.contains("CrOS") {
        Some("ChromeOS")
    } else if ua.contains("Windows") {
        Some("Windows")
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        Some("macOS")
    } else if ua.contains("Linux") {
        Some("Linux")
    } else {
        None
    };
    match (browser, system) {
        (Some(b), Some(s)) => format!("{b} on {s}"),
        (Some(b), None) => b.to_string(),
        (None, Some(s)) => format!("A device on {s}"),
        (None, None) => "A device".to_string(),
    }
}

// ------------------------------------------------------------- rate limit

/// Failed sign-ins allowed from one address before it waits.
pub const FREE_TRIES: u32 = 5;
/// The longest wait, and how long after its last failure an address is
/// forgiven.
pub const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
/// Addresses tracked at once; past this the forgiven ones are dropped, then
/// the oldest.
const MAX_TRACKED: usize = 4096;

#[derive(Debug, Clone, Copy)]
struct Strikes {
    count: u32,
    last: Instant,
    until: Option<Instant>,
}

/// Failed sign-ins per address: the first [`FREE_TRIES`] are free, then
/// each one makes the address wait before trying again, one second
/// doubling to [`MAX_WAIT`]. A success forgives the address. An IPv6
/// address counts with the rest of its /64, which one machine can have
/// all of.
#[derive(Default)]
pub struct Limiter {
    by: HashMap<IpAddr, Strikes>,
}

impl Limiter {
    fn key(ip: IpAddr) -> IpAddr {
        match ip.to_canonical() {
            IpAddr::V6(v6) => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
            v4 => v4,
        }
    }

    /// Whether `ip` may try now; if not, how long until it may.
    pub fn check(&mut self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        let key = Self::key(ip);
        let Some(s) = self.by.get(&key).copied() else {
            return Ok(());
        };
        if now.saturating_duration_since(s.last) >= MAX_WAIT {
            self.by.remove(&key);
            return Ok(());
        }
        match s.until {
            Some(until) if until > now => Err(until - now),
            _ => Ok(()),
        }
    }

    /// Notes a failure; returns how long `ip` must now wait, if at all.
    pub fn failed(&mut self, ip: IpAddr, now: Instant) -> Option<Duration> {
        if self.by.len() >= MAX_TRACKED {
            self.by
                .retain(|_, s| now.saturating_duration_since(s.last) < MAX_WAIT);
            if self.by.len() >= MAX_TRACKED {
                if let Some(oldest) = self.by.iter().min_by_key(|(_, s)| s.last).map(|(k, _)| *k) {
                    self.by.remove(&oldest);
                }
            }
        }
        let s = self.by.entry(Self::key(ip)).or_insert(Strikes {
            count: 0,
            last: now,
            until: None,
        });
        if now.saturating_duration_since(s.last) >= MAX_WAIT {
            s.count = 0;
        }
        s.count += 1;
        s.last = now;
        if s.count < FREE_TRIES {
            return None;
        }
        let wait = Duration::from_secs(1u64 << (s.count - FREE_TRIES).min(20)).min(MAX_WAIT);
        s.until = Some(now + wait);
        Some(wait)
    }

    pub fn succeeded(&mut self, ip: IpAddr) {
        self.by.remove(&Self::key(ip));
    }
}

// ------------------------------------------------------------- the whole

pub fn password_path(dir: &Path) -> PathBuf {
    dir.join("server-password")
}

pub fn devices_path(dir: &Path) -> PathBuf {
    dir.join("server-devices.json")
}

/// Why a sign-in failed.
#[derive(Debug)]
pub enum LoginError {
    /// Too many failures from this address: try again after this long.
    Wait(Duration),
    NoPassword,
    Wrong,
    Other(anyhow::Error),
}

/// The service's password, devices and failure counts, together.
pub struct Access {
    dir: Option<PathBuf>,
    /// The PHC string, when a password is set.
    password: Mutex<Option<String>>,
    pub devices: Mutex<Devices>,
    limiter: Mutex<Limiter>,
    /// Wrong live-stream keys (`recording::strm`), per address, by the same
    /// rules as wrong passwords but counted apart: a media server replaying
    /// an old `.strm` must not lock its address out of signing in.
    keys: Mutex<Limiter>,
    /// Argon2 takes tens of milliseconds and 19 MiB a try: at most two at
    /// once, so a flood of sign-ins cannot take the machine's memory or
    /// every thread a recording needs.
    hashing: tokio::sync::Semaphore,
}

impl Access {
    /// What is on file in `dir` (the config directory).
    pub fn load(dir: &Path) -> Self {
        let password = std::fs::read_to_string(password_path(dir))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| PasswordHash::new(s).is_ok());
        Self {
            dir: Some(dir.to_path_buf()),
            password: Mutex::new(password),
            devices: Mutex::new(Devices::load(&devices_path(dir))),
            limiter: Mutex::new(Limiter::default()),
            keys: Mutex::new(Limiter::default()),
            hashing: tokio::sync::Semaphore::new(2),
        }
    }

    /// Nothing on disk: for tests.
    pub fn in_memory() -> Self {
        Self {
            dir: None,
            password: Mutex::new(None),
            devices: Mutex::new(Devices::in_memory()),
            limiter: Mutex::new(Limiter::default()),
            keys: Mutex::new(Limiter::default()),
            hashing: tokio::sync::Semaphore::new(2),
        }
    }

    /// Whether `ip` may try a live-stream key now; if not, for how long.
    pub fn key_allowed(&self, ip: IpAddr) -> Result<(), Duration> {
        lock(&self.keys).check(ip, Instant::now())
    }

    /// Notes a wrong live-stream key from `ip`; how long it must now wait.
    pub fn key_wrong(&self, ip: IpAddr) -> Option<Duration> {
        lock(&self.keys).failed(ip, Instant::now())
    }

    pub fn key_right(&self, ip: IpAddr) {
        lock(&self.keys).succeeded(ip);
    }

    pub fn password_set(&self) -> bool {
        lock(&self.password).is_some()
    }

    pub fn devices<R>(&self, f: impl FnOnce(&mut Devices) -> R) -> R {
        f(&mut lock(&self.devices))
    }

    /// Sets the sign-in password, and signs every device out: whoever had
    /// the old one is out with it.
    pub async fn set_password(&self, pw: &str) -> Result<()> {
        if let Some(why) = password_problem(pw) {
            return Err(anyhow!(why));
        }
        let pw = pw.to_string();
        let phc = {
            let _slot = self.hashing.acquire().await?;
            tokio::task::spawn_blocking(move || hash_password(&pw)).await??
        };
        if let Some(dir) = &self.dir {
            write_private(&password_path(dir), phc.as_bytes()).context("saving the password")?;
        }
        *lock(&self.password) = Some(phc);
        self.devices(|d| d.revoke_all())?;
        Ok(())
    }

    /// Checks the password from `ip` and, when it is right, signs a new
    /// device in.
    pub async fn login(
        &self,
        ip: IpAddr,
        pw: &str,
        name: Option<&str>,
        user_agent: &str,
        now: i64,
    ) -> Result<LoginDto, LoginError> {
        lock(&self.limiter)
            .check(ip, Instant::now())
            .map_err(LoginError::Wait)?;
        let Some(phc) = lock(&self.password).clone() else {
            return Err(LoginError::NoPassword);
        };
        // A password too long to be one is wrong without hashing it.
        let right = pw.len() <= MAX_PASSWORD && {
            let _slot = self
                .hashing
                .acquire()
                .await
                .map_err(|e| LoginError::Other(e.into()))?;
            let pw = pw.to_string();
            tokio::task::spawn_blocking(move || verify_password(&phc, &pw))
                .await
                .map_err(|e| LoginError::Other(e.into()))?
        };
        if !right {
            let wait = lock(&self.limiter).failed(ip, Instant::now());
            tracing::warn!("a sign-in from {ip} failed: wrong password");
            return Err(match wait {
                Some(w) => LoginError::Wait(w),
                None => LoginError::Wrong,
            });
        }
        lock(&self.limiter).succeeded(ip);
        let name = match name.map(str::trim) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => device_name(user_agent),
        };
        let out = self
            .devices(|d| d.issue(&name, user_agent, ip, now))
            .map_err(LoginError::Other)?;
        tracing::info!("{} signed in from {ip}", out.device.name);
        Ok(out)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(ip: &str, tls: bool) -> Peer {
        Peer {
            addr: SocketAddr::new(ip.parse().unwrap(), 50000),
            tls,
        }
    }

    fn net(enabled: bool, https: bool) -> NetworkConfig {
        NetworkConfig { enabled, https }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zapper-access-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn loopback_is_this_computer_whatever_its_spelling() {
        for ip in ["127.0.0.1", "127.3.2.1", "::1", "::ffff:127.0.0.1"] {
            assert!(peer(ip, false).is_local(), "{ip}");
        }
        for ip in [
            "192.168.1.20",
            "10.0.0.2",
            "fe80::1",
            "::ffff:192.168.1.20",
            "0.0.0.0",
        ] {
            assert!(!peer(ip, false).is_local(), "{ip}");
        }
    }

    #[test]
    fn other_machines_get_in_only_while_network_access_is_on_and_over_tls() {
        let lan = peer("192.168.1.30", false);
        let lan_tls = peer("192.168.1.30", true);
        let me = peer("127.0.0.1", false);
        assert_eq!(admit(&me, &net(false, true)), Ok(()));
        assert_eq!(
            admit(&me, &net(true, true)),
            Ok(()),
            "loopback may stay plain HTTP"
        );
        assert_eq!(admit(&lan_tls, &net(false, true)), Err(Refusal::NetworkOff));
        assert_eq!(admit(&lan, &net(true, true)), Err(Refusal::NeedsHttps));
        assert_eq!(admit(&lan_tls, &net(true, true)), Ok(()));
        assert_eq!(
            admit(&lan, &net(true, false)),
            Ok(()),
            "plain HTTP when chosen"
        );
    }

    #[test]
    fn the_app_token_counts_from_this_computer_only() {
        let mut devices = Devices::in_memory();
        let app = "a".repeat(64);
        let me = peer("127.0.0.1", false);
        let lan = peer("192.168.1.30", true);
        assert_eq!(
            identify(&me, Some(&app), &app, &mut devices, 0),
            Ok(Caller::App)
        );
        assert_eq!(
            identify(&lan, Some(&app), &app, &mut devices, 0),
            Err(Refusal::AppTokenRemote)
        );
        assert_eq!(
            identify(&lan, None, &app, &mut devices, 0),
            Err(Refusal::NoToken)
        );
        assert_eq!(
            identify(&lan, Some(" "), &app, &mut devices, 0),
            Err(Refusal::NoToken)
        );
        assert_eq!(
            identify(&lan, Some("nope"), &app, &mut devices, 0),
            Err(Refusal::BadToken)
        );
    }

    #[test]
    fn device_tokens_are_issued_checked_and_revoked() {
        let mut devices = Devices::in_memory();
        let app = "a".repeat(64);
        let lan = peer("192.168.1.30", true);
        let me = peer("127.0.0.1", false);
        let issued = devices
            .issue("Phone", "Mozilla/5.0", lan.addr.ip(), 1000)
            .unwrap();
        assert_eq!(issued.token.len(), 64, "256 bits, hex");
        assert_eq!(issued.device.name, "Phone");
        let id = issued.device.id.clone();
        assert_eq!(
            identify(&lan, Some(&issued.token), &app, &mut devices, 1100),
            Ok(Caller::Device(id.clone()))
        );
        // A device token works from this computer too (a browser on it).
        assert_eq!(
            identify(&me, Some(&issued.token), &app, &mut devices, 1200),
            Ok(Caller::Device(id.clone()))
        );
        let listed = devices.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].last_used, Some(1200));
        assert_eq!(listed[0].address.as_deref(), Some("127.0.0.1"));
        // Another device's token is its own.
        let other = devices.issue("Tablet", "", lan.addr.ip(), 1300).unwrap();
        assert_ne!(other.token, issued.token);
        assert_eq!(devices.list()[0].name, "Tablet", "most recent first");
        assert!(devices.revoke(&id).unwrap());
        assert!(!devices.revoke(&id).unwrap());
        assert_eq!(
            identify(&lan, Some(&issued.token), &app, &mut devices, 1400),
            Err(Refusal::BadToken)
        );
        assert!(identify(&lan, Some(&other.token), &app, &mut devices, 1400).is_ok());
    }

    #[test]
    fn devices_are_kept_hashed_in_a_private_file() {
        let d = scratch("devices");
        let p = devices_path(&d);
        let mut devices = Devices::load(&p);
        assert!(devices.is_empty());
        let ip: IpAddr = "192.168.1.30".parse().unwrap();
        let issued = devices.issue("Phone", "UA", ip, 1000).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            !text.contains(&issued.token),
            "the token itself is never written"
        );
        assert!(text.contains(&token_hash(&issued.token)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Another start finds it, and its token still works.
        let mut again = Devices::load(&p);
        assert_eq!(
            again.verify(&issued.token, 1010, ip),
            Some(issued.device.id.clone())
        );
        // A use within the minute is not written; one after it is.
        let on_file = |p: &Path| Devices::load(p).list()[0].last_used;
        assert_eq!(on_file(&p), Some(1000));
        again.verify(&issued.token, 1070, ip);
        assert_eq!(on_file(&p), Some(1070));
        again.revoke_all().unwrap();
        assert!(Devices::load(&p).is_empty());
    }

    #[test]
    fn too_many_devices_are_refused() {
        let mut devices = Devices::in_memory();
        let ip: IpAddr = "10.0.0.2".parse().unwrap();
        for i in 0..MAX_DEVICES {
            devices.issue(&format!("d{i}"), "", ip, 0).unwrap();
        }
        assert!(devices.issue("one more", "", ip, 0).is_err());
    }

    #[test]
    fn passwords_are_hashed_with_argon2id_and_verified() {
        let phc = hash_password("correct horse").unwrap();
        assert!(phc.starts_with("$argon2id$v=19$"), "{phc}");
        assert!(!phc.contains("correct horse"));
        assert!(verify_password(&phc, "correct horse"));
        assert!(!verify_password(&phc, "correct horsE"));
        assert!(!verify_password(&phc, ""));
        assert!(!verify_password("not a hash", "correct horse"));
        assert_ne!(phc, hash_password("correct horse").unwrap(), "salted");
    }

    #[test]
    fn short_or_huge_passwords_are_refused() {
        assert!(password_problem("1234567").is_some());
        assert!(password_problem("        ").is_some());
        assert!(password_problem("12345678").is_none());
        assert!(
            password_problem("ÉÉÉÉÉÉÉÉ").is_none(),
            "characters, not bytes"
        );
        assert!(password_problem(&"x".repeat(MAX_PASSWORD + 1)).is_some());
    }

    #[test]
    fn failed_sign_ins_wait_longer_and_longer() {
        let mut l = Limiter::default();
        let ip: IpAddr = "192.168.1.30".parse().unwrap();
        let other: IpAddr = "192.168.1.31".parse().unwrap();
        let t0 = Instant::now();
        for _ in 0..FREE_TRIES - 1 {
            assert_eq!(l.failed(ip, t0), None);
            assert_eq!(l.check(ip, t0), Ok(()));
        }
        assert_eq!(l.failed(ip, t0), Some(Duration::from_secs(1)));
        assert_eq!(l.check(ip, t0), Err(Duration::from_secs(1)));
        assert_eq!(l.check(other, t0), Ok(()), "per address");
        let t1 = t0 + Duration::from_secs(1);
        assert_eq!(l.check(ip, t1), Ok(()));
        assert_eq!(l.failed(ip, t1), Some(Duration::from_secs(2)));
        assert_eq!(l.failed(ip, t1), Some(Duration::from_secs(4)));
        // It tops out.
        let mut t = t1;
        for _ in 0..30 {
            l.failed(ip, t);
            t += Duration::from_secs(1);
        }
        assert_eq!(l.failed(ip, t), Some(MAX_WAIT));
        // Forgiven a while after the last failure; or at once by a success.
        assert_eq!(l.check(ip, t + MAX_WAIT), Ok(()));
        assert_eq!(l.failed(ip, t + MAX_WAIT), None, "counting starts again");
        l.succeeded(ip);
        for _ in 0..FREE_TRIES - 1 {
            assert_eq!(l.failed(ip, t0), None);
        }
    }

    #[test]
    fn an_ipv6_network_counts_as_one_address() {
        let mut l = Limiter::default();
        let t0 = Instant::now();
        for i in 0..FREE_TRIES {
            let ip: IpAddr = format!("2001:db8:1:2::{}", i + 1).parse().unwrap();
            l.failed(ip, t0);
        }
        assert!(l.check("2001:db8:1:2::99".parse().unwrap(), t0).is_err());
        assert!(l.check("2001:db8:1:3::1".parse().unwrap(), t0).is_ok());
    }

    #[test]
    fn a_device_is_named_after_its_browser() {
        let cases = [
            ("Mozilla/5.0 (Android 14; Mobile; rv:131.0) Gecko/131.0 Firefox/131.0", "Firefox on Android"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0", "Edge on Windows"),
            ("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1", "Safari on iPhone"),
            ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36", "Chrome on Linux"),
            ("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15", "Safari on macOS"),
            ("curl/8.5.0", "A device"),
            ("", "A device"),
        ];
        for (ua, want) in cases {
            assert_eq!(device_name(ua), want, "{ua}");
        }
    }

    #[tokio::test]
    async fn signing_in_needs_the_password_and_a_new_one_signs_everyone_out() {
        let d = scratch("login");
        let a = Access::load(&d);
        let ip: IpAddr = "192.168.1.30".parse().unwrap();
        assert!(matches!(
            a.login(ip, "whatever1", None, "", 0).await,
            Err(LoginError::NoPassword)
        ));
        assert!(a.set_password("short").await.is_err());
        a.set_password("correct horse").await.unwrap();
        assert!(a.password_set());
        let on_file = std::fs::read_to_string(password_path(&d)).unwrap();
        assert!(on_file.starts_with("$argon2id$") && !on_file.contains("correct horse"));
        assert!(matches!(
            a.login(ip, "wrong horse", None, "", 0).await,
            Err(LoginError::Wrong)
        ));
        let ok = a
            .login(
                ip,
                "correct horse",
                None,
                "Mozilla/5.0 (Android 14) Firefox/131.0",
                5,
            )
            .await
            .unwrap();
        assert_eq!(ok.device.name, "Firefox on Android");
        assert_eq!(a.devices(|d| d.len()), 1);
        // Another start knows the password and the device.
        let again = Access::load(&d);
        assert!(again.password_set());
        assert_eq!(
            again.devices(|d| d.verify(&ok.token, 6, ip)),
            Some(ok.device.id.clone())
        );
        a.set_password("battery staple").await.unwrap();
        assert_eq!(
            a.devices(|d| d.len()),
            0,
            "a new password signs every device out"
        );
        assert!(Access::load(&d).devices(|d| d.is_empty()));
    }

    #[tokio::test]
    async fn repeated_wrong_passwords_lock_the_address_out() {
        let a = Access::in_memory();
        a.set_password("correct horse").await.unwrap();
        let ip: IpAddr = "192.168.1.30".parse().unwrap();
        for _ in 0..FREE_TRIES - 1 {
            assert!(matches!(
                a.login(ip, "nope nope", None, "", 0).await,
                Err(LoginError::Wrong)
            ));
        }
        assert!(matches!(
            a.login(ip, "nope nope", None, "", 0).await,
            Err(LoginError::Wait(_))
        ));
        // Locked out: even the right password waits, unchecked.
        assert!(matches!(
            a.login(ip, "correct horse", None, "", 0).await,
            Err(LoginError::Wait(_))
        ));
        // Another address is not.
        let other: IpAddr = "192.168.1.31".parse().unwrap();
        assert!(a.login(other, "correct horse", None, "", 0).await.is_ok());
    }
}
