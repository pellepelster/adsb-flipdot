//! Options for the display: its panels, how text is set on it and how the
//! panels are driven.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::ValueEnum;
use clap::builder::PossibleValue;

use crate::domain::pattern::Pattern;
use crate::domain::scene::{Align, Font, Scroll, Typesetting};
use crate::output::Display;
use crate::output::board::GpioPins;
use crate::output::board::{BoardConfig, MAX_COLUMNS, MAX_PANELS, MAX_ROWS};
use crate::output::panel::{Arrangement, Panel, PanelConfig};
use crate::output::terminal::Stacking;

/// Size of one real panel in dots: 28 columns and 16 rows, as many as the
/// board can address.
const PANEL_WIDTH: u32 = MAX_COLUMNS;
const PANEL_HEIGHT: u32 = MAX_ROWS;
/// Number of panels the display is built from.
const PANELS: u32 = 1;

/// The display and how text is set on it. Shared by `debug flipdot` and
/// `live`.
#[derive(clap::Args, Debug)]
pub struct DisplayArgs {
    /// Horizontal alignment of every line.
    #[arg(long, value_enum, default_value_t = Align::Left)]
    pub align: Align,

    #[command(flatten)]
    pub panels: PanelsArgs,

    /// Which lines scroll.
    #[arg(long, value_enum, default_value_t = Scroll::Auto)]
    pub scroll: Scroll,

    /// Scroll speed, in dots per second.
    #[arg(long, value_name = "DOTS_PER_SECOND", default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=60))]
    pub scroll_speed: u32,
}

/// The panels a display is built from and how they are wired. Shared by
/// every command that drives panels.
#[derive(clap::Args, Debug)]
pub struct PanelsArgs {
    /// Number of panels the display is built from, side by side.
    #[arg(long = "panels", default_value_t = PANELS, value_parser = clap::value_parser!(u32).range(1..=16))]
    pub count: u32,

    /// Width of one panel in dots.
    #[arg(long, default_value_t = PANEL_WIDTH, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub width: u32,

    /// Height of one panel in dots.
    #[arg(long, default_value_t = PANEL_HEIGHT, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub height: u32,

    #[command(flatten)]
    pub panel: PanelArgs,
}

impl PanelsArgs {
    fn arrangement(&self) -> Arrangement {
        Arrangement {
            panels: self.count,
            width: self.width,
            height: self.height,
        }
    }

    /// Size of the whole display in dots.
    pub fn display_size(&self) -> (u32, u32) {
        self.arrangement().display_size()
    }

    /// The panels alone, without the preview.
    pub fn open(&self) -> Result<Panel<GpioPins>> {
        Panel::open(self.arrangement(), &self.panel.config())
    }

    /// The preview in the terminal and, if `panels`, the panels.
    pub fn open_display(&self, stacking: Stacking, panels: bool) -> Result<Display> {
        let config = panels.then(|| self.panel.config());
        Display::open(self.arrangement(), config.as_ref(), stacking)
    }
}

impl DisplayArgs {
    /// Size of the whole display in dots.
    pub fn display_size(&self) -> (u32, u32) {
        self.panels.display_size()
    }

    /// Time between two frames of a scrolling line.
    pub fn frame_time(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.scroll_speed))
    }

    /// How many ticks `seconds` are: a tick moves a scrolling line by one
    /// dot.
    pub fn ticks(&self, seconds: u64) -> u64 {
        seconds * u64::from(self.scroll_speed)
    }

    /// Fail if `font` cannot fit on the display at all.
    pub fn check(&self, font: Font) -> Result<()> {
        let (cell_width, cell_height) = font.cell_size();
        let (_, height) = self.display_size();
        if cell_height > height {
            bail!("font {cell_width}x{cell_height} is taller than the {height} dot high display");
        }
        Ok(())
    }

    /// How text is set in `font` on this display.
    pub fn typesetting(&self, font: Font) -> Typesetting {
        let (width, height) = self.display_size();
        Typesetting {
            width,
            height,
            font,
            align: self.align,
            scroll: self.scroll,
            // Rest for one second before each pass so the start is readable.
            hold: self.scroll_speed,
        }
    }

    /// The display with its outputs: the preview in the terminal and, if
    /// `panels`, the panels. Whether the panels are driven is the command's
    /// option: `live --gpio`, `debug flipdot` unless `--dry-run`.
    pub fn open(&self, stacking: Stacking, panels: bool) -> Result<Display> {
        self.panels.open_display(stacking, panels)
    }
}

/// How the panels are driven and mounted.
#[derive(clap::Args, Debug)]
pub struct PanelArgs {
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

    /// The display is mounted upside down: turn the picture by 180°. The
    /// same as `--mirror-x --mirror-y`, and turns those back if given too.
    #[arg(long)]
    pub upside_down: bool,

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
            mirror_x: self.mirror_x != self.upside_down,
            mirror_y: self.mirror_y != self.upside_down,
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
            Font::F5x8 => (
                "5x8",
                "5 characters and 2 lines per panel, filling its full height",
            ),
            Font::F6x10 => ("6x10", "4 characters and 1 line per panel"),
            Font::F7x14 => (
                "7x14",
                "4 characters and 1 line per panel, the largest letters",
            ),
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

impl ValueEnum for Pattern {
    fn value_variants<'a>() -> &'a [Self] {
        &Pattern::ALL
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        let (name, help) = match self {
            Pattern::On => ("on", "Every dot on"),
            Pattern::Off => ("off", "Every dot off"),
            Pattern::Checkerboard => ("checkerboard", "Every dot against its neighbours"),
            Pattern::CheckerboardInverted => (
                "checkerboard-inverted",
                "The checkerboard the other way round",
            ),
            Pattern::Rows => ("rows", "Every other row"),
            Pattern::Columns => ("columns", "Every other column"),
            Pattern::Border => ("border", "The outermost dots"),
            Pattern::TopLeft => (
                "top-left",
                "A block in the top left corner and a line along the top, to check the mounting",
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
    fn display_size_follows_the_panels() {
        assert_eq!(args(&[]).display_size(), (28, 16), "default: one panel");
        assert_eq!(args(&["--panels", "2"]).display_size(), (56, 16));
        assert_eq!(args(&["--panels", "4"]).display_size(), (112, 16));
        assert_eq!(
            args(&["--panels", "3", "--width", "20", "--height", "10"]).display_size(),
            (60, 10)
        );
        assert!(try_parse_args::<DisplayArgs>(&["--layout", "row"]).is_err());
    }

    #[test]
    fn a_font_taller_than_the_display_is_refused() {
        assert!(args(&[]).check(Font::F5x7).is_ok());
        assert!(args(&["--height", "14"]).check(Font::F7x14).is_ok());
        let error = args(&["--height", "10"])
            .check(Font::F7x14)
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
        assert!(
            try_parse_args::<DisplayArgs>(&["--font", "4x6"]).is_err(),
            "only `debug flipdot` has a font option"
        );
    }

    #[test]
    fn options_become_the_values_the_layers_work_with() {
        let args = args(&[
            "--panels",
            "3",
            "--align",
            "center",
            "--scroll",
            "always",
            "--scroll-speed",
            "20",
        ]);
        assert_eq!(
            args.typesetting(Font::F4x6),
            Typesetting {
                width: 84,
                height: 16,
                font: Font::F4x6,
                align: Align::Center,
                scroll: Scroll::Always,
                hold: 20,
            }
        );
        assert_eq!(args.frame_time(), Duration::from_millis(50));
        assert_eq!(args.ticks(3), 60);

        let default = super::tests::args(&[]);
        assert_eq!(
            default.scroll_speed, 7,
            "7 dots per second unless told otherwise"
        );
        assert_eq!(
            default.typesetting(Font::F5x7).hold,
            7,
            "one second of rest"
        );
    }

    #[test]
    fn panel_options_describe_the_wiring() {
        let args = args(&[
            "--chip",
            "/dev/gpiochip4",
            "--pulse-us",
            "500",
            "--select",
            "1,0",
            "--mirror-x",
            "--invert",
        ]);
        assert_eq!(
            args.panels.panel.config(),
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

    #[test]
    fn upside_down_mirrors_both_ways() {
        let mirrors = |extra: &[&str]| {
            let config = args(extra).panels.panel.config();
            (config.mirror_x, config.mirror_y)
        };
        assert_eq!(mirrors(&[]), (false, false));
        assert_eq!(mirrors(&["--upside-down"]), (true, true));
        // Upside down and mirrored left to right: only the rows turn.
        assert_eq!(mirrors(&["--upside-down", "--mirror-x"]), (false, true));
    }
}
