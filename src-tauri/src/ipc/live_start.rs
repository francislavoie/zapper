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
//! Both measure the burst in stream time — how far the newest data's
//! timestamps are ahead of the playhead — and a stream's timestamps can lie.
//! One that opens with a few seconds stamped ten minutes before the rest
//! reads as ten minutes of data in, and skipping "to live" across that
//! lands in a gap where nothing was ever received. So both also check the
//! depth against the bytes that actually arrived ([`ByteRate`]): a burst the
//! bytes cannot account for is a jump in the timestamps, not data to skip.
//! The rate is the caller's, measured from the moment the file loaded, so
//! that the connect burst and the time before the first frame count too.
//!
//! [`LiveStart::CatchUp`]: crate::ipc::types::LiveStart::CatchUp
//! [`LiveStart::Wait`]: crate::ipc::types::LiveStart::Wait
//! [`LiveStart::Stay`]: crate::ipc::types::LiveStart::Stay

use std::collections::VecDeque;

use crate::ipc::player_cmds::LIVE_CUSHION;

/// One heartbeat's reading of mpv's demuxer cache, as these decisions use
/// it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Reading {
    /// How far the newest data is ahead of the playhead, in stream seconds.
    pub behind: f64,
    /// The newest packet's timestamp (`cache-end`), `None` when mpv has not
    /// said.
    pub cache_end: Option<f64>,
    /// Bytes buffered ahead of the playhead (`fw-bytes`): what `behind` is
    /// made of.
    pub bytes_ahead: Option<i64>,
    /// Bytes in the whole cache (`total-bytes`), which grows with every
    /// packet that arrives while the back buffer has room.
    pub bytes_total: Option<i64>,
}

/// The stream's bytes per second of stream time, learned as it arrives,
/// and what that says about a reading of how far behind live it is.
///
/// Measured heartbeat by heartbeat — the bytes that arrived against how far
/// the newest timestamp moved — and the median taken, so neither a
/// heartbeat carrying one large keyframe nor the one heartbeat whose
/// timestamps jumped (minutes of stream time for a few packets) moves it.
/// A heartbeat that brought nothing is skipped, so a server that delivers
/// in lumps a second apart is measured on the heartbeats its lumps land in.
///
/// One per stream, fed from the moment the file loads: [`Join`] and
/// [`Settle`] are handed it rather than keeping their own, which would start
/// at the first frame with the connect burst already behind them.
#[derive(Debug, Clone, Default)]
pub struct ByteRate {
    /// The last `(cache end, total bytes)`.
    last: Option<(f64, i64)>,
    /// Bytes per stream second, one per heartbeat that brought data.
    steps: VecDeque<f64>,
}

/// Heartbeats the rate is the median of.
const BYTE_RATE_STEPS: usize = 24;
/// Fewer than this and there is no rate yet.
const BYTE_RATE_MIN_STEPS: usize = 3;
/// How many times what the bytes account for a depth may be and still be
/// believed. The rate is a median of short steps, and a keyframe-heavy
/// stream's median step is below its average, which errs towards believing:
/// a real burst reads within a small factor of its bytes, a jump in the
/// timestamps by tens or hundreds.
pub const PLAUSIBLE_FACTOR: f64 = 3.0;
/// And this much more. A real connect burst is five to forty seconds, and
/// on a variable-bitrate channel (sport) the recent steps the rate is
/// measured on can run well above the burst's own: with this much slack no
/// real burst reads as a jump however its bitrate swings, while a jump of
/// minutes still does. Below it a depth is believed with or without a rate.
pub const PLAUSIBLE_SLACK: f64 = 60.0;

/// What the bytes say about a depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Data the bytes received can account for.
    Real,
    /// More than the bytes received could be: a jump in the timestamps.
    Jump,
    /// Too deep to take on trust, and no rate yet to judge it by.
    Unknown,
}

impl ByteRate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one heartbeat's newest timestamp and total bytes.
    pub fn observe(&mut self, cache_end: Option<f64>, bytes_total: Option<i64>) {
        let (Some(end), Some(total)) = (cache_end, bytes_total) else {
            return;
        };
        if let Some((e0, b0)) = self.last {
            let (de, db) = (end - e0, total - b0);
            // Only a heartbeat in which both moved forward: one that brought
            // nothing says nothing, and timestamps that went back (a wrap)
            // or bytes that went down (the back buffer trimmed) are no rate.
            if de > 0.0 && db > 0 {
                self.steps.push_back(db as f64 / de);
                if self.steps.len() > BYTE_RATE_STEPS {
                    self.steps.pop_front();
                }
            }
        }
        self.last = Some((end, total));
    }

    /// Bytes per second of stream time, once there have been a few
    /// heartbeats of data to measure it by.
    pub fn per_second(&self) -> Option<f64> {
        if self.steps.len() < BYTE_RATE_MIN_STEPS {
            return None;
        }
        let mut s: Vec<f64> = self.steps.iter().copied().collect();
        s.sort_by(f64::total_cmp);
        let mid = s.len() / 2;
        Some(if s.len().is_multiple_of(2) {
            (s[mid - 1] + s[mid]) / 2.0
        } else {
            s[mid]
        })
    }

    /// How many seconds of this stream `bytes_ahead` bytes are. `None` with
    /// nothing to judge by.
    pub fn seconds_in(&self, bytes_ahead: Option<i64>) -> Option<f64> {
        let rate = self.per_second().filter(|r| *r > 0.0)?;
        Some(bytes_ahead? as f64 / rate)
    }

    /// Whether `behind` seconds of data in is something the bytes buffered
    /// could be. With no rate or no byte count it is believed, which is how
    /// it was before there was anything to check it against.
    pub fn explains(&self, behind: f64, bytes_ahead: Option<i64>) -> bool {
        plausible(behind, self.seconds_in(bytes_ahead))
    }

    /// [`ByteRate::explains`], telling apart a depth too deep to believe on
    /// trust (past [`PLAUSIBLE_SLACK`]) when there is no rate to judge it by
    /// yet. Within the slack a depth is real with or without one.
    pub fn judge(&self, behind: f64, bytes_ahead: Option<i64>) -> Verdict {
        match self.seconds_in(bytes_ahead) {
            Some(e) if plausible(behind, Some(e)) => Verdict::Real,
            Some(_) => Verdict::Jump,
            None if behind <= PLAUSIBLE_SLACK => Verdict::Real,
            None => Verdict::Unknown,
        }
    }
}

/// Whether a depth of `behind` stream seconds is believable from the
/// `explained` seconds the bytes received account for (`None`: no evidence,
/// so believed).
pub fn plausible(behind: f64, explained: Option<f64>) -> bool {
    explained.is_none_or(|e| behind <= e * PLAUSIBLE_FACTOR + PLAUSIBLE_SLACK)
}

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
///
/// A depth the bytes cannot account for ([`ByteRate::judge`]) is a jump in
/// the timestamps, and is neither growth nor something to seek across: this
/// waits for playback to pass it, when the reading is real again, and if it
/// never does, decides on no seek at all. A depth too deep to take on trust
/// before there is a rate is waited on the same way, and believed at
/// [`SETTLE_MAX`] if nothing has said otherwise by then.
#[derive(Debug, Clone)]
pub struct Settle {
    /// Seconds since playback started.
    age: f64,
    /// The furthest behind live seen so far.
    peak: f64,
    /// Seconds since `peak` last grew.
    quiet: f64,
    /// What the bytes said of the last reading.
    verdict: Verdict,
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
            verdict: Verdict::Real,
        }
    }

    /// Takes one heartbeat (`dt` seconds since the last), its reading, and
    /// the stream's byte rate. `None` while waiting; `Some(by)` once
    /// decided, where `by` is the forward seek to make (zero: none needed).
    pub fn observe(&mut self, dt: f64, r: &Reading, bytes: &ByteRate) -> Option<f64> {
        self.age += dt;
        let behind = r.behind;
        let was = self.verdict;
        self.verdict = bytes.judge(behind, r.bytes_ahead);
        let real = self.verdict == Verdict::Real;
        if real && (was != Verdict::Real || behind > self.peak + SETTLE_GROWTH) {
            // Growth; or the first real reading after ones that were not,
            // which starts the wait over from here, so that a moment
            // misjudged cannot have the wait end in the middle of a burst.
            self.peak = behind;
            self.quiet = 0.0;
        } else {
            self.quiet += dt;
        }
        if self.quiet < SETTLE_QUIET && self.age < SETTLE_MAX {
            return None;
        }
        match self.verdict {
            Verdict::Jump => return (self.age >= SETTLE_MAX).then_some(0.0),
            Verdict::Unknown if self.age < SETTLE_MAX => return None,
            Verdict::Unknown | Verdict::Real => {}
        }
        Some(if behind > LIVE_CUSHION + SETTLE_SLACK {
            behind - LIVE_CUSHION
        } else {
            0.0
        })
    }

    /// Whether the last reading was a depth its bytes could not be. For the
    /// log.
    pub fn saw_phantom(&self) -> bool {
        self.verdict == Verdict::Jump
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
///
/// A depth the bytes cannot account for ([`ByteRate::judge`]) is not
/// skipped: the channel starts where the stream is. One too deep to take on
/// trust before there is a rate to judge it by is held on, up to
/// [`JOIN_MAX`], for the rate; and believed then if there still is none.
#[derive(Debug, Clone)]
pub struct Join {
    /// Seconds since the first frame.
    age: f64,
    /// `(age, cache end)` over the last [`JOIN_WINDOW`] and a little more.
    samples: VecDeque<(f64, f64)>,
    /// Whether anything so far looked like a burst.
    burst: bool,
    /// Whether the depth at the decision was more than its bytes could be.
    phantom: bool,
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
            phantom: false,
        }
    }

    /// Takes one heartbeat: `dt` seconds since the last, its reading
    /// (`behind` being how far the paused playhead is behind live), and the
    /// stream's byte rate. `None` while holding; `Some(by)` to start, `by`
    /// being the forward seek to make first (zero: none).
    pub fn observe(&mut self, dt: f64, r: &Reading, bytes: &ByteRate) -> Option<f64> {
        self.age += dt;
        let behind = r.behind;
        // Already deep at the first frame: the burst arrived before mpv had
        // decoded anything, which on a fast connection is the usual case.
        if behind > LIVE_CUSHION + SETTLE_SLACK {
            self.burst = true;
        }
        let rate = r.cache_end.and_then(|end| self.rate(end));
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
        match bytes.judge(behind, r.bytes_ahead) {
            Verdict::Jump => {
                self.phantom = true;
                return Some(0.0);
            }
            // Deep, and nothing to judge it by yet: hold on for the rate.
            Verdict::Unknown if self.age < JOIN_MAX => return None,
            Verdict::Unknown | Verdict::Real => {}
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

    /// Whether the depth it decided on was more than its bytes could be,
    /// and so was not skipped. For the log.
    pub fn saw_phantom(&self) -> bool {
        self.phantom
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

    /// A reading with only the depth and the newest timestamp: no byte
    /// counts, as from an mpv that does not report them. Everything decides
    /// exactly as it did before there was a byte count to check against.
    fn bare(cache_end: Option<f64>, behind: f64) -> Reading {
        Reading {
            behind,
            cache_end,
            ..Reading::default()
        }
    }

    /// Feeds `Settle` a series of `behind_live` readings a heartbeat apart and
    /// returns the decision and the heartbeat it came on.
    fn settle_run(readings: impl IntoIterator<Item = f64>) -> Option<(usize, f64)> {
        let mut s = Settle::new();
        readings
            .into_iter()
            .enumerate()
            .find_map(|(i, b)| s.observe(DT, &bare(None, b), &ByteRate::new()).map(|by| (i, by)))
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
            if let Some(by) = j.observe(DT, &bare(Some(end), end), &ByteRate::new()) {
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
            if let Some(by) = j.observe(DT, &bare(None, 0.0), &ByteRate::new()) {
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
        assert!(j.observe(DT, &bare(Some(100.0), 30.0), &ByteRate::new()).is_none());
        assert!(j.observe(DT, &bare(Some(110.0), 30.0), &ByteRate::new()).is_none());
        // A wrap: 110 → 2. Not a rate of −400x, and not a calm reading.
        assert!(j.observe(DT, &bare(Some(2.0), 30.0), &ByteRate::new()).is_none());
    }

    // --- the bytes behind a burst ------------------------------------------

    /// Half a megabyte per second of stream: about a 4 Mb/s channel.
    const BPS: f64 = 500_000.0;

    /// A stream as a heartbeat sees it: `real` seconds of data received,
    /// at `BPS`, and timestamps that read `jump` seconds more than that.
    struct Feed {
        /// Stream seconds received, by the bytes.
        real: f64,
        /// How far the timestamps jumped, on top.
        jump: f64,
        /// The playhead, in seconds of data received.
        at: f64,
    }

    impl Feed {
        fn reading(&self) -> Reading {
            Reading {
                behind: self.real + self.jump - self.at,
                cache_end: Some(self.real + self.jump),
                bytes_ahead: Some(((self.real - self.at) * BPS) as i64),
                bytes_total: Some((self.real * BPS) as i64),
            }
        }
    }

    /// The stream's byte rate as the app keeps it, from the file loading:
    /// `depth` seconds arrive over `lead` heartbeats before the first frame,
    /// the timestamps jumping by `jump` after the first of them (a stale
    /// lead-in, then live). Returns the feed as the first frame finds it.
    fn load(depth: f64, jump: f64, lead: usize, bytes: &mut ByteRate) -> Feed {
        let mut f = Feed { real: 0.0, jump: 0.0, at: 0.0 };
        for i in 0..lead {
            f.real += depth / lead as f64;
            if i > 0 {
                f.jump = jump;
            }
            let r = f.reading();
            bytes.observe(r.cache_end, r.bytes_total);
        }
        f.real = depth;
        f.jump = jump;
        f
    }

    /// Delivery after the first frame: `rate(t)` times real time, in lumps
    /// every `lump` seconds if given (a restreamer re-serving segments)
    /// rather than smoothly.
    struct Delivery<R: Fn(f64) -> f64> {
        rate: R,
        lump: Option<f64>,
        owed: f64,
        since: f64,
    }

    impl<R: Fn(f64) -> f64> Delivery<R> {
        fn new(rate: R, lump: Option<f64>) -> Self {
            Self { rate, lump, owed: 0.0, since: 0.0 }
        }

        /// Stream seconds that arrive in the heartbeat ending at `t`.
        fn arrives(&mut self, t: f64) -> f64 {
            self.owed += (self.rate)(t) * DT;
            let Some(lump) = self.lump else {
                return std::mem::take(&mut self.owed);
            };
            self.since += DT;
            if self.since + 1e-9 < lump {
                return 0.0;
            }
            self.since = 0.0;
            std::mem::take(&mut self.owed)
        }
    }

    /// `Join` over a feed: `depth` seconds received over `lead` heartbeats
    /// before the first frame, the timestamps jumping by `jump` after the
    /// first, then delivery as `deliver` says. Paused, so the playhead stays
    /// at zero. Returns when it started, the seek, and the `Join`.
    fn join_feed<R: Fn(f64) -> f64>(
        depth: f64,
        jump: f64,
        lead: usize,
        mut deliver: Delivery<R>,
    ) -> (f64, f64, Join) {
        let mut bytes = ByteRate::new();
        let mut f = load(depth, jump, lead, &mut bytes);
        let mut j = Join::new();
        let mut t = 0.0;
        for _ in 0..200 {
            t += DT;
            f.real += deliver.arrives(t);
            let r = f.reading();
            bytes.observe(r.cache_end, r.bytes_total);
            if let Some(by) = j.observe(DT, &r, &bytes) {
                return (t, by, j);
            }
        }
        panic!("never started");
    }

    fn smooth(rate: impl Fn(f64) -> f64) -> Delivery<impl Fn(f64) -> f64> {
        Delivery::new(rate, None)
    }

    #[test]
    fn a_burst_its_bytes_account_for_starts_at_live_as_before() {
        // The same 24 seconds as without byte counts, and the same answer.
        let (at, by, j) = join_feed(24.0, 0.0, 4, smooth(|_| 1.0));
        let (at0, by0) = join_run(24.0, |_| 1.0);
        assert!(!j.saw_phantom());
        assert_eq!((at, by), (at0, by0));
        // A burst still arriving, likewise.
        let burst = |t: f64| if t <= 4.0 { 5.0 } else { 1.0 };
        let (at, by, j) = join_feed(1.0, 0.0, 4, smooth(burst));
        assert!(!j.saw_phantom());
        assert_eq!((at, by), join_run(1.0, burst));
        // And forty seconds in lumps a second apart.
        let (_, by, j) = join_feed(40.0, 0.0, 4, Delivery::new(|_| 1.0, Some(1.0)));
        assert!(!j.saw_phantom());
        assert!(by > 35.0, "skipped only {by}s of a real burst");
    }

    #[test]
    fn ten_minutes_of_timestamps_on_a_few_seconds_of_bytes_is_not_skipped() {
        // Two seconds received before the first frame, the timestamps
        // jumping ten minutes after the first part of it, then real time:
        // reads as 600-odd seconds in.
        let (_, by, j) = join_feed(2.0, 600.0, 4, smooth(|_| 1.0));
        assert!(j.saw_phantom());
        assert_eq!(by, 0.0, "skipped {by}s of a jump in the timestamps");
    }

    #[test]
    fn the_jump_is_not_skipped_when_the_stream_arrives_in_lumps() {
        // A restreamer sending a second at a time: most heartbeats bring
        // nothing. The rate kept from the file loading has the steps the
        // lead-in brought, so the hold is judged on its first decision.
        let (at, by, j) = join_feed(2.0, 600.0, 4, Delivery::new(|_| 1.0, Some(1.0)));
        assert!(j.saw_phantom());
        assert_eq!(by, 0.0);
        assert!(at < JOIN_MAX, "held {at}s");
    }

    #[test]
    fn a_deep_reading_with_no_rate_yet_is_held_until_there_is_one() {
        // Everything arrived in one lump before the first frame, so there is
        // no step to measure by, and then a second at a time: the hold waits
        // for the steps, and then knows the 600 seconds for a jump.
        let (at, by, j) = join_feed(2.0, 600.0, 1, Delivery::new(|_| 1.0, Some(1.0)));
        assert!(j.saw_phantom());
        assert_eq!(by, 0.0);
        assert!(at > 1.0 && at <= JOIN_MAX, "started at {at}s");
    }

    #[test]
    fn a_deep_reading_never_measured_is_believed_at_the_longest_hold() {
        // No byte counts at all (an mpv that does not report them): held as
        // long as a hold goes, then taken at its word, as before.
        let mut j = Join::new();
        let none = ByteRate::new();
        let mut t = 0.0;
        let by = loop {
            t += DT;
            if let Some(by) = j.observe(DT, &bare(Some(100.0), 100.0), &none) {
                break by;
            }
            assert!(t < JOIN_MAX + 1.0, "held forever");
        };
        assert!((t - JOIN_MAX).abs() <= DT, "started at {t}s");
        assert_eq!(by, 100.0 - LIVE_CUSHION);
        // Within the slack nothing waits for a rate.
        let mut j = Join::new();
        let mut t = 0.0;
        while j.observe(DT, &bare(Some(40.0), 40.0), &none).is_none() {
            t += DT;
        }
        assert!(t < JOIN_MAX - 1.0, "held {t}s for a depth within the slack");
    }

    #[test]
    fn the_jump_arriving_during_the_hold_is_not_skipped_either() {
        // The heartbeat whose timestamps jump is one step of minutes for a
        // few packets; the median of the rest is the stream's real rate.
        let mut bytes = ByteRate::new();
        let mut f = load(1.0, 0.0, 4, &mut bytes);
        let mut j = Join::new();
        let mut decided = None;
        for i in 1..40 {
            f.real += 3.0 * DT;
            if i == 3 {
                f.jump = 600.0;
            }
            if i > 8 {
                f.real -= 2.0 * DT; // real time after two seconds of burst
            }
            let r = f.reading();
            bytes.observe(r.cache_end, r.bytes_total);
            if let Some(by) = j.observe(DT, &r, &bytes) {
                decided = Some(by);
                break;
            }
        }
        assert_eq!(decided, Some(0.0));
        assert!(j.saw_phantom());
    }

    #[test]
    fn the_boundary_is_a_multiple_of_what_the_bytes_hold_and_some_slack() {
        let explained = Some(10.0);
        let edge = 10.0 * PLAUSIBLE_FACTOR + PLAUSIBLE_SLACK;
        assert_eq!(edge, 90.0);
        assert!(plausible(edge, explained));
        assert!(!plausible(edge + 0.01, explained));
        // A buffer within the slack is never what decides.
        assert!(plausible(PLAUSIBLE_SLACK, Some(0.0)));
        assert!(!plausible(PLAUSIBLE_SLACK + 0.01, Some(0.0)));
        // A real forty-second burst whose recent steps run at three times
        // its average bitrate still reads as real, and ten minutes on a few
        // seconds of bytes still does not.
        assert!(plausible(40.0, Some(40.0 / 3.0)));
        assert!(!plausible(600.0, Some(5.0)));
    }

    #[test]
    fn with_no_byte_count_or_too_little_history_the_depth_is_believed() {
        assert!(plausible(600.0, None));
        let mut b = ByteRate::new();
        assert!(b.explains(600.0, Some(1_000)), "no rate yet");
        for i in 0..2 {
            b.observe(Some(f64::from(i)), Some(i64::from(i) * 500_000));
        }
        assert_eq!(b.per_second(), None, "one step is not a rate");
        assert!(b.explains(600.0, Some(1_000)));
        b.observe(Some(2.0), Some(1_000_000));
        b.observe(Some(3.0), Some(1_500_000));
        assert_eq!(b.per_second(), Some(500_000.0));
        assert!(!b.explains(600.0, Some(1_000)));
        // No `fw-bytes` reading: nothing to judge by.
        assert!(b.explains(600.0, None));
        // And no readings at all leave it as it was.
        b.observe(None, Some(9_000_000));
        b.observe(Some(9.0), None);
        assert_eq!(b.per_second(), Some(500_000.0));
    }

    #[test]
    fn judged_with_no_rate_a_depth_past_the_slack_is_unknown() {
        let none = ByteRate::new();
        assert_eq!(none.judge(PLAUSIBLE_SLACK, Some(1_000)), Verdict::Real);
        assert_eq!(none.judge(PLAUSIBLE_SLACK + 1.0, Some(1_000)), Verdict::Unknown);
        assert_eq!(none.judge(600.0, None), Verdict::Unknown);
        let mut b = ByteRate::new();
        for i in 0..4 {
            b.observe(Some(f64::from(i)), Some(i64::from(i) * 500_000));
        }
        assert_eq!(b.judge(600.0, Some(1_000_000)), Verdict::Jump);
        assert_eq!(b.judge(20.0, Some(10_000_000)), Verdict::Real);
        // With a rate but no `fw-bytes`, as with neither.
        assert_eq!(b.judge(600.0, None), Verdict::Unknown);
    }

    #[test]
    fn the_rate_ignores_heartbeats_with_nothing_wrapped_or_trimmed() {
        let mut b = ByteRate::new();
        let mut feed = |end: f64, total: i64| b.observe(Some(end), Some(total));
        feed(1.0, 500_000);
        feed(2.0, 1_000_000);
        feed(2.0, 1_000_000); // nothing arrived
        feed(1.0, 1_200_000); // timestamps went back
        feed(2.0, 1_700_000);
        feed(3.0, 1_600_000); // back buffer trimmed
        feed(4.0, 2_100_000);
        feed(5.0, 2_600_000);
        assert_eq!(b.per_second(), Some(500_000.0));
    }

    /// `Settle` over a playing feed: `depth` seconds in at the first frame
    /// (over four heartbeats before it), timestamps `jump` ahead after the
    /// first `stale` seconds of it, then real time, played at real time.
    /// `tamper` may change a heartbeat's reading.
    fn settle_feed(
        depth: f64,
        stale: f64,
        jump: f64,
        tamper: impl Fn(f64, &mut Reading),
    ) -> (f64, f64, Settle) {
        let mut bytes = ByteRate::new();
        let mut f = load(depth, jump, 4, &mut bytes);
        let mut s = Settle::new();
        let mut t = 0.0;
        for _ in 0..200 {
            t += DT;
            f.real += DT;
            f.at += DT;
            // Once the playhead is past the stale part, the jump is behind it
            // and no longer between it and the newest data.
            let mut r = f.reading();
            if f.at >= stale {
                r.behind -= jump;
            }
            tamper(t, &mut r);
            bytes.observe(r.cache_end, r.bytes_total);
            if let Some(by) = s.observe(DT, &r, &bytes) {
                return (t, by, s);
            }
        }
        panic!("never decided");
    }

    #[test]
    fn settle_catches_up_a_real_burst_as_before() {
        let (at, by, s) = settle_feed(24.0, 0.0, 0.0, |_, _| {});
        assert!(!s.saw_phantom());
        assert!((24.0 - by - LIVE_CUSHION).abs() < 0.3, "jumped {by}");
        assert!(at <= SETTLE_QUIET + 2.0 * DT, "decided at {at}");
    }

    #[test]
    fn settle_does_not_chase_a_jump_and_catches_up_what_is_real_after_it() {
        // Twelve seconds in, the first six stamped ten minutes before the
        // rest: until the playhead passes them it reads 600-odd behind.
        let (at, by, s) = settle_feed(12.0, 6.0, 600.0, |_, _| {});
        assert!(at >= 6.0 + SETTLE_QUIET - 1e-9, "decided at {at}, before the jump was well past");
        assert!(!s.saw_phantom());
        // Then it is twelve seconds behind for real, and goes to the cushion.
        assert!((12.0 - by - LIVE_CUSHION).abs() < 0.3, "jumped {by}");
    }

    #[test]
    fn settle_never_seeks_across_a_jump_it_never_passes() {
        let (at, by, s) = settle_feed(4.0, 1_000.0, 600.0, |_, _| {});
        assert!(s.saw_phantom());
        assert_eq!(by, 0.0);
        assert!((at - SETTLE_MAX).abs() <= DT, "decided at {at}");
    }

    #[test]
    fn a_moment_misjudged_starts_the_wait_over() {
        // A real seventy-second burst, and one heartbeat at 1.25s whose
        // bytes read as next to nothing. The wait restarts from the next
        // real reading rather than ending on the quiet counted through it.
        let (at, by, _) = settle_feed(70.0, 0.0, 0.0, |t, r| {
            if (t - 1.25).abs() < 1e-9 {
                r.bytes_ahead = Some(1);
            }
        });
        assert!(at >= 1.5 + SETTLE_QUIET - 1e-9, "decided at {at}");
        assert!((70.0 - by - LIVE_CUSHION).abs() < 0.3, "jumped {by}");
    }
}
