//! `debug flipdot`: render UTF-8 text as dots, the way the flipdot panel will
//! show it.
//!
//! [`FrameBuffer`] is an `embedded-graphics` draw target. Everything that
//! draws text goes through it, and every frame reaches the terminal preview
//! through the same [`Display`] as the hardware driver later, so both show
//! exactly the same dots.
//!
//! A [`Scene`] is laid out text. Lines that fit are static; lines wider than
//! the display scroll as a looping ticker, one dot per tick.

use std::io::{BufRead, IsTerminal};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use anyhow::{Result, bail};
use clap::ValueEnum;
use embedded_graphics::mono_font::iso_8859_1::{
    FONT_4X6, FONT_5X7, FONT_5X8, FONT_6X10, FONT_7X14,
};
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Alignment, Baseline, Text, TextStyleBuilder};

use crate::display::{Display, Flip, Note, Stacking, Terminal};

/// Size of one real panel in dots.
pub const PANEL_WIDTH: u32 = 28;
pub const PANEL_HEIGHT: u32 = 14;
/// Number of panels the display is built from.
pub const PANELS: u32 = 2;
/// Blank space between the end and the restart of a scrolling line, in
/// character cells.
const TICKER_GAP_CHARS: u32 = 2;

#[derive(clap::Args, Debug)]
pub struct FlipdotArgs {
    /// Text to show. Each argument is one line; `\n` inside an argument also
    /// starts a new line. Without any text, lines are read from stdin and
    /// each one is rendered as its own frame.
    pub text: Vec<String>,

    #[command(flatten)]
    pub display: DisplayArgs,
}

/// The display and how text is set on it. Shared by `debug flipdot` and
/// `run`.
#[derive(clap::Args, Debug)]
pub struct DisplayArgs {
    /// Font, named by the size of one character cell in dots.
    #[arg(long, value_enum, default_value_t = Font::F5x7)]
    pub font: Font,

    /// Horizontal alignment of every line.
    #[arg(long, value_enum, default_value_t = Align::Left)]
    pub align: Align,

    /// Number of panels the display is built from.
    #[arg(long, default_value_t = PANELS, value_parser = clap::value_parser!(u32).range(1..=16))]
    pub panels: u32,

    /// How the panels are mounted.
    #[arg(long, value_enum, default_value_t = Layout::Row)]
    pub layout: Layout,

    /// Width of one panel in dots.
    #[arg(long, default_value_t = PANEL_WIDTH, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub width: u32,

    /// Height of one panel in dots.
    #[arg(long, default_value_t = PANEL_HEIGHT, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub height: u32,

    /// Which lines scroll.
    #[arg(long, value_enum, default_value_t = Scroll::Auto)]
    pub scroll: Scroll,

    /// Scroll speed, in dots per second.
    #[arg(long, value_name = "DOTS_PER_SECOND", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=60))]
    pub speed: u32,
}

impl DisplayArgs {
    /// Size of the whole display in dots.
    pub fn display_size(&self) -> (u32, u32) {
        match self.layout {
            Layout::Row => (self.width * self.panels, self.height),
            Layout::Column => (self.width, self.height * self.panels),
        }
    }

    /// Time between two frames of a scrolling line.
    pub fn frame_time(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.speed))
    }

    /// Fail if the font cannot fit on the display at all.
    pub fn check(&self) -> Result<()> {
        let cell = self.font.mono().character_size;
        let (_, height) = self.display_size();
        if cell.height > height {
            bail!(
                "font {}x{} is taller than the {height} dot high display",
                cell.width,
                cell.height,
            );
        }
        Ok(())
    }

    /// Lay out `lines` for this display.
    pub fn scene<S: AsRef<str>>(&self, lines: &[S]) -> Scene {
        let refs: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();
        let (width, height) = self.display_size();
        // Rest for one second before each pass so the start is readable.
        Scene::build(&refs, self.font, self.align, width, height, self.scroll).with_hold(self.speed)
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// Only lines wider than the display scroll.
    Auto,
    /// Every line scrolls, also when it would fit.
    Always,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Panels side by side: a wide display.
    Row,
    /// Panels stacked on top of each other: a tall display.
    Column,
}

/// Fixed-width fonts covering ISO 8859-1 (ASCII plus Western European
/// characters such as ä, é, ß and °). Other characters are drawn as `?`.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    /// 7 characters and 2 lines per panel. Tiny.
    #[value(name = "4x6")]
    F4x6,
    /// 5 characters and 2 lines per panel. The classic dot matrix font.
    #[value(name = "5x7")]
    F5x7,
    /// 5 characters and 1 line per panel.
    #[value(name = "5x8")]
    F5x8,
    /// 4 characters and 1 line per panel.
    #[value(name = "6x10")]
    F6x10,
    /// 4 characters per panel, 1 line, full panel height.
    #[value(name = "7x14")]
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
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A grid of dots, each either flipped on or off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameBuffer {
    width: u32,
    height: u32,
    dots: Vec<bool>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            dots: vec![false; (width * height) as usize],
        }
    }

    /// Number of dot rows.
    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn get(&self, x: u32, y: u32) -> bool {
        x < self.width && y < self.height && self.dots[(y * self.width + x) as usize]
    }

    /// Set one dot. Positions outside the display are ignored.
    pub fn set(&mut self, x: u32, y: u32, on: bool) {
        if x < self.width && y < self.height {
            self.dots[(y * self.width + x) as usize] = on;
        }
    }

    /// Render as text: two terminal columns per dot, so dots come out round
    /// and roughly square like on the real panel.
    pub fn to_terminal(&self, color: bool) -> String {
        self.to_terminal_marked(color, &[])
    }

    /// Like [`Self::to_terminal`], with the dots in `flipped` standing out
    /// from the ones that stayed as they were.
    pub fn to_terminal_marked(&self, color: bool, flipped: &[Flip]) -> String {
        let mut marked = vec![false; self.dots.len()];
        for flip in flipped {
            if flip.x < self.width && flip.y < self.height {
                marked[(flip.y * self.width + flip.x) as usize] = true;
            }
        }
        // Yellow for a flipped dot, dim grey for the black side. A dot that
        // just came on is white, one that just went off a red ring.
        let (on, off, came_on, went_off, reset) = if color {
            (
                "\x1b[93m●",
                "\x1b[90m·",
                "\x1b[97m●",
                "\x1b[91m○",
                "\x1b[0m",
            )
        } else {
            ("●", "·", "◉", "○", "")
        };
        let mut out = String::new();
        for y in 0..self.height {
            for x in 0..self.width {
                if x > 0 {
                    out.push(' ');
                }
                let marked = marked[(y * self.width + x) as usize];
                out.push_str(match (self.get(x, y), marked) {
                    (true, false) => on,
                    (false, false) => off,
                    (true, true) => came_on,
                    (false, true) => went_off,
                });
            }
            out.push_str(reset);
            out.push('\n');
        }
        out
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            // Anything outside the display is clipped.
            if let (Ok(x), Ok(y)) = (u32::try_from(point.x), u32::try_from(point.y)) {
                self.set(x, y, color.is_on());
            }
        }
        Ok(())
    }
}

/// One line that is wider than the display, pre-rendered at full width
/// plus a gap so it can loop seamlessly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ticker {
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

        for (i, line) in shown.iter().enumerate() {
            let y = top + i as u32 * cell.height;
            let chars = line.chars().count() as u32;
            // The last character needs no trailing spacing.
            let line_width = (chars * advance).saturating_sub(mono.character_spacing);

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
                tickers.push(Ticker { strip, y });
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

    /// Ticks until every scrolling line has gone round at least once and
    /// the longest one is back at its start. `None` if nothing scrolls.
    pub fn cycle_ticks(&self) -> Option<u64> {
        self.tickers
            .iter()
            .map(|t| u64::from(self.hold) + u64::from(t.strip.width))
            .max()
    }

    /// The display contents at `tick`. Scrolling lines move one dot per tick.
    pub fn frame(&self, tick: u64) -> FrameBuffer {
        let mut fb = self.base.clone();
        for ticker in &self.tickers {
            let strip = &ticker.strip;
            // One pass: rest at the start, then scroll the whole strip by.
            // After `strip.width` dots the text is back at its start.
            let cycle = u64::from(self.hold) + u64::from(strip.width);
            let offset = (tick % cycle).saturating_sub(u64::from(self.hold)) as u32;
            for y in 0..strip.height {
                for x in 0..fb.width {
                    fb.set(x, ticker.y + y, strip.get((x + offset) % strip.width, y));
                }
            }
        }
        fb
    }

    fn warnings(&self) -> Vec<String> {
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

/// Split arguments into display lines. `\n`, typed literally or as a real
/// newline, starts a new line.
fn split_lines(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|arg| {
            arg.replace("\\n", "\n")
                .split('\n')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn run(args: FlipdotArgs) -> Result<()> {
    args.display.check()?;
    // Animation needs a terminal to redraw in place.
    let terminal = std::io::stdout().is_terminal();
    let frame_time = args.display.frame_time();
    let (width, height) = args.display.display_size();
    let mut display = Display::new(width, height);
    display.attach(Terminal::stdout(width, height, Stacking::Blocks));

    // Texts to show, one after another. Stdin is read on its own thread so a
    // scrolling text keeps moving while waiting for the next line.
    let (tx, rx) = mpsc::channel::<Vec<String>>();
    if args.text.is_empty() {
        let stdin = std::io::stdin();
        if stdin.is_terminal() {
            eprintln!(
                "adsb-flipdot: type text and press Enter, use \\n for a second line, Ctrl-D to quit"
            );
        }
        std::thread::spawn(move || {
            for line in stdin.lock().lines().map_while(Result::ok) {
                if tx.send(split_lines(&[line])).is_err() {
                    break;
                }
            }
        });
    } else {
        tx.send(split_lines(&args.text))?;
        // Closing the channel marks the end of input.
        drop(tx);
    }

    let new_text = Note {
        caption: None,
        new_text: true,
    };
    let mut next = rx.recv().ok();
    while let Some(lines) = next.take() {
        let scene = args.display.scene(&lines);
        // Before the frame: a line below it would end up inside the redraw.
        for warning in scene.warnings() {
            eprintln!("adsb-flipdot: {warning}");
        }
        display.show(&scene.frame(0), new_text)?;

        if !scene.scrolls() {
            next = rx.recv().ok();
            continue;
        }
        if !terminal {
            eprintln!(
                "adsb-flipdot: text is wider than the display; it scrolls when run in a terminal"
            );
            next = rx.recv().ok();
            continue;
        }

        // Scroll until the next text arrives. With text given as arguments
        // there is no next text, so this runs until Ctrl-C.
        let mut tick: u64 = 0;
        let mut input_open = true;
        loop {
            if input_open {
                match rx.recv_timeout(frame_time) {
                    Ok(lines) => {
                        next = Some(lines);
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    // End of stdin: stop there. Arguments: keep scrolling.
                    Err(RecvTimeoutError::Disconnected) if args.text.is_empty() => break,
                    Err(RecvTimeoutError::Disconnected) => input_open = false,
                }
            } else {
                std::thread::sleep(frame_time);
            }
            tick += 1;
            display.show(&scene.frame(tick), Note::default())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 28x14 panel, which keeps the expected dot positions small.
    fn scene1(lines: &[&str], font: Font, align: Align) -> Scene {
        Scene::new(lines, font, align, PANEL_WIDTH, PANEL_HEIGHT)
    }

    fn render(lines: &[&str], font: Font, align: Align) -> FrameBuffer {
        scene1(lines, font, align).frame(0)
    }

    /// Dots as rows of `#` and `.`, for readable assertions.
    fn rows(fb: &FrameBuffer) -> Vec<String> {
        (0..fb.height)
            .map(|y| {
                (0..fb.width)
                    .map(|x| if fb.get(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    fn lit(fb: &FrameBuffer) -> usize {
        fb.dots.iter().filter(|d| **d).count()
    }

    /// Leftmost and rightmost lit column.
    fn extent(fb: &FrameBuffer) -> (u32, u32) {
        let cols: Vec<u32> = (0..fb.width)
            .filter(|x| (0..fb.height).any(|y| fb.get(*x, y)))
            .collect();
        (cols[0], *cols.last().unwrap())
    }

    /// `fb` shifted left by `by` dots, wrapping around at `fb.width`.
    fn rotated(fb: &FrameBuffer, by: u32) -> FrameBuffer {
        let mut out = FrameBuffer::new(fb.width, fb.height);
        for y in 0..fb.height {
            for x in 0..fb.width {
                out.set(x, y, fb.get((x + by) % fb.width, y));
            }
        }
        out
    }

    fn args(extra: &[&str]) -> DisplayArgs {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrapper {
            #[command(flatten)]
            args: DisplayArgs,
        }
        Wrapper::parse_from(["test"].iter().chain(extra)).args
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
        assert_eq!((strip.width, strip.height), (40, 7));

        for tick in [0u64, 1, 2, 13, 39] {
            let frame = scene.frame(tick);
            let expected = rotated(strip, tick as u32);
            for y in 0..7 {
                for x in 0..PANEL_WIDTH {
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
        assert_eq!(fits.tickers[0].strip.width, 10 + 28);
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

    #[test]
    fn terminal_output_has_one_row_per_dot_row() {
        let fb = render(&["A"], Font::F5x7, Align::Left);
        let text = fb.to_terminal(false);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 14);
        // 28 dots separated by single spaces.
        assert!(lines.iter().all(|l| l.chars().count() == 55), "{text}");
        assert!(text.contains('●') && text.contains('·'));
        assert!(!text.contains('\x1b'), "no colour codes when disabled");
        assert!(fb.to_terminal(true).contains("\x1b[93m●"));
    }

    #[test]
    fn arguments_and_escaped_newlines_become_lines() {
        let args = vec![
            "SAS51B".to_string(),
            "a\\nb".to_string(),
            "c\nd".to_string(),
        ];
        assert_eq!(split_lines(&args), ["SAS51B", "a", "b", "c", "d"]);
    }

    #[test]
    fn display_size_follows_panels_and_layout() {
        assert_eq!(
            args(&[]).display_size(),
            (56, 14),
            "default: 2 panels in a row"
        );
        assert_eq!(args(&["--panels", "1"]).display_size(), (28, 14));
        assert_eq!(args(&["--panels", "4"]).display_size(), (112, 14));
        assert_eq!(args(&["--layout", "column"]).display_size(), (28, 28));
        assert_eq!(
            args(&[
                "--panels", "3", "--layout", "column", "--width", "20", "--height", "10"
            ])
            .display_size(),
            (20, 30)
        );
    }
}
