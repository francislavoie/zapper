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
//! as `sub-align-x` with a margin that keeps it inside the 4:3 frame a
//! television would draw the grid in, measured against where the picture
//! really is in the window.
//!
//! Pure but for one place to keep what was last written: the heartbeat in
//! `player_cmds` reads the track list, `sub-text/ass` and the window's
//! `osd-dimensions`, and sets what [`Captions::update`] says.

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

/// Everything set here. mpv's own values for them are read before the
/// first is written, and put back when captions are no longer shown.
pub const KEYS: [&str; 5] = ["sub-ass-override", "sub-align-y", "sub-pos", "sub-align-x", "sub-margin-x"];

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

/// mpv's `osd-dimensions`: the window (`w` × `h`) and the bands either side
/// of the picture in it (`ml`, `mr`, `mt`, `mb`), in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub w: f64,
    pub h: f64,
    pub ml: f64,
    pub mr: f64,
    pub mt: f64,
    pub mb: f64,
}

impl View {
    /// The picture's width and height in the window, when there is one.
    fn picture(&self) -> Option<(f64, f64)> {
        let (w, h) = (self.w - self.ml - self.mr, self.h - self.mt - self.mb);
        (self.w > 0.0 && self.h > 0.0 && w > 0.0 && h > 0.0).then_some((w, h))
    }
}

/// One row of the caption: its grid row, and the columns its characters
/// cover.
struct Row {
    row: u32,
    start: f64,
    end: f64,
}

/// The two numbers in `\pos(x,y)`, and where the text after it starts.
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
/// mpv joins the captions on screen with a line break, oldest first (a
/// roll-up's last line can linger beside the next caption); the newest one
/// is the one placed.
pub fn caption_placement(ass_text: &str) -> Option<CaptionPlacement> {
    let event = ass_text.split('\n').rev().find(|e| e.contains("\\pos("))?;
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
/// picture's height, as a share of the window's height (the picture may
/// have bands above and below it). Kept high enough for all its rows to be
/// seen.
fn sub_pos(p: &CaptionPlacement, view: Option<&View>) -> i64 {
    let share = INSET + f64::from(p.last_row + 1) * (1.0 - 2.0 * INSET) / ROWS;
    let bottom = match view.and_then(|v| v.picture().map(|(_, h)| (v, h))) {
        Some((v, h)) => (v.mt + h * share) / v.h * 100.0,
        None => share * 100.0,
    };
    let lowest = f64::from(p.rows) * 7.0 + 3.0;
    bottom.clamp(lowest, 100.0).round() as i64
}

/// `sub-margin-x` for a caption to one side: from the window's edge to the
/// caption's own first or last column on a television's grid, which sits in
/// a 4:3 frame centred in the picture.
///
/// mpv lays this margin from the window's edge (`sub-use-margins`, its
/// default), in the captions' own units: it becomes `m · 288 / 720` of the
/// decoder's 384 across the picture, so a margin `m` comes out as
/// `m · pw / 960` pixels for a picture `pw` wide, whatever bands are beside
/// it (measured in the window, 4:3 between bands and 16:9 filling it). So
/// the pixels wanted are asked for as `pixels · 960 / pw`. Without the
/// window's dimensions, the picture is taken to fill it, 4:3.
fn margin_x(p: &CaptionPlacement, view: Option<&View>) -> i64 {
    let columns = match p.side {
        Side::Left => p.start,
        Side::Right => COLUMNS - p.end,
        Side::Center => return 0,
    };
    let into_frame = INSET + COLUMN_STEP * columns;
    let Some((v, (pw, ph))) = view.and_then(|v| v.picture().map(|pic| (v, pic))) else {
        return (960.0 * into_frame).round() as i64;
    };
    let frame = (ph * 4.0 / 3.0).min(pw);
    let band = match p.side {
        Side::Right => v.mr,
        _ => v.ml,
    };
    let pixels = band + (pw - frame) / 2.0 + frame * into_frame;
    (pixels * 960.0 / pw).round() as i64
}

/// The properties that draw captions placed at `p` (or, before any
/// placement is known, only plainly in the viewer's style). A centred
/// caption keeps the margin there was (`None` for `sub-margin-x`).
pub fn caption_properties(p: Option<&CaptionPlacement>, view: Option<&View>) -> Vec<(&'static str, Option<String>)> {
    let mut out = vec![("sub-ass-override", Some("strip".to_owned()))];
    if let Some(p) = p {
        out.push(("sub-align-y", Some("bottom".to_owned())));
        out.push(("sub-pos", Some(sub_pos(p, view).to_string())));
        out.push((
            "sub-align-x",
            Some(
                match p.side {
                    Side::Left => "left",
                    Side::Center => "center",
                    Side::Right => "right",
                }
                .to_owned(),
            ),
        ));
        out.push((
            "sub-margin-x",
            (p.side != Side::Center).then(|| margin_x(p, view).to_string()),
        ));
    }
    out
}

/// Whether the subtitle track chosen is closed captions.
pub fn captions_selected(subtitles: &[TrackDto]) -> bool {
    subtitles
        .iter()
        .any(|t| t.selected && t.codec.as_deref() == Some(CAPTION_CODEC))
}

/// What captions are being drawn with, what mpv had before, and what was
/// last written to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Captions {
    on: bool,
    placement: Option<CaptionPlacement>,
    /// mpv's own values for [`KEYS`], read before the first write.
    original: Vec<(&'static str, String)>,
    written: Vec<(&'static str, String)>,
}

impl Default for Captions {
    fn default() -> Self {
        Self::new()
    }
}

impl Captions {
    pub const fn new() -> Self {
        Self {
            on: false,
            placement: None,
            original: Vec::new(),
            written: Vec::new(),
        }
    }

    /// Whether captions are the track chosen: the window is then watched
    /// for a change of size.
    pub fn on(&self) -> bool {
        self.on
    }

    fn value(list: &[(&'static str, String)], key: &str) -> Option<String> {
        list.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
    }

    /// What to set now, given whether captions are the track chosen
    /// (`selected`, when the track list said), the caption on screen
    /// (`text`, when it changed) and the window (`view`). `read` gives
    /// mpv's own value of a property, asked for once, before anything is
    /// written. Only what differs from what is there; and everything
    /// touched goes back to mpv's own values once captions are not chosen.
    pub fn update(
        &mut self,
        selected: Option<bool>,
        text: Option<&str>,
        view: Option<&View>,
        read: impl Fn(&str) -> Option<String>,
    ) -> Vec<(&'static str, String)> {
        if let Some(on) = selected {
            if on != self.on {
                self.on = on;
                self.placement = None;
            }
        }
        if !self.on {
            let back: Vec<(&'static str, String)> = self
                .written
                .iter()
                .filter_map(|(k, v)| {
                    let was = Self::value(&self.original, k)?;
                    (*v != was).then_some((*k, was))
                })
                .collect();
            self.written.clear();
            return back;
        }
        if let Some(p) = text.and_then(caption_placement) {
            self.placement = Some(p);
        }
        if self.original.is_empty() {
            self.original = KEYS.iter().filter_map(|k| Some((*k, read(k)?))).collect();
        }
        let changes: Vec<(&'static str, String)> = caption_properties(self.placement.as_ref(), view)
            .into_iter()
            .filter_map(|(k, v)| {
                // A centred caption leaves the margin as it was before
                // captions: what mpv had.
                let v = v.or_else(|| Self::value(&self.original, k))?;
                let now = Self::value(&self.written, k).or_else(|| Self::value(&self.original, k));
                (now.as_deref() != Some(v.as_str())).then_some((k, v))
            })
            .collect();
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

    // Events shaped as FFmpeg's decoder writes them (from its own caption
    // samples, and synthetic captions with made-up text).
    const CENTRED_BOTTOM: &str =
        "{\\an7}{\\pos(67,228)}\\h\\h\\hIl fait beau ce soir\\N{\\an7}{\\pos(67,243)}et demain, il pleuvra fort";
    const CENTRED_ONE_ROW: &str = "{\\an7}{\\pos(96,243)}Le chat dort encore";
    const TOP_LEFT: &str = "{\\an7}{\\pos(38,28)}({\\i1} inaudible radio chatter{\\i0} )\\N{\\an7}{\\pos(38,44)}>> Safety remains our number one";
    const ROLL_UP: &str = "{\\an7}{\\pos(38,166)}\\hBESIDES THE \\N{\\an7}{\\pos(38,197)}SPENDING AND THIS, IS THAT CAR ";
    const BOTTOM_ROLL_UP: &str =
        "{\\an7}{\\pos(38,228)}SPENDING AND THIS, IS THAT CAR \\N{\\an7}{\\pos(38,243)}MANUFACTURERS ARE ABOUT AS ";
    const RIGHT: &str = "{\\an7}{\\pos(240,243)}ARENA";

    /// A 16:9 picture filling a 16:9 window.
    const WIDE: View = View { w: 1280.0, h: 720.0, ml: 0.0, mr: 0.0, mt: 0.0, mb: 0.0 };
    /// A 4:3 picture in a 16:9 window: bands left and right.
    const PILLARBOX: View = View { w: 1280.0, h: 720.0, ml: 160.0, mr: 160.0, mt: 0.0, mb: 0.0 };
    /// A 16:9 picture in a 4:3 window: bands above and below.
    const LETTERBOX: View = View { w: 960.0, h: 720.0, ml: 0.0, mr: 0.0, mt: 90.0, mb: 90.0 };

    fn placed(text: &str) -> CaptionPlacement {
        caption_placement(text).expect("a placement")
    }

    fn props(text: &str, view: Option<&View>) -> Vec<(&'static str, Option<String>)> {
        caption_properties(Some(&placed(text)), view)
    }

    fn value(props: &[(&'static str, Option<String>)], key: &str) -> Option<String> {
        props.iter().find(|(k, _)| *k == key).and_then(|(_, v)| v.clone())
    }

    /// Where the caption's left edge lands in the window, in pixels, for a
    /// `sub-margin-x` of `m`: what mpv draws (see `margin_x`).
    fn left_px(m: &str, view: &View) -> f64 {
        m.parse::<f64>().unwrap() * (view.w - view.ml - view.mr) / 960.0
    }

    #[test]
    fn a_roll_up_at_the_first_column_stays_left() {
        let p = placed(BOTTOM_ROLL_UP);
        assert_eq!((p.side, p.last_row, p.rows, p.start), (Side::Left, 14, 2, 0.0));
        let at = props(BOTTOM_ROLL_UP, Some(&WIDE));
        // The bottom of the grid, which is a tenth of the way up the picture.
        assert_eq!(value(&at, "sub-pos").as_deref(), Some("90"));
        assert_eq!(value(&at, "sub-align-x").as_deref(), Some("left"));
        assert_eq!(value(&at, "sub-align-y").as_deref(), Some("bottom"));
        // Inside the 4:3 frame (160 in on 16:9), at the grid's first
        // column, a tenth of 960 further: 256 pixels, asked for as 192.
        assert_eq!(value(&at, "sub-margin-x").as_deref(), Some("192"));
        assert_eq!(value(&at, "sub-ass-override").as_deref(), Some("strip"));
        // A real roll-up a little higher, its first row indented by one.
        let p = placed(ROLL_UP);
        assert_eq!((p.side, p.last_row, p.start), (Side::Left, 11, 0.0));
        assert_eq!(value(&props(ROLL_UP, None), "sub-pos").as_deref(), Some("74"));
    }

    #[test]
    fn side_captions_are_placed_against_the_picture_in_the_window() {
        // 16:9 in 16:9: the frame's left edge 160 in, column 0 at 256.
        let wide = value(&props(BOTTOM_ROLL_UP, Some(&WIDE)), "sub-margin-x").unwrap();
        assert_eq!(left_px(&wide, &WIDE), 256.0);
        // 4:3 pillarboxed in 16:9: the picture is the frame, 160 in, so
        // column 0 is 96 into the picture, not in the band.
        let pillar = value(&props(BOTTOM_ROLL_UP, Some(&PILLARBOX)), "sub-margin-x").unwrap();
        assert_eq!(left_px(&pillar, &PILLARBOX), 160.0 + 96.0);
        // 16:9 letterboxed in 4:3: the picture fills the width; its 4:3
        // frame is 720 wide, 120 in, and column 0 is 72 further.
        let letter = value(&props(BOTTOM_ROLL_UP, Some(&LETTERBOX)), "sub-margin-x").unwrap();
        assert_eq!(left_px(&letter, &LETTERBOX), 120.0 + 72.0);
        // And the bottom row sits on the picture, not in the band below:
        // 90 + 540 · 0.9 = 576 of 720, 80%.
        assert_eq!(value(&props(BOTTOM_ROLL_UP, Some(&LETTERBOX)), "sub-pos").as_deref(), Some("80"));
        // Without the window, a 4:3 picture filling it.
        assert_eq!(value(&props(BOTTOM_ROLL_UP, None), "sub-margin-x").as_deref(), Some("96"));
    }

    #[test]
    fn a_caption_centred_by_its_indents_is_centred() {
        let p = placed(CENTRED_BOTTOM);
        assert_eq!((p.side, p.last_row, p.rows), (Side::Center, 14, 2));
        let at = props(CENTRED_BOTTOM, Some(&WIDE));
        assert_eq!(value(&at, "sub-align-x").as_deref(), Some("center"));
        // Centred: the margin there was.
        assert_eq!(value(&at, "sub-margin-x"), None);
        assert_eq!(placed(CENTRED_ONE_ROW).side, Side::Center);
    }

    #[test]
    fn a_caption_on_the_top_rows_is_near_the_top() {
        let p = placed(TOP_LEFT);
        assert_eq!((p.last_row, p.rows, p.side), (1, 2, Side::Left));
        assert_eq!(value(&props(TOP_LEFT, Some(&WIDE)), "sub-pos").as_deref(), Some("21"));
        // A single top row is kept on screen, its whole height.
        let one = placed("{\\an7}{\\pos(38,28)}TOP");
        assert_eq!(value(&caption_properties(Some(&one), None), "sub-pos").as_deref(), Some("15"));
    }

    #[test]
    fn a_caption_to_the_right_is_on_the_right() {
        let p = placed(RIGHT);
        assert_eq!((p.side, p.start, p.end), (Side::Right, 21.0, 26.0));
        let at = props(RIGHT, Some(&WIDE));
        assert_eq!(value(&at, "sub-align-x").as_deref(), Some("right"));
        // (160 + 960 · (0.1 + 0.025 · 6)) · 960 / 1280, the picture's width
        assert_eq!(value(&at, "sub-margin-x").as_deref(), Some("300"));
        // Flush right over two ragged rows.
        let ragged = "{\\an7}{\\pos(144,228)}\\h\\h\\h\\hGOAL BY\\N{\\an7}{\\pos(144,243)}NUMBER NINE";
        assert_eq!(placed(ragged).side, Side::Right);
    }

    #[test]
    fn the_newest_caption_on_screen_is_the_one_placed() {
        // As mpv's sub-text/ass joins them, oldest first: a roll-up's line
        // still up at the top beside a new caption at the bottom.
        let two = format!("{TOP_LEFT}\n{CENTRED_BOTTOM}");
        assert_eq!(placed(&two), placed(CENTRED_BOTTOM));
        let twice = format!("{RIGHT}\n{RIGHT}");
        assert_eq!(placed(&twice), placed(RIGHT));
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
    fn only_captions_chosen_are_captions() {
        assert!(captions_selected(&[track("eia_608", true)]));
        assert!(!captions_selected(&[track("eia_608", false), track("ass", true)]));
        assert!(!captions_selected(&[track("subrip", true)]));
        assert!(!captions_selected(&[]));
    }

    /// mpv's own values, as it reads them back (not as its manual has them).
    fn mpv_reads(key: &str) -> Option<String> {
        Some(
            match key {
                "sub-ass-override" => "scale",
                "sub-align-y" => "bottom",
                "sub-pos" => "100.000000",
                "sub-align-x" => "center",
                "sub-margin-x" => "19",
                _ => return None,
            }
            .to_owned(),
        )
    }

    #[test]
    fn captions_are_placed_and_mpvs_own_values_go_back_after() {
        let mut c = Captions::new();
        let wide = Some(&WIDE);
        let never = |_: &str| -> Option<String> { panic!("nothing read before captions") };
        // A film's subtitles: nothing read, nothing written.
        assert!(c.update(Some(false), Some(CENTRED_BOTTOM), wide, never).is_empty());
        // Captions chosen: drawn plainly at once, placed with the first one.
        assert_eq!(c.update(Some(true), None, wide, mpv_reads), [("sub-ass-override", "strip".to_owned())]);
        assert_eq!(
            c.update(None, Some(TOP_LEFT), wide, mpv_reads),
            [
                ("sub-pos", "21".to_owned()),
                ("sub-align-x", "left".to_owned()),
                ("sub-margin-x", "192".to_owned()),
            ]
        );
        // The same place again: nothing. Nothing to go on: left as it is.
        assert!(c.update(None, Some(TOP_LEFT), wide, mpv_reads).is_empty());
        assert!(c.update(None, Some("no position here"), wide, mpv_reads).is_empty());
        // The window changes shape: placed again. A 4:3 picture in a window
        // a little wider: (93.5 + 126.5) · 960 / 1265.
        let window = View { w: 1452.0, h: 949.0, ml: 93.5, mr: 93.5, mt: 0.0, mb: 0.0 };
        assert_eq!(
            c.update(None, None, Some(&window), mpv_reads),
            [("sub-margin-x", "167".to_owned())]
        );
        // Moved to the bottom, centred: the margin mpv had.
        assert_eq!(
            c.update(None, Some(CENTRED_BOTTOM), wide, mpv_reads),
            [
                ("sub-pos", "90".to_owned()),
                ("sub-align-x", "center".to_owned()),
                ("sub-margin-x", "19".to_owned()),
            ]
        );
        // Another track: mpv's own values back, as it read them, for what
        // was changed.
        assert_eq!(
            c.update(Some(false), None, wide, never),
            [("sub-ass-override", "scale".to_owned()), ("sub-pos", "100.000000".to_owned())]
        );
        // And left alone after that, whatever the text.
        assert!(c.update(Some(false), Some(TOP_LEFT), wide, never).is_empty());
    }
}
