//! Watch time, and the ranking that turns it into favourites.
//!
//! Favourites here are earned, never chosen: there is no "add to favourites"
//! control anywhere in the application, and this module is the whole reason
//! that works. Two pure pieces:
//!
//!   - [`Accumulator`], which decides how much of what the player is doing
//!     counts as watching, and when that is worth a write;
//!   - [`score`] and [`rank`], which decide the order the list comes back in.
//!
//! Neither touches SQLite. The statements live in `catalog::store` beside the
//! rest of the schema's SQL, so that what gets *counted* can be tested without
//! a database and what gets *stored* can be tested without a player.

use crate::ipc::types::FavouriteDto;

/// Seconds a channel must be watched, in one sitting, before it earns
/// anything at all.
///
/// Without a floor, zapping through twenty channels to find something would
/// create twenty favourites, and the list would be a record of indecision
/// rather than of viewing. Thirty seconds is comfortably longer than it takes
/// to see a channel is not what you wanted and comfortably shorter than
/// actually watching something.
pub const FLOOR_SECONDS: f64 = 30.0;

/// How much unwritten watch time may pile up before it is flushed.
///
/// The player ticks four times a second and a write per tick would be absurd;
/// a write per session would mean a crash costs the whole evening. This is the
/// price of a crash, in seconds.
pub const FLUSH_SECONDS: f64 = 30.0;

/// Half-life of the recency weighting: a fortnight.
///
/// The requirement is that a channel binged in March does not outrank
/// one watched all week, which is a statement about the *ratio* between two
/// channels rather than about either one. Two weeks puts the crossover in
/// roughly the right place — see the tests, which pin it.
pub const HALF_LIFE_SECONDS: f64 = 14.0 * 86_400.0;

/// Watch time that has been earned and is ready to be written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Earned {
    pub stream_id: i64,
    /// Whole seconds to add to the running total.
    pub seconds: i64,
    /// True exactly once per open, the moment the floor is crossed. A channel
    /// abandoned before then is never opened as far as this table is
    /// concerned.
    pub open: bool,
    /// Unix seconds, stamped as `last_watched`.
    pub at: i64,
}

/// Turns a stream of player ticks into occasional writes.
///
/// Fed from the player's existing 250ms heartbeat. It counts only while
/// something is genuinely playing: `Opening` is not watching, a paused stream
/// is not watching, and a stopped one certainly is not.
#[derive(Debug, Default)]
pub struct Accumulator {
    /// The channel the ticks are currently about.
    current: Option<i64>,
    /// Seconds watched since this channel opened, written or not. What the
    /// floor is measured against.
    session: f64,
    /// Seconds watched and not yet written down.
    pending: f64,
    /// Whether this open has already been counted in `open_count`.
    counted: bool,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// One tick. `dt` is the tick interval in seconds, `now` is Unix seconds.
    ///
    /// Returns a write when one is due: on crossing the flush interval, and on
    /// a channel change, which is the moment a session's remainder would
    /// otherwise be lost.
    pub fn observe(
        &mut self,
        stream_id: Option<i64>,
        playing: bool,
        dt: f64,
        now: i64,
    ) -> Option<Earned> {
        if stream_id != self.current {
            // Flush what the *previous* channel earned before forgetting it.
            let flushed = self.take(now);
            self.current = stream_id;
            self.session = 0.0;
            self.pending = 0.0;
            self.counted = false;
            if playing {
                self.accrue(dt);
            }
            return flushed;
        }
        if !playing {
            return None;
        }
        self.accrue(dt);
        if self.session >= FLOOR_SECONDS && self.pending >= FLUSH_SECONDS {
            return self.take(now);
        }
        None
    }

    /// Writes out whatever is owed, if anything. For a clean shutdown, and the
    /// mechanism behind the flush on channel change.
    pub fn take(&mut self, now: i64) -> Option<Earned> {
        let stream_id = self.current?;
        // Below the floor nothing is owed, however many ticks went by: a
        // channel that was open for two seconds earns nothing at all.
        if self.session < FLOOR_SECONDS {
            return None;
        }
        let seconds = self.pending.round() as i64;
        if seconds <= 0 {
            return None;
        }
        // The fraction is kept rather than dropped, so a long session does not
        // lose half a second per flush.
        self.pending -= seconds as f64;
        let open = !self.counted;
        self.counted = true;
        Some(Earned {
            stream_id,
            seconds,
            open,
            at: now,
        })
    }

    fn accrue(&mut self, dt: f64) {
        // `is_finite` first, so a NaN tick interval is rejected rather than
        // slipping past a comparison that is false either way.
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.session += dt;
        self.pending += dt;
    }
}

/// Recency-weighted watch time. Higher is more of a favourite.
///
/// Total seconds decayed by an exponential half-life, so an hour watched
/// today counts for the same as two hours watched a fortnight ago. Pure, and
/// deliberately total: every awkward input has a defined answer rather than a
/// panic.
///
///   - A channel that has never been watched scores zero, not a small number.
///   - A `last_watched` in the future — clocks do change, and the database
///     outlives any one boot — is treated as "now" rather than being allowed
///     to amplify the score.
///   - Nothing here can overflow: `0.5^x` for large `x` underflows to zero,
///     which is the right answer anyway, and the result is clamped to a
///     finite, non-negative number.
pub fn score(watch_seconds: i64, last_watched: Option<i64>, now: i64, half_life: f64) -> f64 {
    if watch_seconds <= 0 || !half_life.is_finite() || half_life <= 0.0 {
        return 0.0;
    }
    let Some(last) = last_watched else {
        return 0.0;
    };
    let age = now.saturating_sub(last).max(0) as f64;
    let decayed = watch_seconds as f64 * 0.5_f64.powf(age / half_life);
    if decayed.is_finite() && decayed > 0.0 {
        decayed
    } else {
        0.0
    }
}

/// [`score`] at the module's own half-life, for one row.
pub fn score_of(f: &FavouriteDto, now: i64) -> f64 {
    score(f.watch_seconds, f.last_watched, now, HALF_LIFE_SECONDS)
}

/// The best `limit` of what was watched, most favourite first.
///
/// Rows that score zero are dropped rather than trailing the list: a channel
/// with no watch time at all is not a weak favourite, it is not a favourite.
/// Ties break on recency, then on total time, then on id — the last of those
/// only so that the order is stable and two calls a millisecond apart cannot
/// shuffle the list under the cursor.
pub fn rank(rows: Vec<FavouriteDto>, now: i64, limit: usize) -> Vec<FavouriteDto> {
    // Scored once each and carried, rather than recomputed inside the
    // comparator: `powf` is not free and a comparator must be consistent.
    let mut scored: Vec<(f64, FavouriteDto)> = rows
        .into_iter()
        .map(|f| (score_of(&f, now), f))
        .filter(|(s, _)| *s > 0.0)
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.total_cmp(sa)
            .then(b.last_watched.cmp(&a.last_watched))
            .then(b.watch_seconds.cmp(&a.watch_seconds))
            .then(a.channel.stream_id.cmp(&b.channel.stream_id))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, f)| f).collect()
}

/// Unix seconds. Zero if the clock is before the epoch, which is not a
/// situation worth a `Result` on this path.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::ChannelDto;

    const DAY: i64 = 86_400;
    const HOUR: i64 = 3_600;

    fn fav(stream_id: i64, watch_seconds: i64, last_watched: Option<i64>) -> FavouriteDto {
        FavouriteDto {
            channel: ChannelDto {
                stream_id,
                name: format!("Channel {stream_id}"),
                clean_name: format!("Channel {stream_id}"),
                num: Some(stream_id),
                num_label: None,
                logo_url: None,
                category_id: None,
                has_archive: false,
                archive_days: None,
                is_separator: false,
                quality: None,
                now_playing: None,
            },
            watch_seconds,
            last_watched,
            open_count: 1,
        }
    }

    // ------------------------------------------------------------- scoring

    #[test]
    fn a_channel_never_watched_scores_zero() {
        assert_eq!(score(0, None, 1_000_000, HALF_LIFE_SECONDS), 0.0);
        // Stamped but never accumulated, and accumulated but never stamped:
        // both are "not watched", not "watched a very long time ago".
        assert_eq!(score(0, Some(1_000_000), 1_000_000, HALF_LIFE_SECONDS), 0.0);
        assert_eq!(score(3600, None, 1_000_000, HALF_LIFE_SECONDS), 0.0);
        assert_eq!(score(-5, Some(999_999), 1_000_000, HALF_LIFE_SECONDS), 0.0);
    }

    #[test]
    fn watched_just_now_is_worth_its_full_time() {
        let now = 1_700_000_000;
        let s = score(3600, Some(now), now, HALF_LIFE_SECONDS);
        assert!((s - 3600.0).abs() < 1e-6, "expected ~3600, got {s}");
    }

    #[test]
    fn one_half_life_halves_the_score() {
        let now = 1_700_000_000;
        let then = now - 14 * DAY;
        let s = score(3600, Some(then), now, HALF_LIFE_SECONDS);
        assert!((s - 1800.0).abs() < 1e-3, "expected ~1800, got {s}");
        // And two half-lives quarter it, which is the property that makes
        // "binged in March" fall away rather than merely rank lower.
        let s2 = score(3600, Some(now - 28 * DAY), now, HALF_LIFE_SECONDS);
        assert!((s2 - 900.0).abs() < 1e-3, "expected ~900, got {s2}");
    }

    #[test]
    fn recent_and_brief_overtakes_old_and_long_at_the_crossover() {
        let now = 1_700_000_000;
        // Twenty hours, binged eight weeks ago: four half-lives, so 1/16 of it
        // survives — an effective 1h15m.
        let binged = score(20 * HOUR, Some(now - 56 * DAY), now, HALF_LIFE_SECONDS);
        // An hour watched last night does not beat that yet...
        let last_night = score(HOUR, Some(now - DAY), now, HALF_LIFE_SECONDS);
        assert!(
            last_night < binged,
            "an hour yesterday should not yet beat twenty hours: {last_night} vs {binged}"
        );
        // ...but two hours does, and that is the ordering wanted.
        let all_week = score(2 * HOUR, Some(now - DAY), now, HALF_LIFE_SECONDS);
        assert!(
            all_week > binged,
            "two hours this week should beat twenty hours two months ago: {all_week} vs {binged}"
        );
    }

    #[test]
    fn the_crossover_is_where_the_arithmetic_says_it_is() {
        let now = 1_700_000_000;
        // A channel watched for `t` seconds `n` half-lives ago is worth
        // exactly the same as one watched for `t / 2^n` seconds right now.
        let old = score(16 * HOUR, Some(now - 42 * DAY), now, HALF_LIFE_SECONDS);
        let equivalent = score(2 * HOUR, Some(now), now, HALF_LIFE_SECONDS);
        assert!(
            (old - equivalent).abs() < 1e-3,
            "three half-lives should divide by eight: {old} vs {equivalent}"
        );
    }

    #[test]
    fn a_future_timestamp_neither_overflows_nor_goes_negative() {
        let now = 1_700_000_000;
        // A clock change, or a database carried between machines. The score
        // must be the same as "just now" — never larger, never negative.
        for ahead in [1, DAY, 3650 * DAY, i64::MAX / 2] {
            let s = score(3600, Some(now + ahead), now, HALF_LIFE_SECONDS);
            assert!(s.is_finite(), "not finite at +{ahead}: {s}");
            assert!(s >= 0.0, "negative at +{ahead}: {s}");
            assert!((s - 3600.0).abs() < 1e-6, "amplified at +{ahead}: {s}");
        }
    }

    #[test]
    fn extreme_inputs_stay_finite_and_non_negative() {
        let now = 1_700_000_000;
        // Ages so large the decay underflows, and the degenerate half-lives a
        // future caller might pass.
        for last in [Some(i64::MIN), Some(0), Some(now - 100_000 * DAY)] {
            let s = score(i64::MAX, last, now, HALF_LIFE_SECONDS);
            assert!(s.is_finite() && s >= 0.0, "bad score for {last:?}: {s}");
        }
        assert_eq!(score(3600, Some(now), now, 0.0), 0.0);
        assert_eq!(score(3600, Some(now), now, -1.0), 0.0);
        assert_eq!(score(3600, Some(now), now, f64::NAN), 0.0);
        assert_eq!(score(3600, Some(now), now, f64::INFINITY), 0.0);
    }

    // ------------------------------------------------------------- ranking

    #[test]
    fn rank_orders_by_recency_weighted_time_and_drops_the_unwatched() {
        let now = 1_700_000_000;
        let rows = vec![
            fav(1, 20 * HOUR, Some(now - 56 * DAY)), // binged, months ago
            fav(2, 2 * HOUR, Some(now - DAY)),       // watched all week
            fav(3, 0, None),                         // never watched
            fav(4, 30 * 60, Some(now - 2 * HOUR)),   // half an hour, today
        ];
        let out = rank(rows, now, 10);
        let ids: Vec<i64> = out.iter().map(|f| f.channel.stream_id).collect();
        assert_eq!(ids, [2, 1, 4], "wrong order: {ids:?}");
        assert!(
            !ids.contains(&3),
            "a channel that was never watched is not a weak favourite"
        );
    }

    #[test]
    fn rank_is_stable_across_identical_input() {
        let now = 1_700_000_000;
        // Deliberately identical on every field that scores, so only the
        // tie-break can decide. Two calls must not shuffle the list.
        let rows: Vec<FavouriteDto> = [9, 3, 7, 1]
            .iter()
            .map(|id| fav(*id, HOUR, Some(now - DAY)))
            .collect();
        let a = rank(rows.clone(), now, 10);
        let b = rank(rows, now, 10);
        let ids = |v: &[FavouriteDto]| -> Vec<i64> {
            v.iter().map(|f| f.channel.stream_id).collect()
        };
        assert_eq!(ids(&a), [1, 3, 7, 9]);
        assert_eq!(ids(&a), ids(&b));
    }

    #[test]
    fn rank_honours_the_limit_and_an_empty_table() {
        let now = 1_700_000_000;
        let rows: Vec<FavouriteDto> = (1..=10)
            .map(|i| fav(i, i * HOUR, Some(now - DAY)))
            .collect();
        let out = rank(rows, now, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].channel.stream_id, 10);
        assert!(rank(Vec::new(), now, 5).is_empty());
    }

    // --------------------------------------------------------- accumulating

    const TICK: f64 = 0.25;

    /// Feeds `seconds` of ticks at the player's real rate, collecting writes.
    fn watch(a: &mut Accumulator, id: Option<i64>, playing: bool, seconds: f64, at: i64) -> Vec<Earned> {
        let mut out = Vec::new();
        let ticks = (seconds / TICK).round() as i64;
        for _ in 0..ticks {
            if let Some(e) = a.observe(id, playing, TICK, at) {
                out.push(e);
            }
        }
        out
    }

    #[test]
    fn a_channel_open_for_two_seconds_earns_nothing() {
        let mut a = Accumulator::new();
        assert!(watch(&mut a, Some(1), true, 2.0, 100).is_empty());
        // Not even on the way out: zapping past is not watching.
        assert_eq!(a.observe(Some(2), false, TICK, 100), None);
    }

    #[test]
    fn twenty_channels_zapped_through_create_no_favourites() {
        let mut a = Accumulator::new();
        let mut writes = Vec::new();
        for id in 1..=20 {
            writes.extend(watch(&mut a, Some(id), true, 4.0, 100));
        }
        writes.extend(a.take(100));
        assert!(writes.is_empty(), "zapping wrote {writes:?}");
    }

    #[test]
    fn the_floor_earns_the_whole_session_once_it_is_crossed() {
        let mut a = Accumulator::new();
        let writes = watch(&mut a, Some(7), true, 30.0, 500);
        assert_eq!(writes.len(), 1, "expected one write, got {writes:?}");
        let w = writes[0];
        assert_eq!(w.stream_id, 7);
        // The first thirty seconds are not forfeited, they are the price of
        // entry and they count.
        assert_eq!(w.seconds, 30);
        assert!(w.open, "crossing the floor is what counts as an open");
        assert_eq!(w.at, 500);
    }

    #[test]
    fn a_long_sitting_is_written_periodically_and_counted_open_once() {
        let mut a = Accumulator::new();
        let writes = watch(&mut a, Some(7), true, 5.0 * 60.0, 900);
        assert_eq!(writes.len(), 10, "expected a write every 30s: {writes:?}");
        assert_eq!(writes.iter().map(|w| w.seconds).sum::<i64>(), 300);
        assert_eq!(writes.iter().filter(|w| w.open).count(), 1);
    }

    #[test]
    fn nothing_accrues_while_opening_paused_or_stopped() {
        let mut a = Accumulator::new();
        // Opening for a minute, then paused for a minute: not watching.
        assert!(watch(&mut a, Some(7), false, 60.0, 100).is_empty());
        assert!(watch(&mut a, Some(7), false, 60.0, 160).is_empty());
        assert_eq!(a.take(200), None);
        // Stopped: no channel at all.
        assert!(watch(&mut a, None, false, 60.0, 260).is_empty());
    }

    #[test]
    fn pausing_in_the_middle_keeps_what_was_watched_either_side() {
        let mut a = Accumulator::new();
        watch(&mut a, Some(7), true, 20.0, 100);
        watch(&mut a, Some(7), false, 600.0, 700); // a ten-minute pause
        let writes = watch(&mut a, Some(7), true, 10.0, 710);
        assert_eq!(writes.len(), 1);
        // Thirty seconds of watching, not ten minutes and thirty seconds.
        assert_eq!(writes[0].seconds, 30);
    }

    #[test]
    fn a_channel_change_flushes_the_remainder_of_the_one_that_earned_it() {
        let mut a = Accumulator::new();
        // 45s: one 30s write lands, 15s is still owed.
        let during = watch(&mut a, Some(7), true, 45.0, 100);
        assert_eq!(during.len(), 1);
        let on_change = a.observe(Some(8), false, TICK, 145);
        let w = on_change.expect("the remainder must not be lost");
        assert_eq!((w.stream_id, w.seconds, w.open), (7, 15, false));
        assert_eq!(w.at, 145);
    }

    #[test]
    fn stopping_flushes_the_same_way_a_channel_change_does() {
        let mut a = Accumulator::new();
        watch(&mut a, Some(7), true, 40.0, 100);
        let w = a
            .observe(None, false, TICK, 140)
            .expect("stopping must flush what was earned");
        assert_eq!((w.stream_id, w.seconds), (7, 10));
    }

    #[test]
    fn returning_to_a_channel_counts_as_a_second_open() {
        let mut a = Accumulator::new();
        watch(&mut a, Some(7), true, 31.0, 100);
        watch(&mut a, Some(8), true, 31.0, 200);
        let back = watch(&mut a, Some(7), true, 31.0, 300);
        assert!(back.iter().any(|w| w.stream_id == 7 && w.open));
    }

    #[test]
    fn fractions_are_carried_rather_than_lost_across_flushes() {
        let mut a = Accumulator::new();
        // Ticks that do not divide the flush interval evenly; over an hour a
        // dropped fraction would be minutes.
        let mut total = 0;
        for _ in 0..(3600.0 / 0.35) as i64 {
            if let Some(w) = a.observe(Some(1), true, 0.35, 0) {
                total += w.seconds;
            }
        }
        if let Some(w) = a.take(0) {
            total += w.seconds;
        }
        assert!(
            (total - 3600).abs() <= 1,
            "an hour of ticks came to {total}s"
        );
    }
}
