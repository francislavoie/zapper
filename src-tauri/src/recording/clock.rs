//! The clock track: a subtitle stream showing when each moment was live.
//!
//! One cue per second, `HH:MM:SS`, small in the bottom-right corner. The time
//! is the wall clock at which the picture arrived, carried through the
//! stream's own timestamps: so it runs smoothly while the feed does, and
//! jumps where the recording has a hole (a reconnect, a switch of feed, a
//! stall the provider did not make up).
//!
//! Two inputs, both from the capture of one part:
//!
//! - [`Arrival`]s, written by the capture as bytes land: "by this wall-clock
//!   millisecond, the file was this long", plus a mark where each new
//!   connection's bytes begin.
//! - [`Sample`]s, from the remuxer's first pass over the finished file: for
//!   packets of the reference stream (the video, or else the audio), the
//!   byte position they came from and where they fall on the output's
//!   timeline, flagged where the remuxer found the timestamps broken and
//!   joined them up.
//!
//! Within one stretch of unbroken timeline, wall time and stream time run
//! together, but a feed delivers in bursts — a provider typically opens with
//! several seconds at once, from its buffer — so a packet's arrival is that
//! much later than its moment on air. The latency the steady flow settles to
//! is the smallest gap seen, so each stretch is placed by its minimum of
//! `arrival - position`.

/// The file was `pos` bytes long by `wall_ms`. `connect`: a new connection
/// begins at `pos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    pub pos: u64,
    pub wall_ms: i64,
    pub connect: bool,
}

/// A reference-stream packet read from byte `pos` lands at `out_ms` on the
/// output's timeline. `disc`: the timeline was joined up here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub pos: u64,
    pub out_ms: i64,
    pub disc: bool,
}

/// One subtitle event, output-timeline milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// When the byte at `pos` had arrived: the first record past it.
fn arrived(arrivals: &[Arrival], pos: u64) -> Option<i64> {
    let i = arrivals.partition_point(|a| a.pos <= pos);
    arrivals.get(i).map(|a| a.wall_ms)
}

/// Whether a connection began between two byte positions.
fn reconnected(arrivals: &[Arrival], from: u64, to: u64) -> bool {
    let i = arrivals.partition_point(|a| a.pos <= from);
    arrivals[i..]
        .iter()
        .take_while(|a| a.pos <= to)
        .any(|a| a.connect)
}

/// A stretch of output timeline from `from_ms`, placed on the wall clock by
/// `offset_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stretch {
    from_ms: i64,
    offset_ms: i64,
}

fn stretches(samples: &[Sample], arrivals: &[Arrival]) -> Vec<Stretch> {
    let mut out: Vec<Stretch> = Vec::new();
    let mut prev: Option<&Sample> = None;
    for s in samples {
        let Some(wall) = arrived(arrivals, s.pos) else {
            continue;
        };
        let breaks = match prev {
            None => true,
            Some(p) => s.disc || reconnected(arrivals, p.pos, s.pos),
        };
        let offset = wall - s.out_ms;
        match out.last_mut() {
            Some(last) if !breaks => last.offset_ms = last.offset_ms.min(offset),
            _ => out.push(Stretch {
                from_ms: s.out_ms,
                offset_ms: offset,
            }),
        }
        prev = Some(s);
    }
    out
}

/// The cues for an output of `duration_ms`, read in a time zone `tz(unix)`
/// seconds east of UTC.
///
/// Each cue lasts until the wall clock reaches its next second, so the clock
/// ticks over on the second rather than a second after the recording began.
/// The part before the first sample shows the first stretch's time.
pub fn cues(
    samples: &[Sample],
    arrivals: &[Arrival],
    duration_ms: i64,
    tz: impl Fn(i64) -> i64,
) -> Vec<Cue> {
    let mut parts = stretches(samples, arrivals);
    let Some(first) = parts.first_mut() else {
        return Vec::new();
    };
    first.from_ms = first.from_ms.min(0);
    let mut out = Vec::new();
    for (i, st) in parts.iter().enumerate() {
        let end = parts
            .get(i + 1)
            .map_or(duration_ms, |n| n.from_ms)
            .min(duration_ms);
        let mut t = st.from_ms.max(0);
        while t < end {
            let wall = t + st.offset_ms;
            // The next whole second of wall time, back on the output's.
            let next = (wall.div_euclid(1000) + 1) * 1000 - st.offset_ms;
            let until = next.min(end);
            out.push(Cue {
                start_ms: t,
                end_ms: until,
                text: hms(wall, &tz),
            });
            t = until;
        }
    }
    out
}

fn hms(wall_ms: i64, tz: &impl Fn(i64) -> i64) -> String {
    let secs = wall_ms.div_euclid(1000);
    let local = (secs + tz(secs)).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        local / 3600,
        (local % 3600) / 60,
        local % 60
    )
}

/// The track's `[Script Info]`, style and event format: Matroska keeps it in
/// the track's private data, and each packet carries one event's fields.
///
/// Small (a sixteenth of the picture's height), white with a dark outline so
/// it reads on any picture, in the bottom-right corner (alignment 3).
pub fn ass_header() -> String {
    "[Script Info]\n\
     ScriptType: v4.00+\n\
     PlayResX: 384\n\
     PlayResY: 288\n\
     ScaledBorderAndShadow: yes\n\
     \n\
     [V4+ Styles]\n\
     Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
     Style: Clock,Monospace,14,&H00FFFFFF,&H00FFFFFF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,1,0,3,8,8,6,1\n\
     \n\
     [Events]\n\
     Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
        .to_string()
}

/// One event as a Matroska ASS packet carries it: `ReadOrder, Layer, Style,
/// Name, MarginL, MarginR, MarginV, Effect, Text` (the timing is the
/// packet's).
pub fn ass_packet(read_order: usize, text: &str) -> String {
    format!("{read_order},0,Clock,,0,0,0,,{text}")
}

/// The capture's arrival log, one line per record: `<pos> <wall_ms>`, or
/// `<pos> <wall_ms> connect`. Text, so a capture cut off mid-write loses at
/// most its last line.
pub fn log_line(a: &Arrival) -> String {
    if a.connect {
        format!("{} {} connect\n", a.pos, a.wall_ms)
    } else {
        format!("{} {}\n", a.pos, a.wall_ms)
    }
}

pub fn parse_log(text: &str) -> Vec<Arrival> {
    let mut out: Vec<Arrival> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let pos = f.next()?.parse().ok()?;
            let wall_ms = f.next()?.parse().ok()?;
            Some(Arrival {
                pos,
                wall_ms,
                connect: f.next() == Some("connect"),
            })
        })
        .collect();
    out.sort_by_key(|a| (a.pos, !a.connect));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: fn(i64) -> i64 = |_| 0;
    /// 20:00:00 UTC on some day, in ms.
    const EIGHT_PM: i64 = 1_793_736_000_000;

    /// A steady feed: one second of stream per second of wall time, 100
    /// bytes per second of stream, arriving `latency` ms after it aired.
    fn steady(
        from_s: i64,
        secs: i64,
        base_pos: u64,
        wall0: i64,
        connect: bool,
    ) -> (Vec<Sample>, Vec<Arrival>) {
        let mut s = Vec::new();
        let mut a = Vec::new();
        for i in 0..secs {
            let pos = base_pos + (i as u64) * 100;
            s.push(Sample {
                pos,
                out_ms: (from_s + i) * 1000,
                disc: false,
            });
            a.push(Arrival {
                pos: pos + 100,
                wall_ms: wall0 + i * 1000 + 500,
                connect: false,
            });
        }
        if connect {
            a.insert(
                0,
                Arrival {
                    pos: base_pos,
                    wall_ms: wall0,
                    connect: true,
                },
            );
        }
        (s, a)
    }

    #[test]
    fn a_steady_feed_ticks_once_a_second_on_the_second() {
        let (s, a) = steady(0, 5, 0, EIGHT_PM, true);
        let c = cues(&s, &a, 5000, UTC);
        // Packet at 0 ms fully arrived at 20:00:00.500, so the clock reads
        // 20:00:00 from 0 to 500 ms, then 20:00:01 for a whole second.
        assert_eq!(
            c[0],
            Cue {
                start_ms: 0,
                end_ms: 500,
                text: "20:00:00".into()
            }
        );
        assert_eq!(
            c[1],
            Cue {
                start_ms: 500,
                end_ms: 1500,
                text: "20:00:01".into()
            }
        );
        assert_eq!(c.last().unwrap().end_ms, 5000);
        assert!(
            c.windows(2).all(|w| w[0].end_ms == w[1].start_ms),
            "no gaps or overlaps"
        );
    }

    #[test]
    fn an_opening_burst_does_not_skew_the_clock() {
        // The first three seconds arrive together, a second late; the rest
        // flows. The flow's latency places the whole stretch.
        let (s, mut a) = steady(0, 10, 0, EIGHT_PM, false);
        for x in a.iter_mut().take(3) {
            x.wall_ms = EIGHT_PM + 3000;
        }
        let c = cues(&s, &a, 10_000, UTC);
        assert_eq!(c[0].text, "20:00:00");
        assert_eq!(c[0].end_ms, 500);
    }

    #[test]
    fn a_reconnect_shows_as_a_jump() {
        // Five seconds, then a reconnect forty seconds later in wall time,
        // whose stream the remuxer joined straight on at 5 s.
        let (mut s, mut a) = steady(0, 5, 0, EIGHT_PM, true);
        let (s2, a2) = steady(5, 5, 500, EIGHT_PM + 45_000, true);
        s.extend(s2);
        a.extend(a2);
        let c = cues(&s, &a, 10_000, UTC);
        let at = |ms: i64| {
            c.iter()
                .find(|q| q.start_ms <= ms && ms < q.end_ms)
                .unwrap()
                .text
                .clone()
        };
        assert_eq!(at(4_600), "20:00:05");
        assert_eq!(at(5_000), "20:00:45");
        assert_eq!(at(9_999), "20:00:50");
    }

    #[test]
    fn a_joined_timeline_starts_a_new_stretch() {
        let (mut s, a) = steady(0, 10, 0, EIGHT_PM, true);
        // Pretend the remuxer joined a broken timeline at the sixth packet,
        // which then arrived much later than the timeline suggests.
        s[5].disc = true;
        let mut a = a;
        for x in a.iter_mut().skip(6) {
            x.wall_ms += 60_000;
        }
        let c = cues(&s, &a, 10_000, UTC);
        let at = |ms: i64| {
            c.iter()
                .find(|q| q.start_ms <= ms && ms < q.end_ms)
                .unwrap()
                .text
                .clone()
        };
        assert_eq!(at(4_000), "20:00:04");
        assert_eq!(at(5_000), "20:01:05");
    }

    #[test]
    fn the_time_zone_is_applied() {
        let (s, a) = steady(0, 2, 0, EIGHT_PM, true);
        let c = cues(&s, &a, 2000, |_| -5 * 3600);
        assert_eq!(c[0].text, "15:00:00");
    }

    #[test]
    fn nothing_to_place_means_no_cues() {
        assert!(cues(&[], &[], 10_000, UTC).is_empty());
    }

    #[test]
    fn the_log_round_trips_and_survives_a_torn_last_line() {
        let a = [
            Arrival {
                pos: 0,
                wall_ms: 5,
                connect: true,
            },
            Arrival {
                pos: 188,
                wall_ms: 9,
                connect: false,
            },
        ];
        let mut text: String = a.iter().map(log_line).collect();
        text.push_str("376");
        assert_eq!(parse_log(&text), a);
        assert_eq!(parse_log("garbage\n12"), vec![]);
    }

    #[test]
    fn packets_carry_the_style_and_text() {
        assert_eq!(ass_packet(7, "20:00:01"), "7,0,Clock,,0,0,0,,20:00:01");
        assert!(ass_header().contains("Style: Clock,"));
    }
}
