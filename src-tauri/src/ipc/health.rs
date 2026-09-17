//! The rolling record behind the diagnostics graphs.
//!
//! A history that started when the panel was opened would start at precisely
//! the moment it is least useful: a stutter is a shape over time, and somebody
//! opens the panel *because* something just happened. Three empty boxes and a
//! thirty-second wait is the wrong answer to "what was that?".
//!
//! So the record is kept whether or not anybody is looking. It rides the
//! player heartbeat that already runs, so it costs no timer of its own and no
//! IPC at all while the panel is closed — four mpv property reads on a tick
//! that was going to happen regardless. The full `Stats` read, which is
//! twenty-odd properties and all the static text about codecs and colour,
//! stays where it was: on demand, when the panel is actually open.
//!
//! What it holds is deltas and levels, not running totals. A dropped-frame
//! counter only ever climbs, which draws a straight line and says nothing
//! about *when* the frames were dropped; the difference between consecutive
//! readings is the thing worth plotting.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;

/// How many readings the graphs hold.
///
/// Mirrored by `HISTORY_LENGTH` in `src/logic/stats.ts`, which sizes the
/// horizontal axis: a sample is one slot wide there, so a ring of a different
/// depth would draw a line that does not reach the edge of its box.
pub const HISTORY_LENGTH: usize = 120;

/// Heartbeats per recorded sample.
///
/// The heartbeat is 250ms and this makes a sample every 500ms, which at
/// `HISTORY_LENGTH` is one minute of history — long enough to show a buffer
/// draining, short enough to still be about now.
///
/// Decimated rather than recorded every tick because the two rates have to
/// agree: the graph plots samples at even spacing and cannot know that some
/// of them were twice as close together as others.
pub const TICKS_PER_SAMPLE: u32 = 2;

/// The four numbers a graph is drawn from, as the frontend receives them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleDto {
    /// Seconds of demuxed data ahead of the playhead.
    pub buffer: f64,
    /// Bytes per second arriving.
    pub speed: f64,
    /// Frames dropped since the previous sample.
    pub dropped: i64,
    /// Whether playback was stopped waiting for data at this instant.
    pub starved: bool,
}

/// One raw reading, before it is turned into a sample.
///
/// Separate from `SampleDto` because the dropped-frame figure arrives as a
/// running total and leaves as a difference, and because keeping the reading
/// plain makes the recorder testable without an mpv.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Reading {
    pub cache_duration: Option<f64>,
    pub cache_speed: Option<f64>,
    pub dropped_total: i64,
    pub buffering: bool,
}

#[derive(Debug, Default)]
pub struct Recorder {
    samples: VecDeque<SampleDto>,
    /// `None` until the first reading of a stream, which therefore reports no
    /// drops rather than reporting the whole counter as having happened at
    /// once.
    last_dropped: Option<i64>,
    ticks: u32,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes a heartbeat's reading, recording a sample every
    /// `TICKS_PER_SAMPLE`th one. Returns whether it recorded.
    pub fn observe(&mut self, r: Reading) -> bool {
        self.ticks += 1;
        if self.ticks < TICKS_PER_SAMPLE {
            return false;
        }
        self.ticks = 0;

        // A negative difference means the counter went backwards, which is a
        // new file rather than negative drops.
        let dropped = match self.last_dropped {
            Some(before) => (r.dropped_total - before).max(0),
            None => 0,
        };
        self.last_dropped = Some(r.dropped_total);

        if self.samples.len() >= HISTORY_LENGTH {
            self.samples.pop_front();
        }
        self.samples.push_back(SampleDto {
            buffer: r.cache_duration.unwrap_or(0.0).max(0.0),
            speed: r.cache_speed.unwrap_or(0.0).max(0.0),
            dropped,
            starved: r.buffering,
        });
        true
    }

    pub fn samples(&self) -> Vec<SampleDto> {
        self.samples.iter().copied().collect()
    }

    /// Forgets everything, for a new stream.
    ///
    /// The previous channel's buffer curve plotted against this one's is not
    /// a longer history, it is a misleading one — and the frame counters
    /// start again from zero, so the deltas across the join would be wrong
    /// as well.
    pub fn clear(&mut self) {
        self.samples.clear();
        self.last_dropped = None;
        self.ticks = 0;
    }
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Recorder) -> T) -> T {
    let mut guard = RECORDER.lock().expect("health recorder mutex poisoned");
    f(guard.get_or_insert_with(Recorder::new))
}

pub fn observe(r: Reading) {
    with(|rec| {
        rec.observe(r);
    });
}

pub fn samples() -> Vec<SampleDto> {
    with(|rec| rec.samples())
}

pub fn clear() {
    with(Recorder::clear);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(buffer: f64, dropped_total: i64) -> Reading {
        Reading {
            cache_duration: Some(buffer),
            cache_speed: Some(1000.0),
            dropped_total,
            buffering: false,
        }
    }

    /// Feeds `n` heartbeats, so a test reads in heartbeats rather than in
    /// samples and the decimation is part of what is under test.
    fn beat(rec: &mut Recorder, n: usize, r: Reading) {
        for _ in 0..n {
            rec.observe(r);
        }
    }

    #[test]
    fn one_sample_per_two_heartbeats() {
        let mut rec = Recorder::new();
        assert!(!rec.observe(reading(1.0, 0)));
        assert!(rec.observe(reading(1.0, 0)));
        assert_eq!(rec.samples().len(), 1);
        beat(&mut rec, 6, reading(1.0, 0));
        assert_eq!(rec.samples().len(), 4);
    }

    #[test]
    fn the_first_sample_of_a_stream_reports_no_drops() {
        let mut rec = Recorder::new();
        // A counter that is already high because the stream has been running:
        // none of those frames were dropped in the last half second.
        beat(&mut rec, 2, reading(1.0, 5000));
        assert_eq!(rec.samples()[0].dropped, 0);
    }

    #[test]
    fn drops_are_reported_as_the_difference() {
        let mut rec = Recorder::new();
        beat(&mut rec, 2, reading(1.0, 100));
        beat(&mut rec, 2, reading(1.0, 112));
        beat(&mut rec, 2, reading(1.0, 112));
        let s = rec.samples();
        assert_eq!(s[0].dropped, 0);
        assert_eq!(s[1].dropped, 12);
        assert_eq!(s[2].dropped, 0);
    }

    #[test]
    fn a_counter_that_went_backwards_is_a_new_file_not_negative_drops() {
        let mut rec = Recorder::new();
        beat(&mut rec, 2, reading(1.0, 900));
        beat(&mut rec, 2, reading(1.0, 3));
        assert_eq!(rec.samples()[1].dropped, 0);
    }

    #[test]
    fn the_ring_keeps_the_newest_and_no_more_than_its_depth() {
        let mut rec = Recorder::new();
        for i in 0..(HISTORY_LENGTH * 3) {
            beat(&mut rec, 2, reading(i as f64, 0));
        }
        let s = rec.samples();
        assert_eq!(s.len(), HISTORY_LENGTH);
        // Oldest first, newest last: the graph draws them in that order.
        assert_eq!(s.last().unwrap().buffer, (HISTORY_LENGTH * 3 - 1) as f64);
        assert_eq!(s[0].buffer, (HISTORY_LENGTH * 2) as f64);
    }

    #[test]
    fn missing_properties_read_as_zero_rather_than_as_a_gap() {
        // Before the first frame mpv has neither figure, and a graph cannot
        // draw an absence. Zero is the truth here: nothing is buffered.
        let mut rec = Recorder::new();
        beat(&mut rec, 2, Reading::default());
        let s = rec.samples()[0];
        assert_eq!(s.buffer, 0.0);
        assert_eq!(s.speed, 0.0);
    }

    #[test]
    fn a_negative_buffer_is_clamped() {
        let mut rec = Recorder::new();
        beat(&mut rec, 2, reading(-4.0, 0));
        assert_eq!(rec.samples()[0].buffer, 0.0);
    }

    #[test]
    fn clearing_forgets_the_history_and_the_counter_with_it() {
        let mut rec = Recorder::new();
        beat(&mut rec, 2, reading(1.0, 100));
        beat(&mut rec, 2, reading(1.0, 150));
        rec.clear();
        assert!(rec.samples().is_empty());
        // The new stream's counter starts again, and the first sample after a
        // clear must not read that as a rush of drops.
        beat(&mut rec, 2, reading(1.0, 0));
        assert_eq!(rec.samples()[0].dropped, 0);
    }

    #[test]
    fn starvation_is_carried_through() {
        let mut rec = Recorder::new();
        beat(
            &mut rec,
            2,
            Reading {
                buffering: true,
                ..reading(0.0, 0)
            },
        );
        assert!(rec.samples()[0].starved);
    }
}
