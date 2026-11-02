//! How recordings share the account's connections.
//!
//! The rules, which the service follows as it records ([`when_full`]) and
//! which [`simulate`] plays through an evening to warn of conflicts before
//! they happen:
//!
//! 1. **One connection per feed.** Recordings that want the same feed at the
//!    same time ride one connection: the service fans its bytes out to each
//!    recording's file. A recording that joins a connection already open
//!    starts its file then (its padding before comes from the live bytes)
//!    and costs no connection; one whose window ends leaves, and the
//!    connection closes when nobody is left on it. So back-to-back
//!    programmes on one channel are recorded whole, padding and all, on an
//!    account of one stream.
//! 2. **A free connection is taken** when the account has one (the
//!    connection budget, `budget`, says so; it counts every device's streams
//!    where the provider can).
//! 3. **On a full account, the next programme's start beats the last one's
//!    padding.** When a recording needs a connection, the account is full,
//!    and one of the service's own connections carries only recordings past
//!    their programme's end (in their padding after), that connection is
//!    closed: those recordings end there, with a note saying so ("ended
//!    4 min early for ‘B’"). Of several, the one with the least padding left
//!    goes. This is only done for a recording whose own programme has not
//!    ended: one past its end does not take another's padding for its own.
//! 4. **A programme is never cut.** A connection carrying a recording whose
//!    programme is still on is never closed for another; the recording that
//!    wants it waits, giving up its own padding before, and takes the
//!    connection at that programme's end, when rule 3 lets it.
//! 5. **A viewer's connection is never taken.** Only the service's own
//!    connections are closed; one the provider counts for another device (or
//!    the app's player) is left alone, and the recording waits for it as it
//!    always has.
//! 6. **Waiting is woken from inside.** A recording waiting for a
//!    connection tries again as soon as one of the service's own connections
//!    closes or opens on a feed it wants, and at the moment the next
//!    programme on a connection ends (rule 3 may then apply), not only on
//!    its back-off.
//!
//! Pure, with the clock an argument, so every rule is a test on a fake clock.

use std::collections::HashMap;

/// Someone on a connection, as the rules see it.
///
/// A recording rides from its padded start to `until` and its programme
/// ends at `stop`. Anything else that may one day share a connection (a
/// browser watching the channel) is a rider whose `stop` never comes, so the
/// rules never close its connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rider {
    pub id: i64,
    pub stop: i64,
    pub until: i64,
}

impl Rider {
    /// A rider that must never be cut off: a viewer.
    pub fn viewer(id: i64) -> Self {
        Self {
            id,
            stop: i64::MAX,
            until: i64::MAX,
        }
    }
}

/// One of the service's own connections: a feed and who is on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conn {
    pub feed: i64,
    pub riders: Vec<Rider>,
}

impl Conn {
    /// Every rider is past its programme's end: only padding is left on it.
    pub fn only_padding(&self, now: i64) -> bool {
        !self.riders.is_empty() && self.riders.iter().all(|r| r.stop <= now)
    }

    /// When it will be only padding, if it ever will.
    fn padding_from(&self) -> Option<i64> {
        let at = self.riders.iter().map(|r| r.stop).max()?;
        (at != i64::MAX).then_some(at)
    }

    /// When its last rider leaves.
    fn last_until(&self) -> i64 {
        self.riders
            .iter()
            .map(|r| r.until)
            .max()
            .unwrap_or(i64::MIN)
    }
}

/// What a recording that needs a connection does when the account is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Full {
    /// Close the own connection at this index into the list given (rule 3),
    /// then take its place.
    Cut(usize),
    /// Wait. `until`: the moment one of the own connections will carry only
    /// padding, worth trying again at even if nothing else changes.
    Wait { until: Option<i64> },
}

/// Rule 3 and 4: what a recording whose programme ends at `my_stop` does at
/// `now`, the account full, with the service's own connections `own`.
pub fn when_full(now: i64, my_stop: i64, own: &[Conn]) -> Full {
    if now >= my_stop {
        return Full::Wait { until: None };
    }
    let cut = own
        .iter()
        .enumerate()
        .filter(|(_, c)| c.only_padding(now))
        .min_by_key(|(i, c)| (c.last_until(), *i))
        .map(|(i, _)| i);
    if let Some(i) = cut {
        return Full::Cut(i);
    }
    let until = own
        .iter()
        .filter_map(Conn::padding_from)
        .filter(|&t| t > now && t < my_stop)
        .min();
    Full::Wait { until }
}

// ---------------------------------------------------------------------------
// Playing an evening through
// ---------------------------------------------------------------------------

/// A recording as planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Planned {
    pub id: i64,
    /// The feed it records (its main channel: a backup is a failure's
    /// business and cannot be foreseen).
    pub feed: i64,
    /// Its padded start, its programme, and the end of its padding after.
    pub from: i64,
    pub start: i64,
    pub stop: i64,
    pub until: i64,
}

/// How a recording would go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub id: i64,
    /// What it would record, `[from, until)`; `None`: nothing.
    pub got: Option<(i64, i64)>,
    /// The recording its padding after would be cut for.
    pub cut_for: Option<i64>,
    /// The recording it would wait for, when it would wait.
    pub waited_for: Option<i64>,
}

/// Plays `plans` through the rules on an account of `max` connections that
/// nothing else uses, and says how each recording would go.
///
/// The connections only change when a recording's window opens, when a
/// programme ends (rule 3 may apply) and when a window closes, so looking at
/// those moments alone is exact. At one moment, windows that close go first,
/// then recordings take connections in order of their padded start.
pub fn simulate(plans: &[Planned], max: usize) -> Vec<Outcome> {
    let max = max.max(1);
    let mut out: Vec<Outcome> = plans
        .iter()
        .map(|p| Outcome {
            id: p.id,
            got: None,
            cut_for: None,
            waited_for: None,
        })
        .collect();
    let mut order: Vec<usize> = (0..plans.len()).collect();
    order.sort_by_key(|&i| (plans[i].from, plans[i].id));
    let mut times: Vec<i64> = plans
        .iter()
        .flat_map(|p| [p.from, p.stop, p.until])
        .collect();
    times.sort_unstable();
    times.dedup();

    // Each connection: its feed and the plans on it, by index.
    let mut conns: Vec<(i64, Vec<usize>)> = Vec::new();
    // Plans on a connection, and when they joined; plans that are over.
    let mut on: HashMap<usize, i64> = HashMap::new();
    let mut over = vec![false; plans.len()];

    for &t in &times {
        // Windows that close leave their connection.
        for (_, riders) in &mut conns {
            riders.retain(|&i| {
                if plans[i].until <= t {
                    out[i].got = Some((on[&i], plans[i].until));
                    over[i] = true;
                    false
                } else {
                    true
                }
            });
        }
        conns.retain(|(_, r)| !r.is_empty());

        // Whoever wants a connection now takes one, until nothing changes
        // (one opening a feed lets another on that feed ride it).
        loop {
            let mut changed = false;
            for &i in &order {
                let p = &plans[i];
                if over[i] || on.contains_key(&i) || p.from > t {
                    continue;
                }
                if p.until <= t {
                    over[i] = true;
                    continue;
                }
                if let Some((_, riders)) = conns.iter_mut().find(|(f, _)| *f == p.feed) {
                    riders.push(i);
                } else if conns.len() < max {
                    conns.push((p.feed, vec![i]));
                } else {
                    let view: Vec<Conn> = conns
                        .iter()
                        .map(|(feed, riders)| Conn {
                            feed: *feed,
                            riders: riders.iter().map(|&r| rider(&plans[r])).collect(),
                        })
                        .collect();
                    match when_full(t, p.stop, &view) {
                        Full::Cut(c) => {
                            let (_, cut) = conns.remove(c);
                            for r in cut {
                                out[r].got = Some((on[&r], t));
                                out[r].cut_for = Some(p.id);
                                over[r] = true;
                            }
                            conns.push((p.feed, vec![i]));
                        }
                        Full::Wait { .. } => {
                            if out[i].waited_for.is_none() {
                                out[i].waited_for = blocker(&view);
                            }
                            continue;
                        }
                    }
                }
                on.insert(i, t);
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }
    out
}

fn rider(p: &Planned) -> Rider {
    Rider {
        id: p.id,
        stop: p.stop,
        until: p.until,
    }
}

/// Who a waiting recording waits for: a rider of the connection that frees
/// first.
fn blocker(own: &[Conn]) -> Option<i64> {
    let c = own
        .iter()
        .min_by_key(|c| c.padding_from().unwrap_or(i64::MAX).min(c.last_until()))?;
    c.riders.iter().max_by_key(|r| r.stop).map(|r| r.id)
}

/// What a recording would lose to another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loss {
    /// Its padding after ends `secs` early.
    CutEnd { secs: i64 },
    /// It starts `secs` into its padding before: only padding is lost.
    LateStart { secs: i64 },
    /// It starts `secs` into its programme.
    LostStart { secs: i64 },
    /// It never gets a connection.
    Missed,
}

/// A conflict worth showing on a recording: what it loses, and to whom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conflict {
    pub id: i64,
    pub with: i64,
    pub loss: Loss,
}

/// The conflicts in a set of plans on an account of `max` connections:
/// [`simulate`]'s outcomes, as what each recording loses. A recording's
/// start already behind `now` is history, not a forecast, so only what is
/// still ahead is said.
pub fn conflicts(plans: &[Planned], max: usize, now: i64) -> Vec<Conflict> {
    let mut v = Vec::new();
    for (p, o) in plans.iter().zip(simulate(plans, max)) {
        let ahead = |at: i64| at > now;
        match o.got {
            None => {
                if let (Some(with), true) = (o.waited_for, ahead(p.until)) {
                    v.push(Conflict {
                        id: p.id,
                        with,
                        loss: Loss::Missed,
                    });
                }
            }
            Some((from, until)) => {
                if from > p.from && ahead(from) {
                    if let Some(with) = o.waited_for {
                        let loss = if from > p.start {
                            Loss::LostStart {
                                secs: from - p.start,
                            }
                        } else {
                            Loss::LateStart {
                                secs: from - p.from,
                            }
                        };
                        v.push(Conflict {
                            id: p.id,
                            with,
                            loss,
                        });
                    }
                }
                if let (Some(with), true) = (o.cut_for, until < p.until && ahead(until)) {
                    v.push(Conflict {
                        id: p.id,
                        with,
                        loss: Loss::CutEnd {
                            secs: p.until - until,
                        },
                    });
                }
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60;
    const H: i64 = 3600;
    /// 20:00 on some evening.
    const EIGHT: i64 = 1_800_000_000 - 1_800_000_000 % 86_400 + 20 * H;

    /// A programme `[start, stop)` on `feed` with the default padding (1 min
    /// before, 5 after).
    fn plan(id: i64, feed: i64, start: i64, stop: i64) -> Planned {
        padded(id, feed, start, stop, MIN, 5 * MIN)
    }

    fn padded(id: i64, feed: i64, start: i64, stop: i64, before: i64, after: i64) -> Planned {
        Planned {
            id,
            feed,
            from: start - before,
            start,
            stop,
            until: stop + after,
        }
    }

    fn got(out: &[Outcome], id: i64) -> Option<(i64, i64)> {
        out.iter().find(|o| o.id == id).unwrap().got
    }

    fn outcome(out: &[Outcome], id: i64) -> Outcome {
        *out.iter().find(|o| o.id == id).unwrap()
    }

    #[test]
    fn back_to_back_on_one_channel_share_one_connection_and_keep_their_padding() {
        // A 20:00–21:00 and B 21:00–22:00 on the same channel, one stream.
        let a = plan(1, 7, EIGHT, EIGHT + H);
        let b = plan(2, 7, EIGHT + H, EIGHT + 2 * H);
        let out = simulate(&[a, b], 1);
        assert_eq!(
            got(&out, 1),
            Some((a.from, a.until)),
            "A whole: 19:59–21:05"
        );
        assert_eq!(
            got(&out, 2),
            Some((b.from, b.until)),
            "B from 20:59, riding A's connection"
        );
        assert!(
            conflicts(&[a, b], 1, 0).is_empty(),
            "the same channel is no conflict"
        );
    }

    #[test]
    fn on_another_channel_the_padding_after_gives_way_to_the_next_start() {
        // A 20:00–21:00 (until 21:05) on one channel; B 21:02–22:00 on
        // another, padded start 21:01. One stream.
        let a = plan(1, 7, EIGHT, EIGHT + H);
        let b = plan(2, 8, EIGHT + H + 2 * MIN, EIGHT + 2 * H);
        let out = simulate(&[a, b], 1);
        assert_eq!(
            got(&out, 1),
            Some((a.from, b.from)),
            "A ends at B's padded start"
        );
        assert_eq!(outcome(&out, 1).cut_for, Some(2));
        assert_eq!(got(&out, 2), Some((b.from, b.until)), "B whole");
        assert_eq!(
            conflicts(&[a, b], 1, 0),
            [Conflict {
                id: 1,
                with: 2,
                loss: Loss::CutEnd { secs: 4 * MIN }
            }]
        );
    }

    #[test]
    fn a_programme_still_on_is_never_cut_the_next_one_gives_up_its_padding_before() {
        // A 20:00–21:00; B 21:00–22:00 on another channel, padded start
        // 20:59 while A's programme is still on. One stream.
        let a = plan(1, 7, EIGHT, EIGHT + H);
        let b = plan(2, 8, EIGHT + H, EIGHT + 2 * H);
        let out = simulate(&[a, b], 1);
        assert_eq!(
            got(&out, 1),
            Some((a.from, a.stop)),
            "A to its end, not a second less"
        );
        assert_eq!(
            got(&out, 2),
            Some((b.start, b.until)),
            "B from A's end: 21:00"
        );
        assert_eq!(outcome(&out, 2).waited_for, Some(1));
        let c = conflicts(&[a, b], 1, 0);
        assert!(c.contains(&Conflict {
            id: 1,
            with: 2,
            loss: Loss::CutEnd { secs: 5 * MIN }
        }));
        assert!(c.contains(&Conflict {
            id: 2,
            with: 1,
            loss: Loss::LateStart { secs: MIN }
        }));
    }

    #[test]
    fn two_streams_record_both_channels_whole() {
        let a = plan(1, 7, EIGHT, EIGHT + H);
        let b = plan(2, 8, EIGHT + H, EIGHT + 2 * H);
        let out = simulate(&[a, b], 2);
        assert_eq!(got(&out, 1), Some((a.from, a.until)));
        assert_eq!(got(&out, 2), Some((b.from, b.until)));
        assert!(conflicts(&[a, b], 2, 0).is_empty());
    }

    #[test]
    fn overlapping_programmes_on_one_stream_cannot_both_be_recorded() {
        // A 20:00–21:00 and Y 20:30–21:30 on another channel: Y waits for
        // A's end and loses its first half hour.
        let a = plan(1, 7, EIGHT, EIGHT + H);
        let y = plan(2, 8, EIGHT + 30 * MIN, EIGHT + H + 30 * MIN);
        let out = simulate(&[a, y], 1);
        assert_eq!(got(&out, 2), Some((a.stop, y.until)));
        assert!(conflicts(&[a, y], 1, 0).contains(&Conflict {
            id: 2,
            with: 1,
            loss: Loss::LostStart { secs: 30 * MIN }
        }));
        // Inside A's programme altogether: nothing of it.
        let z = plan(3, 8, EIGHT + 10 * MIN, EIGHT + 40 * MIN);
        let out = simulate(&[a, z], 1);
        assert_eq!(got(&out, 3), None);
        assert_eq!(
            conflicts(&[a, z], 1, 0),
            [Conflict {
                id: 3,
                with: 1,
                loss: Loss::Missed
            }]
        );
    }

    #[test]
    fn an_evening_on_one_stream() {
        // 19:00–20:00 news, 20:00–21:00 film and 21:00–22:00 talk on channel
        // 1; 22:00–23:00 match on channel 2; 23:00–23:30 late news on 1.
        let news = plan(1, 1, EIGHT - H, EIGHT);
        let film = plan(2, 1, EIGHT, EIGHT + H);
        let talk = plan(3, 1, EIGHT + H, EIGHT + 2 * H);
        let game = plan(4, 2, EIGHT + 2 * H, EIGHT + 3 * H);
        let late = plan(5, 1, EIGHT + 3 * H, EIGHT + 3 * H + 30 * MIN);
        let all = [news, film, talk, game, late];
        let out = simulate(&all, 1);
        // Channel 1 rides one connection through three programmes, each
        // with all its padding.
        for p in [news, film] {
            assert_eq!(got(&out, p.id), Some((p.from, p.until)));
        }
        // The talk show's programme runs to 22:00, the match's padded start
        // is 21:59: the match waits for 22:00 and the talk show's padding
        // after goes.
        assert_eq!(got(&out, 3), Some((talk.from, talk.stop)));
        assert_eq!(got(&out, 4), Some((game.start, game.stop)));
        // Likewise at 23:00 for the late news.
        assert_eq!(got(&out, 5), Some((late.start, late.until)));
        let c = conflicts(&all, 1, 0);
        assert_eq!(c.len(), 4, "{c:?}");

        // Two streams: everything whole.
        let out = simulate(&all, 2);
        for p in all {
            assert_eq!(got(&out, p.id), Some((p.from, p.until)), "{}", p.id);
        }
        assert!(conflicts(&all, 2, 0).is_empty());
    }

    #[test]
    fn an_evening_on_two_streams_with_three_channels() {
        // A and B overlap on channels 1 and 2 with both streams busy; C on
        // channel 3 starts in their padding after.
        let a = plan(1, 1, EIGHT, EIGHT + H);
        let b = plan(2, 2, EIGHT, EIGHT + H);
        let c = padded(3, 3, EIGHT + H + 3 * MIN, EIGHT + 2 * H, MIN, 5 * MIN);
        let out = simulate(&[a, b, c], 2);
        // At 21:02 both A and B are in padding with the same left; one goes.
        let cut: Vec<i64> = out
            .iter()
            .filter(|o| o.cut_for == Some(3))
            .map(|o| o.id)
            .collect();
        assert_eq!(
            cut,
            [1],
            "one connection is enough, and the first of equals goes"
        );
        assert_eq!(got(&out, 2), Some((b.from, b.until)));
        assert_eq!(got(&out, 3), Some((c.from, c.until)));
    }

    #[test]
    fn the_padding_with_least_left_is_the_one_cut() {
        let a = padded(1, 1, EIGHT, EIGHT + H, 0, 10 * MIN);
        let b = padded(2, 2, EIGHT, EIGHT + H, 0, 3 * MIN);
        let c = padded(3, 3, EIGHT + H + MIN, EIGHT + 2 * H, 0, 0);
        let out = simulate(&[a, b, c], 2);
        assert_eq!(outcome(&out, 2).cut_for, Some(3));
        assert_eq!(got(&out, 1), Some((a.from, a.until)));
    }

    #[test]
    fn a_recording_past_its_own_end_takes_no_ones_padding() {
        let own = [Conn {
            feed: 1,
            riders: vec![Rider {
                id: 1,
                stop: EIGHT,
                until: EIGHT + 5 * MIN,
            }],
        }];
        assert_eq!(
            when_full(EIGHT + MIN, EIGHT + MIN, &own),
            Full::Wait { until: None }
        );
        assert_eq!(when_full(EIGHT + MIN, EIGHT + H, &own), Full::Cut(0));
    }

    #[test]
    fn a_connection_with_a_programme_on_waits_until_its_end() {
        let own = [Conn {
            feed: 1,
            riders: vec![
                Rider {
                    id: 1,
                    stop: EIGHT,
                    until: EIGHT + 5 * MIN,
                },
                // Joined on the same channel: its programme is on.
                Rider {
                    id: 2,
                    stop: EIGHT + H,
                    until: EIGHT + H + 5 * MIN,
                },
            ],
        }];
        assert_eq!(
            when_full(EIGHT + MIN, EIGHT + 2 * H, &own),
            Full::Wait {
                until: Some(EIGHT + H)
            }
        );
        // Its end beyond the waiting one's own: nothing worth waking for.
        assert_eq!(
            when_full(EIGHT + MIN, EIGHT + 30 * MIN, &own),
            Full::Wait { until: None }
        );
    }

    #[test]
    fn a_programme_scheduled_part_way_through_is_followed_on_the_same_connection() {
        // On an account of one stream: rec 1, 18:00–18:30, scheduled while
        // it was on, so its window opened at 18:21:34 (padded 17:59–18:35);
        // rec 2, 18:30–19:30 on the same channel (padded 18:29–19:35). Rec 2
        // used to wait for rec 1's connection and start at 18:36:54, seven
        // minutes into its programme. It rides that connection from 18:29.
        let six = EIGHT - 2 * H;
        let mut one = plan(1, 7, six, six + 30 * MIN);
        one.from = six + 21 * MIN + 34;
        let two = plan(2, 7, six + 30 * MIN, six + 90 * MIN);
        assert_eq!(two.from, six + 29 * MIN);
        let out = simulate(&[one, two], 1);
        assert_eq!(got(&out, 1), Some((one.from, one.until)), "rec 1 to 18:35");
        assert_eq!(got(&out, 2), Some((two.from, two.until)), "rec 2 from 18:29");
        assert!(conflicts(&[one, two], 1, 0).is_empty());
        // And the decision the service takes at 18:29 never comes to a
        // full account: rec 1's connection is there to ride, not to wait for.
        let own = [Conn {
            feed: 7,
            riders: vec![rider(&one)],
        }];
        assert!(own.iter().any(|c| c.feed == two.feed));
    }

    #[test]
    fn a_viewer_is_never_cut() {
        let own = [Conn {
            feed: 1,
            riders: vec![Rider::viewer(-1)],
        }];
        assert_eq!(
            when_full(EIGHT, EIGHT + H, &own),
            Full::Wait { until: None }
        );
    }

    #[test]
    fn what_is_already_behind_is_not_forecast() {
        let a = plan(1, 7, EIGHT, EIGHT + H);
        let b = plan(2, 8, EIGHT + H, EIGHT + 2 * H);
        // At 21:00:30 B's late start is history; A's cut is too (its window
        // was over at 21:00 on this plan).
        assert!(conflicts(&[a, b], 1, EIGHT + H + 30).is_empty());
        // At 20:30 both are ahead.
        assert_eq!(conflicts(&[a, b], 1, EIGHT + 30 * MIN).len(), 2);
    }
}
