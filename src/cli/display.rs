//! Options for the display: its panels, how text is set on it and how the
//! panels are driven.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::ValueEnum;
use clap::builder::PossibleValue;

use crate::domain::scene::{Align, Font, Scroll, Typesetting};
use crate::output::Display;
use crate::output::board::{BoardConfig, MAX_COLUMNS, MAX_PANELS};
use crate::output::panel::{Arrangement, Layout, PanelConfig};
use crate::output::terminal::Stacking;

/// Size of one real panel in dots: as wide as the board can address, two
/// rows short of it.
const PANEL_WIDTH: u32 = MAX_COLUMNS;
const PANEL_HEIGHT: u32 = 14;
/// Number of panels the display is built from.
const PANELS: u32 = 2;

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

    #[command(flatten)]
    pub panel: PanelArgs,
}

impl DisplayArgs {
    /// The panels the display is built from.
    fn arrangement(&self) -> Arrangement {
        Arrangement {
            panels: self.panels,
            width: self.width,
            height: self.height,
            layout: self.layout,
        }
    }

    /// Size of the whole display in dots.
    pub fn display_size(&self) -> (u32, u32) {
        self.arrangement().display_size()
    }

    /// Time between two frames of a scrolling line.
    pub fn frame_time(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.speed))
    }

    /// How many ticks `seconds` are: a tick moves a scrolling line by one
    /// dot.
    pub fn ticks(&self, seconds: u64) -> u64 {
        seconds * u64::from(self.speed)
    }

    /// Fail if the font cannot fit on the display at all.
    pub fn check(&self) -> Result<()> {
        let (cell_width, cell_height) = self.font.cell_size();
        let (_, height) = self.display_size();
        if cell_height > height {
            bail!("font {cell_width}x{cell_height} is taller than the {height} dot high display");
        }
        Ok(())
    }

    /// How text is set on this display.
    pub fn typesetting(&self) -> Typesetting {
        let (width, height) = self.display_size();
        Typesetting {
            width,
            height,
            font: self.font,
            align: self.align,
            scroll: self.scroll,
            // Rest for one second before each pass so the start is readable.
            hold: self.speed,
        }
    }

    /// Whether the panels are driven, next to the preview in the terminal.
    pub fn drives_panels(&self) -> bool {
        self.panel.gpio
    }

    /// The display with its outputs: the preview in the terminal and, with
    /// `--gpio`, the panels.
    pub fn open(&self, stacking: Stacking) -> Result<Display> {
        let panels = self.panel.gpio.then(|| self.panel.config());
        Display::open(self.arrangement(), panels.as_ref(), stacking)
    }
}

/// How the panels are driven and mounted.
#[derive(clap::Args, Debug)]
pub struct PanelArgs {
    /// Drive the flipdot panels through the adapter board on the Pi's GPIO
    /// pins, next to the preview in the terminal.
    #[arg(long)]
    pub gpio: bool,

    #[command(flatten)]
    pub board: BoardArgs,

    /// COL-SEL line of each panel, in the order of the panels, e.g. `1,0`.
    /// Without it the first panel is on COL-SEL0, the second on COL-SEL1
    /// and so on.
    #[arg(long, value_name = "LINES", value_delimiter = ',',
          value_parser = clap::value_parser!(u8).range(0..MAX_PANELS as i64))]
    pub select: Vec<u8>,

    /// The first column of the panels is at the right of the display.
    #[arg(long)]
    pub mirror_x: bool,

    /// The first row of the panels is at the bottom of the display.
    #[arg(long)]
    pub mirror_y: bool,

    /// Swap the two sides of the dots: use it if text shows up dark on a
    /// bright display.
    #[arg(long)]
    pub invert: bool,
}

impl PanelArgs {
    fn config(&self) -> PanelConfig {
        PanelConfig {
            board: self.board.config(),
            select: self.select.clone(),
            mirror_x: self.mirror_x,
            mirror_y: self.mirror_y,
            invert: self.invert,
        }
    }
}

/// How to reach the adapter board. Shared by the panel output and
/// `debug pi`.
#[derive(clap::Args, Debug)]
pub struct BoardArgs {
    /// GPIO character device of the Pi's header pins.
    #[arg(long, value_name = "PATH", default_value = "/dev/gpiochip0")]
    pub chip: PathBuf,

    /// Length of the COL-SEL pulse that flips one dot, in microseconds.
    #[arg(long, value_name = "MICROSECONDS", default_value_t = 190,
          value_parser = clap::value_parser!(u32).range(1..=25_000))]
    pub pulse_us: u32,
}

impl BoardArgs {
    pub fn config(&self) -> BoardConfig {
        BoardConfig {
            chip: self.chip.clone(),
            pulse: Duration::from_micros(self.pulse_us.into()),
        }
    }
}

// The values of the options below are types of the other layers, which do
// not know about the command line. Their names and help texts are given
// here.

impl ValueEnum for Font {
    fn value_variants<'a>() -> &'a [Self] {
        &[Font::F4x6, Font::F5x7, Font::F5x8, Font::F6x10, Font::F7x14]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        let (name, help) = match self {
            Font::F4x6 => ("4x6", "7 characters and 2 lines per panel. Tiny"),
            Font::F5x7 => (
                "5x7",
                "5 characters and 2 lines per panel. The classic dot matrix font",
            ),
            Font::F5x8 => ("5x8", "5 characters and 1 line per panel"),
            Font::F6x10 => ("6x10", "4 characters and 1 line per panel"),
            Font::F7x14 => ("7x14", "4 characters per panel, 1 line, full panel height"),
        };
        Some(PossibleValue::new(name).help(help))
    }
}

impl ValueEnum for Align {
    fn value_variants<'a>() -> &'a [Self] {
        &[Align::Left, Align::Center, Align::Right]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(PossibleValue::new(match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        }))
    }
}

impl ValueEnum for Scroll {
    fn value_variants<'a>() -> &'a [Self] {
        &[Scroll::Auto, Scroll::Always]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        let (name, help) = match self {
            Scroll::Auto => ("auto", "Only lines wider than the display scroll"),
            Scroll::Always => ("always", "Every line scrolls, also when it would fit"),
        };
        Some(PossibleValue::new(name).help(help))
    }
}

impl ValueEnum for Layout {
    fn value_variants<'a>() -> &'a [Self] {
        &[Layout::Row, Layout::Column]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        let (name, help) = match self {
            Layout::Row => ("row", "Panels side by side: a wide display"),
            Layout::Column => (
                "column",
                "Panels stacked on top of each other: a tall display",
            ),
        };
        Some(PossibleValue::new(name).help(help))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tests::{parse_args, try_parse_args};

    fn args(extra: &[&str]) -> DisplayArgs {
        parse_args(extra)
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

    #[test]
    fn a_font_taller_than_the_display_is_refused() {
        assert!(args(&[]).check().is_ok());
        assert!(args(&["--font", "7x14", "--height", "14"]).check().is_ok());
        let error = args(&["--font", "7x14", "--height", "10"])
            .check()
            .unwrap_err()
            .to_string();
        assert!(error.contains("7x14 is taller than the 10 dot"), "{error}");
    }

    #[test]
    fn every_value_of_an_option_has_a_name() {
        fn names<T: ValueEnum>() -> Vec<String> {
            T::value_variants()
                .iter()
                .map(|v| v.to_possible_value().unwrap().get_name().to_string())
                .collect()
        }
        assert_eq!(names::<Font>(), ["4x6", "5x7", "5x8", "6x10", "7x14"]);
        assert_eq!(names::<Align>(), ["left", "center", "right"]);
        assert_eq!(names::<Scroll>(), ["auto", "always"]);
        assert_eq!(names::<Layout>(), ["row", "column"]);
        assert!(try_parse_args::<DisplayArgs>(&["--font", "9x9"]).is_err());
    }

    #[test]
    fn options_become_the_values_the_layers_work_with() {
        let args = args(&[
            "--panels", "3", "--font", "4x6", "--align", "center", "--scroll", "always", "--speed",
            "20",
        ]);
        assert_eq!(
            args.typesetting(),
            Typesetting {
                width: 84,
                height: 14,
                font: Font::F4x6,
                align: Align::Center,
                scroll: Scroll::Always,
                hold: 20,
            }
        );
        assert_eq!(args.frame_time(), Duration::from_millis(50));
        assert_eq!(args.ticks(3), 60);
        assert!(!args.drives_panels());
    }

    #[test]
    fn panel_options_describe_the_wiring() {
        let args = args(&[
            "--gpio",
            "--chip",
            "/dev/gpiochip4",
            "--pulse-us",
            "500",
            "--select",
            "1,0",
            "--mirror-x",
            "--invert",
        ]);
        assert!(args.drives_panels());
        assert_eq!(
            args.panel.config(),
            PanelConfig {
                board: BoardConfig {
                    chip: "/dev/gpiochip4".into(),
                    pulse: Duration::from_micros(500),
                },
                select: vec![1, 0],
                mirror_x: true,
                mirror_y: false,
                invert: true,
            }
        );
        assert!(try_parse_args::<DisplayArgs>(&["--select", "5"]).is_err());
    }
}
