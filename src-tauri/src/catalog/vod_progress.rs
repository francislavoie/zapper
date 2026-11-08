//! When to write down how far into a title the player is.
//!
//! The player reports its position many times a second; the database should
//! hear about it rarely, but never lose more than a few seconds of it. The
//! saver watches those reports and says when a write is due.

use anyhow::Result;
use rusqlite::Connection;

use crate::catalog::vod_store::{self, Marks, VodItem};

/// How much play passes between writes.
pub const SAVE_EVERY_SECS: f64 = 10.0;
/// Past this fraction of a title it counts as started ("in progress").
pub const STARTED_FRACTION: f64 = 0.01;
/// Past this fraction it counts as watched.
pub const WATCHED_FRACTION: f64 = 0.92;

/// Whether `position` is far enough in to call the title watched. Unknown
/// length never is: there is nothing to take a fraction of.
pub fn is_finished(position: f64, duration: Option<f64>) -> bool {
    match duration {
        Some(d) if d > 0.0 => position >= d * WATCHED_FRACTION,
        _ => false,
    }
}

/// Whether `position` is "in progress": past [`STARTED_FRACTION`] and short
/// of [`WATCHED_FRACTION`]. Unknown length never is.
pub fn is_started(position: f64, duration: Option<f64>) -> bool {
    match duration {
        Some(d) if d > 0.0 => (STARTED_FRACTION..WATCHED_FRACTION).contains(&(position / d)),
        _ => false,
    }
}

/// A place to write down: a movie's or an episode's, never mistaken for
/// the other kind's with the same id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressWrite {
    pub item: VodItem,
    pub position: f64,
    pub duration: Option<f64>,
}

#[derive(Debug, Default)]
pub struct ProgressSaver {
    item: Option<VodItem>,
    position: Option<f64>,
    duration: Option<f64>,
    /// The position has moved since the last write.
    dirty: bool,
    playing: bool,
    last_seen: Option<f64>,
    /// Seconds played since the last write. Counted from the clock between
    /// reports while playing, so a long pause does not make the next report
    /// look overdue.
    played: f64,
}

impl ProgressSaver {
    pub fn new() -> Self {
        Self::default()
    }

    /// One report from the player: which title (if any), where, how long it
    /// is, whether it is playing, and the clock in seconds. Returns a write
    /// when one is due: every [`SAVE_EVERY_SECS`] of play, once when playback
    /// pauses, and once when the title changes (for the one just left).
    pub fn observe(
        &mut self,
        item: Option<VodItem>,
        position: Option<f64>,
        duration: Option<f64>,
        playing: bool,
        now: f64,
    ) -> Option<ProgressWrite> {
        let mut out = None;
        if item != self.item {
            out = self.flush();
            *self = Self {
                item,
                ..Self::default()
            };
        } else if self.playing {
            if let Some(t) = self.last_seen {
                self.played += (now - t).max(0.0);
            }
        }
        self.last_seen = Some(now);
        if let Some(p) = position {
            if self.position != Some(p) {
                self.dirty = true;
            }
            self.position = Some(p);
        }
        if duration.is_some() {
            self.duration = duration;
        }
        let was_playing = self.playing;
        self.playing = playing;
        if out.is_some() {
            return out;
        }
        if playing && self.played >= SAVE_EVERY_SECS {
            self.played = 0.0;
            return self.flush();
        }
        if was_playing && !playing {
            return self.flush();
        }
        None
    }

    /// The last position not yet written, for when playback stops or ends.
    pub fn flush(&mut self) -> Option<ProgressWrite> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(ProgressWrite {
            item: self.item?,
            position: self.position?,
            duration: self.duration,
        })
    }
}

/// Writes `w` down as of `at` (seconds): a movie's place on the movie, an
/// episode's on the episode (and, through it, its show).
pub fn write_progress(conn: &Connection, w: ProgressWrite, at: i64) -> Result<Marks> {
    vod_store::record_progress(conn, w.item.kind, w.item.id, w.position, w.duration, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_saved_every_ten_seconds_of_play() {
        let mut s = ProgressSaver::new();
        let mut at = Vec::new();
        for t in 0..=25 {
            if s.observe(Some(VodItem::movie(1)), Some(t as f64), Some(100.0), true, t as f64)
                .is_some()
            {
                at.push(t);
            }
        }
        assert_eq!(at, [10, 20]);
    }

    #[test]
    fn a_pause_saves_once_and_a_new_title_saves_the_last_one() {
        let mut s = ProgressSaver::new();
        assert!(s
            .observe(Some(VodItem::movie(1)), Some(5.0), Some(100.0), true, 0.0)
            .is_none());
        let w = s
            .observe(Some(VodItem::movie(1)), Some(6.0), Some(100.0), false, 1.0)
            .unwrap();
        assert_eq!((w.item, w.position), (VodItem::movie(1), 6.0));
        assert!(s
            .observe(Some(VodItem::movie(1)), Some(6.0), Some(100.0), false, 2.0)
            .is_none());
        assert!(s.flush().is_none());

        s.observe(Some(VodItem::movie(1)), Some(7.0), Some(100.0), true, 3.0);
        let w = s
            .observe(Some(VodItem::movie(2)), Some(0.0), Some(50.0), true, 4.0)
            .unwrap();
        assert_eq!((w.item, w.position, w.duration), (VodItem::movie(1), 7.0, Some(100.0)));
    }

    #[test]
    fn stopping_flushes_what_was_not_yet_saved() {
        let mut s = ProgressSaver::new();
        s.observe(Some(VodItem::movie(3)), Some(4.0), None, true, 0.0);
        s.observe(Some(VodItem::movie(3)), Some(5.0), None, true, 1.0);
        assert_eq!(s.flush().map(|w| (w.item, w.position)), Some((VodItem::movie(3), 5.0)));
        assert!(s.flush().is_none());
    }

    #[test]
    fn movie_7_then_episode_7_is_a_change_of_title() {
        let mut s = ProgressSaver::new();
        let movie = VodItem::movie(7);
        let episode = VodItem::episode(7);
        assert!(s.observe(Some(movie), Some(30.0), Some(100.0), true, 0.0).is_none());
        let w = s
            .observe(Some(episode), Some(0.0), Some(50.0), true, 1.0)
            .expect("the movie's place, written as the episode takes over");
        assert_eq!((w.item, w.position, w.duration), (movie, 30.0, Some(100.0)));
        s.observe(Some(episode), Some(5.0), Some(50.0), true, 2.0);
        assert_eq!(s.flush().map(|w| w.item), Some(episode));
    }

    #[test]
    fn progress_of_episode_7_never_touches_movie_7() {
        let c = crate::catalog::open_in_memory().unwrap();
        let marks = write_progress(
            &c,
            ProgressWrite {
                item: VodItem::episode(7),
                position: 95.0,
                duration: Some(100.0),
            },
            1,
        )
        .unwrap();
        assert!(marks.watched);
        let rows: Vec<(String, i64, bool)> = c
            .prepare("SELECT kind, item_id, watched_at IS NOT NULL FROM vod_marks")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, [("episode".to_string(), 7, true)]);
    }

    #[test]
    fn ninety_two_percent_is_watched() {
        assert!(!is_finished(91.9, Some(100.0)));
        assert!(is_finished(92.0, Some(100.0)));
        assert!(!is_finished(5000.0, None));
    }

    #[test]
    fn in_progress_is_one_to_ninety_two_percent() {
        assert!(!is_started(0.9, Some(100.0)));
        assert!(is_started(1.0, Some(100.0)));
        assert!(is_started(91.9, Some(100.0)));
        assert!(!is_started(92.0, Some(100.0)));
        assert!(!is_started(30.0, None));
        assert!(!is_started(30.0, Some(0.0)));
    }
}
