//! A [`PlayerBackend`] that plays nothing and writes everything down, for
//! tests and the harnesses.
//!
//! Every call is recorded as one line ([`FakeBackend::calls`]). Properties
//! set are remembered and read back by the getters; what [`pump`] reports
//! and what [`probe`] answers are scripted.
//!
//! [`pump`]: PlayerBackend::pump
//! [`probe`]: PlayerBackend::probe

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use anyhow::Result;

use super::backend::{BackendCaps, Health, PlayerBackend, Probe, Signal, Stats};

#[derive(Default)]
struct Inner {
    calls: Vec<String>,
    props: HashMap<String, String>,
    pumps: VecDeque<Vec<Signal>>,
    probe: Probe,
}

/// See the module documentation.
pub struct FakeBackend {
    inner: Mutex<Inner>,
    caps: BackendCaps,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeBackend {
    /// A backend that can do everything mpv can.
    pub fn new() -> Self {
        Self::with_caps(BackendCaps {
            rewind_buffer: true,
            cache_dump: true,
            audio_filters: true,
            caption_geometry: true,
        })
    }

    pub fn with_caps(caps: BackendCaps) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            caps,
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, call: String) {
        self.inner().calls.push(call);
    }

    /// Records `call` and hands back the state, still locked, for the call
    /// to read or change in the same breath.
    fn recorded(&self, call: String) -> MutexGuard<'_, Inner> {
        let mut i = self.inner();
        i.calls.push(call);
        i
    }

    /// Every call so far, oldest first: `set_property pause=yes`,
    /// `command seek 5 relative`, `get_f64 time-pos`, `pump`, ...
    pub fn calls(&self) -> Vec<String> {
        self.inner().calls.clone()
    }

    /// Forgets the calls so far.
    pub fn clear_calls(&self) {
        self.inner().calls.clear();
    }

    /// What the next [`PlayerBackend::pump`] reports. Queued: each pump takes
    /// one batch, and a pump with none queued reports nothing.
    pub fn push_signals(&self, signals: Vec<Signal>) {
        self.inner().pumps.push_back(signals);
    }

    /// What [`PlayerBackend::probe`] answers from now on.
    pub fn set_probe(&self, probe: Probe) {
        self.inner().probe = probe;
    }

    /// A property as the getters will read it, without recording a call.
    pub fn preset(&self, key: &str, value: &str) {
        self.inner().props.insert(key.to_owned(), value.to_owned());
    }

    fn get(&self, what: &str, key: &str) -> Option<String> {
        self.recorded(format!("{what} {key}")).props.get(key).cloned()
    }

    fn set(&self, key: &str, value: String) -> Result<()> {
        let mut i = self.recorded(format!("set_property {key}={value}"));
        i.props.insert(key.to_owned(), value);
        Ok(())
    }
}

impl PlayerBackend for FakeBackend {
    fn capabilities(&self) -> BackendCaps {
        self.caps
    }

    fn load(&self, url: &str) -> Result<()> {
        self.record(format!("load {url}"));
        Ok(())
    }

    fn set_property(&self, key: &str, value: &str) -> Result<()> {
        self.set(key, value.to_owned())
    }

    fn command(&self, name: &str, args: &[&str]) -> Result<()> {
        let line = std::iter::once(name).chain(args.iter().copied()).collect::<Vec<_>>();
        self.record(format!("command {}", line.join(" ")));
        Ok(())
    }

    fn set_flag(&self, key: &str, value: bool) -> Result<()> {
        self.set(key, if value { "yes" } else { "no" }.to_owned())
    }

    fn set_f64(&self, key: &str, value: f64) -> Result<()> {
        self.set(key, value.to_string())
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        self.get("get_f64", key)?.parse().ok()
    }

    fn get_i64(&self, key: &str) -> Option<i64> {
        self.get("get_i64", key)?.parse().ok()
    }

    fn get_bool(&self, key: &str) -> Option<bool> {
        match self.get("get_bool", key)?.as_str() {
            "yes" | "true" => Some(true),
            "no" | "false" => Some(false),
            _ => None,
        }
    }

    fn get_string(&self, key: &str) -> Option<String> {
        self.get("get_string", key).filter(|s| !s.is_empty())
    }

    fn pump(&self) -> Vec<Signal> {
        self.recorded("pump".to_owned()).pumps.pop_front().unwrap_or_default()
    }

    fn probe(&self) -> Probe {
        self.recorded("probe".to_owned()).probe.clone()
    }

    fn stats(&self) -> Stats {
        self.record("stats".to_owned());
        Stats::default()
    }

    fn reading(&self) -> crate::catalog::store::Reading {
        self.record("reading".to_owned());
        crate::catalog::store::Reading::default()
    }

    fn health(&self) -> Health {
        self.record("health".to_owned());
        Health::default()
    }

    fn dump_cache(&self, start: f64, end: f64, path: &Path) -> Result<()> {
        self.record(format!("dump_cache {start} {end} {}", path.display()));
        Ok(())
    }
}
