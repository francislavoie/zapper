//! Closed captions (CEA-608, mpv's `eia_608`) drawn where the broadcaster
//! put them.
//!
//! FFmpeg's caption decoder writes each caption as ASS text pinned to a cell
//! of the 608 grid (32 columns by 15 rows), with `{\an7}{\pos(x,y)}` worked
//! out on a 384×288 canvas, which is 4:3. On wide video mpv widens that
//! canvas to the picture's shape but leaves the positions as they were, so
//! the captions drift into the left of the frame, and the viewer's font in
//! place of the grid's breaks the indents. So while captions are shown mpv
//! is told to ignore the positions (`sub-ass-override=strip`, which also
//! draws them in the viewer's subtitle style), and the placement is rebuilt
//! here from the caption's own grid cells: its rows as `sub-pos`, its side
//! as `sub-align-x` with a margin that keeps it inside the 4:3 picture a
//! television would draw the grid in.
//!
//! Pure but for one place to keep what was last written: the heartbeat in
//! `player_cmds` reads the track list and `sub-text/ass`, and sets what
//! [`Captions::update`] says.

use crate::ipc::types::TrackDto;

/// What mpv calls the decoder's captions.
pub const CAPTION_CODEC: &str = "eia_608";

/// The grid: 32 columns, 15 rows.
const COLUMNS: f64 = 32.0;
const ROWS: f64 = 15.0;
/// The decoder's canvas (`ASS_DEFAULT_PLAYRES*`), and the share of it the
/// grid starts in from each edge: `x = 384 · (0.1 + 0.025 · column)`,
/// `y = 288 · (0.1 + 0.0533 · row)`, rows from 0.
const PLAY_RES_X: f64 = 384.0;
const PLAY_RES_Y: f64 = 288.0;
const COLUMN_STEP: f64 = 0.025;
const ROW_STEP: f64 = 0.0533;
const INSET: f64 = 0.1;

/// How far a row's middle may be from the grid's and still count as
/// centred, in columns: a caption centred by its indents is off by half a
/// column at most, for a line of odd length.
const CENTRED_WITHIN: f64 = 2.0;

/// mpv's own values for everything set here, which is what any other
/// subtitle track is drawn with (as this build of mpv reads them back).
/// `sub-margin-x` is in mpv's scaled pixels, on a picture 720 high.
pub const DEFAULTS: [(&str, &str); 5] = [
    ("sub-ass-override", "scale"),
    ("sub-align-y", "bottom"),
    ("sub-pos", "100"),
    ("sub-align-x", "center"),
    ("sub-margin-x", "19"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Center,
    Right,
}

/// Where a caption sits on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptionPlacement {
    /// Its lowest row, from 0 (the top) to 14.
    pub last_row: u32,
    /// How many rows it spans, from its first to its last.
    pub rows: u32,
    pub side: Side,
    /// Its first column, and the column after its last character.
    pub start: f64,
    pub end: f64,
}

/// One row of the caption: its grid row, and the columns its characters
/// cover.
struct Row {
    row: u32,
    start: f64,
    end: f64,
}

/// The two numbers in `\pos(x,y)`, after `at`.
fn pos_after(text: &str) -> Option<(f64, f64, usize)> {
    let at = text.find("\\pos(")?;
    let rest = &text[at + 5..];
    let close = rest.find(')')?;
    let (x, y) = rest[..close].split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?, at + 5 + close + 1))
}

/// A row of the caption as the decoder wrote it: `{\an7}{\pos(x,y)}`, then
/// a `\h` for each space it is indented by past the other rows, then the
/// text, with `{…}` tags for italics and colour.
fn row_of(line: &str) -> Option<Row> {
    let (x, y, after) = pos_after(line)?;
    let mut text = String::new();
    let mut tag = false;
    for c in line[after..].chars() {
        match c {
            '{' => tag = true,
            '}' => tag = false,
            c if !tag => text.push(c),
            _ => {}
        }
    }
    let mut indent = 0.0;
    let mut body = text.as_str();
    while let Some(rest) = body.strip_prefix("\\h") {
        indent += 1.0;
        body = rest;
    }
    let width = body.replace("\\h", " ").trim_end().chars().count() as f64;
    let column = ((x / PLAY_RES_X - INSET) / COLUMN_STEP).round().clamp(0.0, COLUMNS - 1.0);
    let row = ((y / PLAY_RES_Y - INSET) / ROW_STEP).round().clamp(0.0, ROWS - 1.0);
    let start = column + indent;
    Some(Row {
        row: row as u32,
        start,
        end: (start + width).min(COLUMNS),
    })
}

/// Where a caption is, from its ASS text as mpv's `sub-text/ass` gives it:
/// its rows, and whether it is centred on the grid, or to the left or the
/// right of it. `None` without a `\pos` (nothing to go on: the last
/// placement stands).
///
/// mpv joins captions on screen together with a line break (the same one
/// twice, when a stream starts over); the first one is the one placed.
pub fn caption_placement(ass_text: &str) -> Option<CaptionPlacement> {
    let event = ass_text.split('\n').find(|e| e.contains("\\pos("))?;
    let rows: Vec<Row> = event
        .split("\\N")
        .filter_map(row_of)
        .filter(|r| r.end > r.start)
        .collect();
    let first = rows.iter().map(|r| r.row).min()?;
    let last = rows.iter().map(|r| r.row).max()?;
    let start = rows.iter().map(|r| r.start).fold(f64::INFINITY, f64::min);
    let end = rows.iter().map(|r| r.end).fold(0.0, f64::max);
    let middle = COLUMNS / 2.0;
    let centred = rows
        .iter()
        .all(|r| ((r.start + r.end) / 2.0 - middle).abs() <= CENTRED_WITHIN);
    let same = |f: fn(&Row) -> f64| rows.iter().all(|r| f(r) == f(&rows[0]));
    let side = if centred {
        Side::Center
    } else if rows.len() > 1 && same(|r| r.end) && !same(|r| r.start) {
        // Ragged on the left, flush on the right.
        Side::Right
    } else if (rows.len() > 1 && same(|r| r.start)) || start < middle {
        Side::Left
    } else {
        Side::Right
    };
    Some(CaptionPlacement {
        last_row: last,
        rows: last - first + 1,
        side,
        start,
        end,
    })
}

/// mpv's `sub-pos` for a caption: where its lowest row's bottom edge is on
/// a television, which draws the grid's 15 rows over the middle 80% of the
/// picture's height. Kept high enough for all its rows to be seen.
fn sub_pos(p: &CaptionPlacement) -> i64 {
    let bottom = INSET * 100.0 + f64::from(p.last_row + 1) * 80.0 / ROWS;
    let lowest = f64::from(p.rows) * 7.0 + 3.0;
    bottom.clamp(lowest, 100.0).round() as i64
}

/// `sub-margin-x` for a caption to one side: from the edge of the 4:3
/// picture centred in a wider one (the grid's frame on a television), to
/// the caption's own first or last column. `aspect` is the video's width
/// over its height; without one the picture is taken to be 4:3.
///
/// mpv measures this margin on the captions' own canvas, which keeps the
/// decoder's 384 units across the whole width while its 288 are the height:
/// on wide video a margin comes out `aspect · 3/4` times wider than it would
/// in mpv's scaled pixels (a picture 720 high). So that much less is asked.
fn margin_x(p: &CaptionPlacement, aspect: Option<f64>) -> i64 {
    let frame = 720.0 * 4.0 / 3.0;
    let aspect = aspect.filter(|a| a.is_finite() && *a > 0.0).unwrap_or(4.0 / 3.0);
    let side = ((720.0 * aspect - frame) / 2.0).max(0.0);
    let columns = match p.side {
        Side::Left => p.start,
        Side::Right => COLUMNS - p.end,
        Side::Center => return 19,
    };
    let scaled = side + frame * (INSET + COLUMN_STEP * columns);
    (scaled * 4.0 / 3.0 / aspect).round() as i64
}

/// The properties that draw captions placed at `p` (or, before any
/// placement is known, only plainly in the viewer's style).
pub fn caption_properties(p: Option<&CaptionPlacement>, aspect: Option<f64>) -> Vec<(&'static str, String)> {
    let mut out = vec![("sub-ass-override", "strip".to_owned())];
    if let Some(p) = p {
        out.push(("sub-align-y", "bottom".to_owned()));
        out.push(("sub-pos", sub_pos(p).to_string()));
        out.push((
            "sub-align-x",
            match p.side {
                Side::Left => "left",
                Side::Center => "center",
                Side::Right => "right",
            }
            .to_owned(),
        ));
        out.push(("sub-margin-x", margin_x(p, aspect).to_string()));
    }
    out
}

/// Whether the subtitle track shown is closed captions.
pub fn captions_selected(subtitles: &[TrackDto]) -> bool {
    subtitles
        .iter()
        .any(|t| t.selected && t.codec.as_deref() == Some(CAPTION_CODEC))
}

/// What captions are being drawn with, and what was last written to mpv.
#[derive(Debug, Clone, PartialEq)]
pub struct Captions {
    on: bool,
    placement: Option<CaptionPlacement>,
    written: Vec<(&'static str, String)>,
}

impl Default for Captions {
    fn default() -> Self {
        Self::new()
    }
}

impl Captions {
    /// As mpv starts: its own values for everything.
    pub const fn new() -> Self {
        Self {
            on: false,
            placement: None,
            written: Vec::new(),
        }
    }

    fn last(&self, key: &str) -> String {
        self.written
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .or_else(|| DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_owned()))
            .unwrap_or_default()
    }

    /// What to set now, given whether captions are the track shown
    /// (`selected`, when the track list said), the caption on screen
    /// (`text`, when it changed) and the video's shape. Only what differs
    /// from what was last written; and everything touched goes back to
    /// mpv's own values once another track, or none, is shown.
    pub fn update(
        &mut self,
        selected: Option<bool>,
        text: Option<&str>,
        aspect: Option<f64>,
    ) -> Vec<(&'static str, String)> {
        if let Some(on) = selected {
            if on != self.on {
                self.on = on;
                self.placement = None;
            }
        }
        let want: Vec<(&'static str, String)> = if self.on {
            if let Some(p) = text.and_then(caption_placement) {
                self.placement = Some(p);
            }
            caption_properties(self.placement.as_ref(), aspect)
        } else {
            DEFAULTS.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
        };
        let changes: Vec<(&'static str, String)> =
            want.into_iter().filter(|(k, v)| self.last(k) != *v).collect();
        for (k, v) in &changes {
            self.written.retain(|(w, _)| w != k);
            self.written.push((k, v.clone()));
        }
        changes
    }

    /// A property mpv refused: written again next time it is wanted.
    pub fn forget(&mut self, key: &str) {
        self.written.retain(|(k, _)| *k != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Events as FFmpeg's decoder writes them (dumped from a synthetic
    // caption file and from FFmpeg's own caption samples).
    const CENTRED_BOTTOM: &str =
        "{\\an7}{\\pos(67,228)}\\h\\h\\hBonsoir et bienvenue\\N{\\an7}{\\pos(67,243)}au Téléjournal de ce soir.";
    const CENTRED_ONE_ROW: &str = "{\\an7}{\\pos(96,243)}Le premier ministre";
    const TOP_LEFT: &str = "{\\an7}{\\pos(38,28)}({\\i1} inaudible radio chatter{\\i0} )\\N{\\an7}{\\pos(38,44)}>> Safety remains our number one";
    const ROLL_UP: &str = "{\\an7}{\\pos(38,166)}\\hBESIDES THE \\N{\\an7}{\\pos(38,197)}SPENDING AND THIS, IS THAT CAR ";
    const BOTTOM_ROLL_UP: &str =
        "{\\an7}{\\pos(38,228)}SPENDING AND THIS, IS THAT CAR \\N{\\an7}{\\pos(38,243)}MANUFACTURERS ARE ABOUT AS ";
    const RIGHT: &str = "{\\an7}{\\pos(240,243)}ARENA";

    fn placed(text: &str) -> CaptionPlacement {
        caption_placement(text).expect("a placement")
    }

    fn props(text: &str, aspect: Option<f64>) -> Vec<(&'static str, String)> {
        caption_properties(Some(&placed(text)), aspect)
    }

    fn value(props: &[(&'static str, String)], key: &str) -> String {
        props.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap()
    }

    #[test]
    fn a_roll_up_at_the_first_column_stays_left() {
        let p = placed(BOTTOM_ROLL_UP);
        assert_eq!((p.side, p.last_row, p.rows, p.start), (Side::Left, 14, 2, 0.0));
        let at = props(BOTTOM_ROLL_UP, Some(16.0 / 9.0));
        // The bottom of the grid, which is a tenth of the way up the picture.
        assert_eq!(value(&at, "sub-pos"), "90");
        assert_eq!(value(&at, "sub-align-x"), "left");
        assert_eq!(value(&at, "sub-align-y"), "bottom");
        // Inside the 4:3 picture a television draws on 16:9 (160 either
        // side), at the grid's first column (a tenth of 960 in): 256, asked
        // for on the captions' stretched canvas as 256 · 3/4.
        assert_eq!(value(&at, "sub-margin-x"), "192");
        assert_eq!(value(&at, "sub-ass-override"), "strip");
        // A real roll-up a little higher, its first row indented by one.
        let p = placed(ROLL_UP);
        assert_eq!((p.side, p.last_row, p.start), (Side::Left, 11, 0.0));
        assert_eq!(value(&props(ROLL_UP, None), "sub-pos"), "74");
    }

    #[test]
    fn a_caption_centred_by_its_indents_is_centred() {
        let p = placed(CENTRED_BOTTOM);
        assert_eq!((p.side, p.last_row, p.rows), (Side::Center, 14, 2));
        assert_eq!(value(&props(CENTRED_BOTTOM, Some(16.0 / 9.0)), "sub-align-x"), "center");
        assert_eq!(placed(CENTRED_ONE_ROW).side, Side::Center);
    }

    #[test]
    fn a_caption_on_the_top_rows_is_near_the_top() {
        let p = placed(TOP_LEFT);
        assert_eq!((p.last_row, p.rows, p.side), (1, 2, Side::Left));
        let at = props(TOP_LEFT, Some(16.0 / 9.0));
        assert_eq!(value(&at, "sub-pos"), "21");
        // A single top row is kept on screen, its whole height.
        let one = placed("{\\an7}{\\pos(38,28)}TOP");
        assert_eq!(value(&caption_properties(Some(&one), None), "sub-pos"), "15");
    }

    #[test]
    fn a_caption_to_the_right_is_on_the_right() {
        let p = placed(RIGHT);
        assert_eq!((p.side, p.start, p.end), (Side::Right, 21.0, 26.0));
        let at = props(RIGHT, Some(16.0 / 9.0));
        assert_eq!(value(&at, "sub-align-x"), "right");
        // (160 + 960 · (0.1 + 0.025 · 6)) · 3/4
        assert_eq!(value(&at, "sub-margin-x"), "300");
        // Flush right over two ragged rows.
        let ragged = "{\\an7}{\\pos(144,228)}\\h\\h\\h\\hGOAL BY\\N{\\an7}{\\pos(144,243)}NUMBER NINE";
        assert_eq!(placed(ragged).side, Side::Right);
        // On 4:3 video the frame is the picture: no side bands.
        assert_eq!(value(&props(RIGHT, Some(4.0 / 3.0)), "sub-margin-x"), "240");
    }

    #[test]
    fn two_captions_on_screen_are_placed_by_the_first() {
        // As mpv's sub-text/ass joins them: the same caption twice, read
        // again where the stream started over.
        let twice = format!("{RIGHT}\n{RIGHT}");
        assert_eq!(placed(&twice), placed(RIGHT));
        let two = format!("{TOP_LEFT}\n{CENTRED_BOTTOM}");
        assert_eq!(placed(&two), placed(TOP_LEFT));
    }

    #[test]
    fn nothing_to_go_on_is_none() {
        assert_eq!(caption_placement(""), None);
        assert_eq!(caption_placement("Just text"), None);
        assert_eq!(caption_placement("{\\an7}{\\pos(oops)}TEXT"), None);
        assert_eq!(caption_placement("{\\an7}{\\pos(38,243)}   "), None);
    }

    fn track(codec: &str, selected: bool) -> TrackDto {
        TrackDto {
            id: 1,
            title: None,
            lang: Some("eng".into()),
            codec: Some(codec.into()),
            channels: None,
            default: false,
            forced: false,
            hearing_impaired: false,
            visual_impaired: false,
            selected,
        }
    }

    #[test]
    fn only_captions_shown_are_captions() {
        assert!(captions_selected(&[track("eia_608", true)]));
        assert!(!captions_selected(&[track("eia_608", false), track("ass", true)]));
        assert!(!captions_selected(&[track("subrip", true)]));
        assert!(!captions_selected(&[]));
    }

    #[test]
    fn captions_are_placed_and_everything_goes_back_after() {
        let mut c = Captions::new();
        let wide = Some(16.0 / 9.0);
        // A film's subtitles: mpv's own values, nothing to write.
        assert!(c.update(Some(false), Some(CENTRED_BOTTOM), wide).is_empty());
        // Captions chosen: drawn plainly at once, placed with the first one.
        assert_eq!(c.update(Some(true), None, wide), [("sub-ass-override", "strip".to_owned())]);
        let first = c.update(None, Some(TOP_LEFT), wide);
        assert_eq!(
            first,
            [
                ("sub-pos", "21".to_owned()),
                ("sub-align-x", "left".to_owned()),
                ("sub-margin-x", "192".to_owned()),
            ]
        );
        // The same place again: nothing. Nothing to go on: left as it is.
        assert!(c.update(None, Some(TOP_LEFT), wide).is_empty());
        assert!(c.update(None, Some("no position here"), wide).is_empty());
        // Moved to the bottom, centred.
        assert_eq!(
            c.update(None, Some(CENTRED_BOTTOM), wide),
            [
                ("sub-pos", "90".to_owned()),
                ("sub-align-x", "center".to_owned()),
                ("sub-margin-x", "19".to_owned()),
            ]
        );
        // Another track: back to mpv's own, for what was changed.
        assert_eq!(
            c.update(Some(false), None, wide),
            [("sub-ass-override", "scale".to_owned()), ("sub-pos", "100".to_owned())]
        );
        // And left alone after that, whatever the text.
        assert!(c.update(Some(false), Some(TOP_LEFT), wide).is_empty());
    }
}
