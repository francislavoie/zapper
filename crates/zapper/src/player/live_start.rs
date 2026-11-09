//! Getting a freshly tuned channel to live.
//!
//! An IPTV server answers a new connection with a burst of what it already
//! holds — commonly twenty or thirty seconds of video, faster than real
//! time — and mpv shows the first frame it gets, which is the oldest. Left
//! alone, a channel starts that far in the past.
//!
//! Two answers live here, both pure so every case is a unit test, and both
//! shared with `bin/live_start_bench`, which measures them against the
//! fixture server:
//!
//! - [`Settle`] lets the picture run from the old data and then jumps it to
//!   live once, after the burst: a picture at once, and one jump. The
//!   default ([`LiveStart::CatchUp`]), and the safety net behind [`Join`]
//!   when a burst outlasts it.
//! - [`Join`] keeps the picture from running at all until the burst is in:
//!   the channel opens paused on its first frame, and once the data stops
//!   racing ahead of the clock it is started at live, so the first moving
//!   picture is already live ([`LiveStart::Wait`]).
//!
//! The third choice, [`LiveStart::Stay`], uses neither: the picture plays
//! from where the stream starts and nothing skips it ahead.
//!
//! [`LiveStart::CatchUp`]: crate::ipc::types::LiveStart::CatchUp
//! [`LiveStart::Wait`]: crate::ipc::types::LiveStart::Wait
//! [`LiveStart::Stay`]: crate::ipc::types::LiveStart::Stay

use std::collections::VecDeque;

use super::LIVE_CUSHION;

/// Brings a channel that started in the past up to live once its connect
/// burst is over.
///
/// The automatic catch-up only answers time *lost* to stalls — deliberately,
/// since chasing the edge while the burst was still arriving would seek,
/// wait, and seek again: a stutter-jump-stutter at the start of every zap.
///
/// So this waits for the burst to finish — `behind_live` stops growing for
/// [`SETTLE_QUIET`] — or for [`SETTLE_MAX`] if it never visibly does, and
/// then makes one decision: jump to the live cushion if playback is more
/// than [`SETTLE_SLACK`] past it, otherwise nothing. One seek, after the
/// data has arrived, so there is nothing for it to stutter over.
#[derive(Debug, Clone, Copy)]
pub struct Settle {
    /// Seconds since playback started.
    age: f64,
    /// The furthest behind live seen so far.
    peak: f64,
    /// Seconds since `peak` last grew.
    quiet: f64,
}

/// How long `behind_live` must stop growing for the burst to count as over.
pub const SETTLE_QUIET: f64 = 1.5;
/// The longest to wait for that before deciding anyway.
pub const SETTLE_MAX: f64 = 10.0;
/// How far past the cushion is worth a seek. Less than this is the
/// sawtooth of an ordinary live stream, not a burst's backlog.
pub const SETTLE_SLACK: f64 = 2.0;
/// Growth smaller than this is the seekable end ticking over, not the burst.
const SETTLE_GROWTH: f64 = 0.5;

impl Default for Settle {
    fn default() -> Self {
        Self::new()
    }
}

impl Settle {
    pub fn new() -> Self {
        Self {
            age: 0.0,
            peak: 0.0,
            quiet: 0.0,
        }
    }

    /// Takes one heartbeat (`dt` seconds since the last) and the current
    /// distance behind live. `None` while waiting; `Some(by)` once decided,
    /// where `by` is the forward seek to make (zero: none needed).
    pub fn observe(&mut self, dt: f64, behind: f64) -> Option<f64> {
        self.age += dt;
        if behind > self.peak + SETTLE_GROWTH {
            self.peak = behind;
            self.quiet = 0.0;
        } else {
            self.quiet += dt;
        }
        if self.quiet < SETTLE_QUIET && self.age < SETTLE_MAX {
            return None;
        }
        Some(if behind > LIVE_CUSHION + SETTLE_SLACK {
            behind - LIVE_CUSHION
        } else {
            0.0
        })
    }
}

/// Holds a freshly tuned channel on its first frame until the connect burst
/// is in, then says how far forward to start it.
///
/// The burst is read off the newest data mpv holds — the end of its demuxer
/// cache, in stream time — against the wall clock: while that runs ahead at
/// several times real time the server is still emptying its buffer, and once
/// it advances at about one second per second the server is sending what is
/// happening now. Measuring the end itself rather than the distance behind it
/// is what makes this work paused, when the playhead does not move.
///
/// A server that sends no burst must not cost a wait: real-time delivery is
/// recognised within [`JOIN_WINDOW`], and anything short of a burst after
/// [`JOIN_NO_BURST`] starts. A burst that never ends — or a server that
/// delivers faster than real time all along — is cut off at [`JOIN_MAX`],
/// and [`Settle`] takes over from there.
#[derive(Debug, Clone)]
pub struct Join {
    /// Seconds since the first frame.
    age: f64,
    /// `(age, cache end)` over the last [`JOIN_WINDOW`] and a little more.
    samples: VecDeque<(f64, f64)>,
    /// Whether anything so far looked like a burst.
    burst: bool,
}

/// The span the delivery rate is measured over. Long enough that a stream
/// delivered in uneven lumps still averages out to its rate, short enough
/// that real-time delivery is recognised before anybody waits on it.
pub const JOIN_WINDOW: f64 = 0.75;
/// Delivery at least this many times real time is a burst.
pub const JOIN_BURST_RATE: f64 = 1.6;
/// Delivery at most this many times real time is the server keeping pace:
/// the burst, if there was one, is over.
pub const JOIN_CALM_RATE: f64 = 1.25;
/// With nothing that looks like a burst by now, start.
pub const JOIN_NO_BURST: f64 = 1.25;
/// The longest a channel is held, burst or no burst.
pub const JOIN_MAX: f64 = 6.0;
/// How far past the cushion is worth the seek. Small, unlike
/// [`SETTLE_SLACK`]: the picture is paused, so the seek is unseen and costs
/// a few tens of milliseconds.
const JOIN_SLACK: f64 = 0.5;

impl Default for Join {
    fn default() -> Self {
        Self::new()
    }
}

impl Join {
    pub fn new() -> Self {
        Self {
            age: 0.0,
            samples: VecDeque::new(),
            burst: false,
        }
    }

    /// Takes one heartbeat: `dt` seconds since the last, the end of mpv's
    /// cache in stream time (`None` when mpv has not said), and how far the
    /// paused playhead is behind live. `None` while holding; `Some(by)` to
    /// start, `by` being the forward seek to make first (zero: none).
    pub fn observe(&mut self, dt: f64, cache_end: Option<f64>, behind: f64) -> Option<f64> {
        self.age += dt;
        // Already deep at the first frame: the burst arrived before mpv had
        // decoded anything, which on a fast connection is the usual case.
        if behind > LIVE_CUSHION + SETTLE_SLACK {
            self.burst = true;
        }
        let rate = cache_end.and_then(|end| self.rate(end));
        if rate.is_some_and(|r| r >= JOIN_BURST_RATE) {
            self.burst = true;
        }
        let calm = rate.is_some_and(|r| r <= JOIN_CALM_RATE);
        let decided = calm
            || (!self.burst && self.age >= JOIN_NO_BURST)
            || self.age >= JOIN_MAX;
        if !decided {
            return None;
        }
        Some(if behind > LIVE_CUSHION + JOIN_SLACK {
            behind - LIVE_CUSHION
        } else {
            0.0
        })
    }

    /// Whether anything so far looked like a burst. For the log.
    pub fn saw_burst(&self) -> bool {
        self.burst
    }

    /// Records a reading and returns the delivery rate over the last
    /// [`JOIN_WINDOW`], once there is that much history.
    fn rate(&mut self, end: f64) -> Option<f64> {
        // A stream whose timestamps jump backwards (a wrap, a splice) has
        // started a new timeline; what came before says nothing about it.
        if self.samples.back().is_some_and(|&(_, last)| end < last) {
            self.samples.clear();
        }
        self.samples.push_back((self.age, end));
        // Keep exactly one sample at or before the window's start, to
        // measure from.
        while self.samples.len() > 2 && self.samples[1].0 <= self.age - JOIN_WINDOW {
            self.samples.pop_front();
        }
        let &(t0, e0) = self.samples.front()?;
        let span = self.age - t0;
        if span < JOIN_WINDOW - 1e-9 {
            return None;
        }
        Some((end - e0) / span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 0.25;

    /// Feeds `Settle` a series of `behind_live` readings a heartbeat apart and
    /// returns the decision and the heartbeat it came on.
    fn settle_run(readings: impl IntoIterator<Item = f64>) -> Option<(usize, f64)> {
        let mut s = Settle::new();
        readings
            .into_iter()
            .enumerate()
            .find_map(|(i, b)| s.observe(DT, b).map(|by| (i, by)))
    }

    #[test]
    fn a_tune_in_burst_is_caught_up_once_it_has_finished() {
        // The burst: behind_live climbs to 24s over two seconds, then holds
        // (real-time delivery, the seekable end ticking over by tenths).
        let mut readings: Vec<f64> = (1..=8).map(|i| f64::from(i) * 3.0).collect();
        readings.extend((0..40).map(|i| 24.0 + f64::from(i % 3) * 0.1));
        let (at, by) = settle_run(readings).expect("never decided");
        // Not during the climb: the seek waits for the data to have arrived.
        assert!(at >= 8 + 5, "decided at heartbeat {at}, while the burst was arriving");
        // Lands on the cushion.
        assert!((24.0 - by - LIVE_CUSHION).abs() < 0.2, "jumped by {by}");
    }

    #[test]
    fn a_tune_in_already_near_live_is_left_alone() {
        let readings = std::iter::repeat_n(LIVE_CUSHION + 1.0, 40);
        assert_eq!(settle_run(readings).map(|(_, by)| by), Some(0.0));
    }

    #[test]
    fn a_burst_that_never_settles_is_decided_anyway() {
        // Still growing at SETTLE_MAX: decide then rather than wait forever.
        let readings = (1..=200).map(|i| f64::from(i) * 0.6);
        let (at, by) = settle_run(readings).expect("never decided");
        assert!((at as f64 + 1.0) * DT >= SETTLE_MAX - DT);
        assert!(by > 0.0);
    }

    /// A paused channel, one heartbeat at a time: the cache end advances at
    /// `rate(t)` times real time, starting `depth` seconds ahead of the
    /// playhead (what arrived before the first frame). Returns when it
    /// started, in seconds after the first frame, and the seek it made.
    fn join_run(depth: f64, rate: impl Fn(f64) -> f64) -> (f64, f64) {
        let mut j = Join::new();
        let mut end = depth;
        let mut t = 0.0;
        for _ in 0..200 {
            t += DT;
            end += rate(t) * DT;
            // Paused at zero, so the distance behind is the end itself.
            if let Some(by) = j.observe(DT, Some(end), end) {
                return (t, by);
            }
        }
        panic!("never started");
    }

    #[test]
    fn a_server_with_no_burst_starts_without_a_needless_wait() {
        // One second per second from the start: recognised as soon as there
        // is a window of it to measure, and no seek.
        let (at, by) = join_run(0.4, |_| 1.0);
        assert!(at <= JOIN_WINDOW + DT + 1e-9, "held {at}s for no burst");
        assert_eq!(by, 0.0);
    }

    #[test]
    fn a_burst_that_arrived_before_the_first_frame_starts_at_once_at_live() {
        // 24 seconds already in when the first frame decoded, then real time.
        let (at, by) = join_run(24.0, |_| 1.0);
        assert!(at <= JOIN_WINDOW + DT + 1e-9, "held {at}s");
        let lands = 24.0 + at - by;
        assert!((lands - LIVE_CUSHION).abs() < 0.3, "started {lands}s behind");
    }

    #[test]
    fn a_burst_still_arriving_is_waited_out_and_then_started_at_live() {
        // 20 seconds delivered at 5x after the first frame — four seconds of
        // racing — then real time.
        let (at, by) = join_run(1.0, |t| if t <= 4.0 { 5.0 } else { 1.0 });
        assert!(at > 4.0, "started at {at}s, while the burst was arriving");
        assert!(at <= 4.0 + JOIN_WINDOW + 2.0 * DT, "held {at}s, long after it ended");
        let behind = 1.0 + 4.0 * 5.0 + (at - 4.0) - by;
        assert!((behind - LIVE_CUSHION).abs() < 0.3, "started {behind}s behind");
    }

    #[test]
    fn a_burst_that_does_not_end_is_cut_off() {
        let (at, by) = join_run(0.0, |_| 3.0);
        assert!((at - JOIN_MAX).abs() <= DT, "held {at}s");
        assert!(by > 0.0, "and still started as near live as it could");
    }

    #[test]
    fn a_slow_trickle_is_not_waited_on() {
        // Under real time (a starved connection): nothing to wait for.
        let (at, _) = join_run(0.5, |_| 0.6);
        assert!(at <= JOIN_NO_BURST + DT, "held {at}s");
    }

    #[test]
    fn a_lumpy_real_time_server_is_not_mistaken_for_a_burst() {
        // Two seconds of video every two seconds, delivered in one lump:
        // real time on average, never smoothly.
        let (at, by) = join_run(0.5, |t| if (t / DT) as i64 % 8 == 0 { 8.0 } else { 0.0 });
        assert!(at <= JOIN_NO_BURST + DT, "held {at}s");
        assert!(by < 3.0);
    }

    #[test]
    fn no_reading_from_mpv_still_starts() {
        let mut j = Join::new();
        let mut t = 0.0;
        let decided = loop {
            t += DT;
            if let Some(by) = j.observe(DT, None, 0.0) {
                break by;
            }
            assert!(t < JOIN_MAX + 1.0, "held forever with no reading");
        };
        assert_eq!(decided, 0.0);
        assert!(t <= JOIN_NO_BURST + DT);
    }

    #[test]
    fn timestamps_that_jump_back_start_the_measurement_again() {
        let mut j = Join::new();
        assert!(j.observe(DT, Some(100.0), 30.0).is_none());
        assert!(j.observe(DT, Some(110.0), 30.0).is_none());
        // A wrap: 110 → 2. Not a rate of −400x, and not a calm reading.
        assert!(j.observe(DT, Some(2.0), 30.0).is_none());
    }
}
