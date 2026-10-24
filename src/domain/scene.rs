//! Text laid out as dots.
//!
//! A [`Scene`] is laid out text. Lines that fit are static; lines wider than
//! the display scroll as a looping ticker, one dot per tick.

use embedded_graphics::mono_font::iso_8859_1::{
    FONT_4X6, FONT_5X7, FONT_5X8, FONT_6X10, FONT_7X14,
};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};

use crate::domain::dots::FrameBuffer;

/// Blank space between the end and the restart of a scrolling line, in
/// character cells.
const TICKER_GAP_CHARS: u32 = 2;

/// Splits a line in two: the text before it is set at the left edge, the
/// text after it at the right edge. Such a line does not scroll, so its
/// values can change in place. Where both parts do not fit side by side,
/// it is a space and the line scrolls like any line that is too wide.
pub const FILL: char = '\t';

/// Least blank space between the two parts of a line split by [`FILL`], in
/// dots, on top of the blank column every glyph ends in.
const FILL_GAP: u32 = 1;

/// Which lines scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// Only lines wider than the display scroll.
    Auto,
    /// Every line scrolls, also when it would fit.
    Always,
}

/// Fixed-width fonts covering ISO 8859-1 (ASCII plus Western European
/// characters such as ä, é, ß and °). Other characters are drawn as `?`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// 7 characters and 2 lines per panel. Tiny.
    F4x6,
    /// 5 characters and 2 lines per panel. The classic dot matrix font.
    F5x7,
    /// 5 characters and 2 lines per panel, filling its full height.
    F5x8,
    /// 4 characters and 1 line per panel.
    F6x10,
    /// 4 characters and 1 line per panel, the largest letters.
    F7x14,
}

impl Font {
    fn mono(self) -> &'static MonoFont<'static> {
        match self {
            Font::F4x6 => &FONT_4X6,
            Font::F5x7 => &FONT_5X7,
            Font::F5x8 => &FONT_5X8,
            Font::F6x10 => &FONT_6X10,
            Font::F7x14 => &FONT_7X14,
        }
    }

    /// Size of one character cell in dots.
    pub fn cell_size(self) -> (u32, u32) {
        let cell = self.mono().character_size;
        (cell.width, cell.height)
    }
}

/// Horizontal alignment of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// One line that is wider than the display, pre-rendered at full width
/// plus a gap so it can loop seamlessly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ticker {
    /// Which of the lines this is.
    line: usize,
    strip: FrameBuffer,
    /// Row of the display the strip is drawn at.
    y: u32,
}

/// Text laid out for a display. Produces one [`FrameBuffer`] per tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scene {
    /// All lines that fit, already drawn.
    base: FrameBuffer,
    /// Lines that scroll.
    tickers: Vec<Ticker>,
    /// Ticks a scrolling line rests at its start before each pass.
    hold: u32,
    /// Number of lines that fit vertically, if some did not.
    lines_shown: Option<usize>,
    total_lines: usize,
}

impl Scene {
    /// Lay out `lines` of UTF-8 text. Lines wider than the display scroll.
    #[cfg(test)]
    pub fn new(lines: &[&str], font: Font, align: Align, width: u32, height: u32) -> Self {
        Self::build(lines, font, align, width, height, Scroll::Auto)
    }

    /// Lay out `lines` of UTF-8 text. The block of lines is centred
    /// vertically. Lines beyond the display height are dropped.
    pub fn build(
        lines: &[&str],
        font: Font,
        align: Align,
        width: u32,
        height: u32,
        scroll: Scroll,
    ) -> Self {
        let mut base = FrameBuffer::new(width, height);
        let mut tickers = Vec::new();
        let mono = font.mono();
        let cell = mono.character_size;
        let advance = cell.width + mono.character_spacing;

        let fitting = (height / cell.height) as usize;
        let lines_shown = (lines.len() > fitting).then_some(fitting);
        let shown = &lines[..lines.len().min(fitting)];
        let block_height = shown.len() as u32 * cell.height;
        let top = height.saturating_sub(block_height) / 2;

        let (alignment, x) = match align {
            Align::Left => (Alignment::Left, 0),
            Align::Center => (Alignment::Center, width as i32 / 2),
            Align::Right => (Alignment::Right, width as i32 - 1),
        };
        let character_style = MonoTextStyle::new(mono, BinaryColor::On);
        let text_style = |alignment| {
            TextStyleBuilder::new()
                .alignment(alignment)
                .baseline(Baseline::Top)
                .build()
        };

        // The last character needs no trailing spacing.
        let text_width = |text: &str| {
            (text.chars().count() as u32 * advance).saturating_sub(mono.character_spacing)
        };

        for (i, line) in shown.iter().enumerate() {
            let y = top + i as u32 * cell.height;

            let joined;
            let mut line: &str = line;
            if let Some((left, right)) = line.split_once(FILL) {
                // Every glyph ends in a blank column, so one more dot is
                // enough to keep the two parts apart.
                let gap = if left.is_empty() || right.is_empty() {
                    0
                } else {
                    FILL_GAP
                };
                let right = right.replace(FILL, " ");
                let fits = text_width(left) + gap + text_width(&right) <= width;
                if fits {
                    for (text, alignment, x) in [
                        (left, Alignment::Left, 0),
                        (right.as_str(), Alignment::Right, width as i32 - 1),
                    ] {
                        let position = Point::new(x, y as i32);
                        let _ = Text::with_text_style(
                            text,
                            position,
                            character_style,
                            text_style(alignment),
                        )
                        .draw(&mut base);
                    }
                    continue;
                }
                joined = format!("{left} {right}").trim().to_string();
                line = &joined;
            }
            let chars = line.chars().count() as u32;
            let line_width = text_width(line);

            // Drawing into a frame buffer cannot fail.
            let scrolls = line_width > width || (scroll == Scroll::Always && chars > 0);
            if !scrolls {
                let position = Point::new(x, y as i32);
                let _ =
                    Text::with_text_style(line, position, character_style, text_style(alignment))
                        .draw(&mut base);
            } else {
                // Alignment is meaningless here, the line scrolls. A line
                // wider than the display loops as a ticker with a small gap.
                // A line that fits leaves the display completely before it
                // comes back in from the right.
                let gap = if line_width > width {
                    TICKER_GAP_CHARS * advance
                } else {
                    width
                };
                let strip_width = line_width + gap;
                let mut strip = FrameBuffer::new(strip_width, cell.height);
                let _ = Text::with_text_style(
                    line,
                    Point::zero(),
                    character_style,
                    text_style(Alignment::Left),
                )
                .draw(&mut strip);
                tickers.push(Ticker { line: i, strip, y });
            }
        }

        Self {
            base,
            tickers,
            hold: 0,
            lines_shown,
            total_lines: lines.len(),
        }
    }

    /// Rest scrolling lines at their start for `ticks` before each pass.
    pub fn with_hold(mut self, ticks: u32) -> Self {
        self.hold = ticks;
        self
    }

    /// True if at least one line scrolls.
    pub fn scrolls(&self) -> bool {
        !self.tickers.is_empty()
    }

    /// True if line `line` scrolls.
    pub fn line_scrolls(&self, line: usize) -> bool {
        self.tickers.iter().any(|t| t.line == line)
    }

    /// True if `other` scrolls the same text on the same lines, so that it
    /// can take over at any tick without the scrolling text jumping.
    pub fn scrolls_like(&self, other: &Scene) -> bool {
        self.tickers == other.tickers && self.hold == other.hold
    }

    /// Ticks until every scrolling line has gone round at least once and
    /// the longest one is back at its start. `None` if nothing scrolls.
    pub fn cycle_ticks(&self) -> Option<u64> {
        self.tickers
            .iter()
            .map(|t| u64::from(self.hold) + u64::from(t.strip.width()))
            .max()
    }

    /// The display contents at `tick`. Scrolling lines move one dot per tick.
    pub fn frame(&self, tick: u64) -> FrameBuffer {
        let mut fb = self.base.clone();
        for ticker in &self.tickers {
            let strip = &ticker.strip;
            // One pass: rest at the start, then scroll the whole strip by.
            // After `strip.width()` dots the text is back at its start.
            let cycle = u64::from(self.hold) + u64::from(strip.width());
            let offset = (tick % cycle).saturating_sub(u64::from(self.hold)) as u32;
            for y in 0..strip.height() {
                for x in 0..fb.width() {
                    fb.set(x, ticker.y + y, strip.get((x + offset) % strip.width(), y));
                }
            }
        }
        fb
    }

    /// What was asked for but cannot be shown.
    pub fn warnings(&self) -> Vec<String> {
        self.lines_shown
            .map(|shown| {
                format!(
                    "{} lines given, only {shown} fit on the display",
                    self.total_lines
                )
            })
            .into_iter()
            .collect()
    }
}

/// How text is set on a display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typesetting {
    /// Size of the display in dots.
    pub width: u32,
    pub height: u32,
    pub font: Font,
    pub align: Align,
    pub scroll: Scroll,
    /// Ticks a scrolling line rests at its start before each pass.
    pub hold: u32,
}

impl Typesetting {
    /// Lay out `lines`.
    pub fn scene<S: AsRef<str>>(&self, lines: &[S]) -> Scene {
        let refs: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();
        Scene::build(
            &refs,
            self.font,
            self.align,
            self.width,
            self.height,
            self.scroll,
        )
        .with_hold(self.hold)
    }

    /// A display with nothing on it.
    pub fn blank(&self) -> FrameBuffer {
        FrameBuffer::new(self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 28x14 panel, which keeps the expected dot positions small.
    const WIDTH: u32 = 28;
    const HEIGHT: u32 = 14;

    fn scene1(lines: &[&str], font: Font, align: Align) -> Scene {
        Scene::new(lines, font, align, WIDTH, HEIGHT)
    }

    fn render(lines: &[&str], font: Font, align: Align) -> FrameBuffer {
        scene1(lines, font, align).frame(0)
    }

    /// Dots as rows of `#` and `.`, for readable assertions.
    fn rows(fb: &FrameBuffer) -> Vec<String> {
        (0..fb.height())
            .map(|y| {
                (0..fb.width())
                    .map(|x| if fb.get(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    fn lit(fb: &FrameBuffer) -> usize {
        (0..fb.height())
            .flat_map(|y| (0..fb.width()).map(move |x| (x, y)))
            .filter(|(x, y)| fb.get(*x, *y))
            .count()
    }

    /// Leftmost and rightmost lit column.
    fn extent(fb: &FrameBuffer) -> (u32, u32) {
        let cols: Vec<u32> = (0..fb.width())
            .filter(|x| (0..fb.height()).any(|y| fb.get(*x, y)))
            .collect();
        (cols[0], *cols.last().unwrap())
    }

    /// `fb` shifted left by `by` dots, wrapping around at `fb.width()`.
    fn rotated(fb: &FrameBuffer, by: u32) -> FrameBuffer {
        let mut out = FrameBuffer::new(fb.width(), fb.height());
        for y in 0..fb.height() {
            for x in 0..fb.width() {
                out.set(x, y, fb.get((x + by) % fb.width(), y));
            }
        }
        out
    }

    #[test]
    fn empty_text_leaves_the_display_dark() {
        let scene = scene1(&[], Font::F5x7, Align::Left);
        assert_eq!(lit(&scene.frame(0)), 0);
        assert!(!scene.scrolls());
        assert!(scene.warnings().is_empty());
    }

    #[test]
    fn letter_i_is_drawn_as_expected_dots() {
        let fb = Scene::new(&["I"], Font::F5x7, Align::Left, 5, 7).frame(0);
        let r = rows(&fb);
        // A capital I: serifs top and bottom, a vertical stroke between.
        assert_eq!(r[0], r[5], "top and bottom serif match: {r:#?}");
        assert!(r[0].matches('#').count() >= 3, "{r:#?}");
        for row in &r[1..5] {
            assert_eq!(row.matches('#').count(), 1, "single stroke: {r:#?}");
        }
    }

    #[test]
    fn two_lines_of_5x7_fill_the_panel_height() {
        let scene = scene1(&["AB", "CD"], Font::F5x7, Align::Left);
        assert!(scene.warnings().is_empty());
        let r = rows(&scene.frame(0));
        assert!(
            r[..7].iter().any(|row| row.contains('#')),
            "first line drawn"
        );
        assert!(
            r[7..].iter().any(|row| row.contains('#')),
            "second line drawn"
        );
    }

    #[test]
    fn single_line_is_centred_vertically() {
        let r = rows(&render(&["A"], Font::F5x7, Align::Left));
        // 7 dot font on 14 rows: 3 blank rows above.
        assert!(r[..3].iter().all(|row| !row.contains('#')), "{r:#?}");
        assert!(r[3].contains('#'), "{r:#?}");
        assert!(r[11..].iter().all(|row| !row.contains('#')), "{r:#?}");
    }

    #[test]
    fn alignment_moves_the_text() {
        let left = render(&["A"], Font::F5x7, Align::Left);
        let center = render(&["A"], Font::F5x7, Align::Center);
        let right = render(&["A"], Font::F5x7, Align::Right);
        assert_eq!(lit(&left), lit(&center));
        assert_eq!(lit(&left), lit(&right));
        assert!(extent(&left).1 < 5, "left: {:?}", extent(&left));
        let (c0, c1) = extent(&center);
        assert!(c0 >= 10 && c1 <= 17, "center: {c0}..{c1}");
        assert!(extent(&right).0 >= 23, "right: {:?}", extent(&right));
    }

    #[test]
    fn fill_sets_the_parts_at_both_edges() {
        let fill = render(&["A\tB"], Font::F5x7, Align::Center);
        let a = render(&["A"], Font::F5x7, Align::Left);
        let b = render(&["B"], Font::F5x7, Align::Right);
        assert_eq!(lit(&fill), lit(&a) + lit(&b));
        assert_eq!(extent(&fill), (extent(&a).0, extent(&b).1));
        assert!(!scene1(&["A\tB"], Font::F5x7, Align::Left).scrolls());
    }

    #[test]
    fn fill_with_one_part_empty_aligns_the_other() {
        assert_eq!(
            render(&["\tB"], Font::F5x7, Align::Left),
            render(&["B"], Font::F5x7, Align::Right)
        );
        assert_eq!(
            render(&["A\t"], Font::F5x7, Align::Right),
            render(&["A"], Font::F5x7, Align::Left)
        );
    }

    #[test]
    fn fill_that_does_not_fit_scrolls_as_a_space() {
        // 4 + 1 + 2 characters do not fit 5.
        let scene = scene1(&["ABCD\tEF"], Font::F5x7, Align::Left);
        assert!(scene.scrolls());
        assert_eq!(scene, scene1(&["ABCD EF"], Font::F5x7, Align::Left));
        assert_eq!(
            scene1(&["\tABCDEF"], Font::F5x7, Align::Left),
            scene1(&["ABCDEF"], Font::F5x7, Align::Left),
            "no gap for an empty part"
        );
        // Exactly 5 characters fit: D goes to the right edge.
        let fits = render(&["ABC\tD"], Font::F5x7, Align::Left);
        assert_ne!(fits, render(&["ABC D"], Font::F5x7, Align::Left));
        assert!(extent(&fits).1 >= 23, "{:?}", extent(&fits));
    }

    #[test]
    fn speed_and_altitude_fit_two_panels() {
        // 11 characters of 5 dots and a gap of 1: exactly 56 dots.
        let two = |line| Scene::new(&[line], Font::F5x7, Align::Left, 56, 14);
        assert!(!two("145kt\t1000ft").scrolls());
        assert!(two("145kt\t10000ft").scrolls(), "12 characters do not fit");
        let fb = two("145kt\t1000ft").frame(0);
        let blank = |x| (0..14).all(|y| !fb.get(x, y));
        assert!(
            blank(24) && blank(25),
            "two blank columns between the parts"
        );
    }

    #[test]
    fn fill_stands_still_with_scroll_always() {
        let always = |line| Scene::build(&[line], Font::F5x7, Align::Left, 28, 14, Scroll::Always);
        assert!(!always("A\tB").scrolls());
        assert_eq!(
            always("A\tB").frame(0),
            render(&["A\tB"], Font::F5x7, Align::Left)
        );
    }

    #[test]
    fn scenes_that_scroll_the_same_lines_alike_can_take_over() {
        let scene = |lines: &[&str]| scene1(lines, Font::F5x7, Align::Left);
        let wide = "FLIGHT 1234";
        let a = scene(&[wide, "1\t2"]);
        assert!(a.line_scrolls(0) && !a.line_scrolls(1));
        assert!(a.scrolls_like(&scene(&[wide, "3\t4"])));
        assert!(!a.scrolls_like(&scene(&["FLIGHT 5678", "1\t2"])));
        assert!(!a.scrolls_like(&scene(&["AB", "1\t2"])));
    }

    #[test]
    fn text_that_fits_does_not_scroll() {
        // 5 characters * 5 dots = 25 <= 28.
        let scene = scene1(&["FL390"], Font::F5x7, Align::Left);
        assert!(!scene.scrolls());
        assert_eq!(scene.frame(0), scene.frame(17), "static text never moves");
    }

    #[test]
    fn wide_line_scrolls_one_dot_per_tick() {
        // 6 characters * 5 dots = 30 > 28, so this line scrolls. The strip is
        // 30 dots of text plus a 2 character gap = 40 dots.
        let scene = scene1(&["SAS51B"], Font::F5x7, Align::Left);
        assert!(scene.scrolls());
        let strip = &scene.tickers[0].strip;
        assert_eq!((strip.width(), strip.height()), (40, 7));

        for tick in [0u64, 1, 2, 13, 39] {
            let frame = scene.frame(tick);
            let expected = rotated(strip, tick as u32);
            for y in 0..7 {
                for x in 0..WIDTH {
                    // The single line is centred: it starts at row 3.
                    assert_eq!(
                        frame.get(x, y + 3),
                        expected.get(x, y),
                        "tick {tick} at {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn scrolling_loops_seamlessly() {
        let scene = scene1(&["SAS51B"], Font::F5x7, Align::Left);
        assert_ne!(scene.frame(0), scene.frame(1));
        assert_eq!(scene.frame(0), scene.frame(40), "one full pass later");
        assert_eq!(scene.frame(7), scene.frame(47));
    }

    #[test]
    fn hold_rests_at_the_start_before_each_pass() {
        let scene = scene1(&["SAS51B"], Font::F5x7, Align::Left).with_hold(10);
        let start = scene.frame(0);
        for tick in 1..=10 {
            assert_eq!(scene.frame(tick), start, "still resting at tick {tick}");
        }
        assert_ne!(scene.frame(11), start, "moving after the rest");
        // Cycle is 10 ticks rest + 40 dots.
        assert_eq!(scene.frame(50), start);
        assert_eq!(scene.frame(61), scene.frame(11));
    }

    #[test]
    fn cycle_is_rest_plus_one_pass_of_the_longest_line() {
        assert_eq!(
            scene1(&["FL390"], Font::F5x7, Align::Left).cycle_ticks(),
            None
        );
        let scene = scene1(&["SAS51B"], Font::F5x7, Align::Left);
        assert_eq!(scene.cycle_ticks(), Some(40));
        assert_eq!(scene.clone().with_hold(10).cycle_ticks(), Some(50));
        // Two scrolling lines: 6 and 8 characters, 40 and 50 dot strips.
        let two = scene1(&["SAS51B", "SAS51BXY"], Font::F5x7, Align::Left).with_hold(10);
        assert_eq!(two.cycle_ticks(), Some(60));
    }

    #[test]
    fn scroll_always_moves_a_line_that_fits() {
        let fits = Scene::build(&["AB"], Font::F5x7, Align::Left, 28, 14, Scroll::Always);
        assert!(fits.scrolls());
        // The strip is the text plus one display width: the text leaves
        // completely before it comes back in.
        assert_eq!(fits.tickers[0].strip.width(), 10 + 28);
        assert_eq!(
            fits.frame(0),
            render(&["AB"], Font::F5x7, Align::Left),
            "starts at the left"
        );
        assert_ne!(fits.frame(1), fits.frame(0));
        assert_eq!(lit(&fits.frame(10)), 0, "scrolled out of view");
        assert_ne!(lit(&fits.frame(20)), 0, "coming back in from the right");
        assert_eq!(fits.frame(38), fits.frame(0), "and back at the start");

        // An empty line has nothing to scroll.
        let empty = Scene::build(&["", "AB"], Font::F5x7, Align::Left, 28, 14, Scroll::Always);
        assert_eq!(empty.tickers.len(), 1);
        // Auto leaves a fitting line alone.
        assert!(!Scene::build(&["AB"], Font::F5x7, Align::Left, 28, 14, Scroll::Auto).scrolls());
    }

    #[test]
    fn only_the_wide_line_scrolls() {
        let scene = scene1(&["SAS51B", "FL390"], Font::F5x7, Align::Left);
        assert_eq!(scene.tickers.len(), 1);
        let (a, b) = (rows(&scene.frame(0)), rows(&scene.frame(5)));
        assert_ne!(a[..7], b[..7], "first line moved");
        assert_eq!(a[7..], b[7..], "second line stayed");
    }

    #[test]
    fn wide_line_fits_on_two_panels() {
        let scene = Scene::new(&["SAS51B"], Font::F5x7, Align::Left, 56, 14);
        assert!(!scene.scrolls());
    }

    #[test]
    fn callsign_fits_one_panel_in_the_small_font() {
        let scene = scene1(&["SAS51B", "FL390"], Font::F4x6, Align::Left);
        assert!(!scene.scrolls());
        assert!(scene.warnings().is_empty());
    }

    #[test]
    fn extra_lines_are_dropped_and_reported() {
        let scene = scene1(&["A", "B", "C"], Font::F5x7, Align::Left);
        assert_eq!(
            scene.warnings(),
            ["3 lines given, only 2 fit on the display"]
        );
        assert_eq!(
            scene.frame(0),
            render(&["A", "B"], Font::F5x7, Align::Left),
            "third line is not drawn"
        );
    }

    #[test]
    fn latin1_characters_have_their_own_glyphs() {
        let umlaut = render(&["ä"], Font::F5x7, Align::Left);
        assert_ne!(
            umlaut,
            render(&["a"], Font::F5x7, Align::Left),
            "ä differs from a"
        );
        assert_ne!(
            umlaut,
            render(&["?"], Font::F5x7, Align::Left),
            "ä is not the fallback"
        );
    }

    #[test]
    fn unsupported_characters_fall_back_to_question_mark() {
        assert_eq!(
            render(&["↑"], Font::F5x7, Align::Left),
            render(&["?"], Font::F5x7, Align::Left)
        );
    }
}
