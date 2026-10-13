//! The flipdot panels as an [`Output`]: every flip becomes one pulse on the
//! adapter board.
//!
//! [`Geometry`] finds the panel, row and column of a dot of the display.
//! The pins and their rules are those of [`Board`].
//! The pulses are timed on the thread that shows the frame, which is busy
//! for about a quarter of a millisecond per dot.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::board::{Board, GpioPins, MAX_COLUMNS, MAX_PANELS, MAX_ROWS, Pins, enable_realtime};
use crate::display::{Flip, Note, Output};
use crate::flipdot::{DisplayArgs, Layout};

#[derive(clap::Args, Debug)]
pub struct PanelArgs {
    /// Drive the flipdot panels through the adapter board on the Pi's GPIO
    /// pins, next to the preview in the terminal.
    #[arg(long)]
    pub gpio: bool,

    /// GPIO character device of the Pi's header pins.
    #[arg(long, value_name = "PATH", default_value = "/dev/gpiochip0")]
    pub chip: PathBuf,

    /// Length of the pulse that flips one dot, in microseconds.
    #[arg(long, value_name = "MICROSECONDS", default_value_t = 190,
          value_parser = clap::value_parser!(u32).range(1..=25_000))]
    pub pulse_us: u32,

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

/// The address of a row or column, counted from 0. Each decoder chip
/// serves seven lines and leaves every eighth code unused.
fn address(line: u32) -> u8 {
    (line / 7 * 8 + line % 7 + 1) as u8
}

/// Where one dot of the display is on the panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    /// COL-SEL line of the panel, counted from 0.
    select: usize,
    /// Row address.
    row: u8,
    /// Column address.
    column: u8,
}

/// How the panels make up the display.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Geometry {
    /// Size of one panel in dots.
    width: u32,
    height: u32,
    layout: Layout,
    /// COL-SEL line of each panel.
    select: Vec<usize>,
    mirror_x: bool,
    mirror_y: bool,
}

impl Geometry {
    /// Fail if the board or the panels cannot address this display.
    fn new(args: &DisplayArgs) -> Result<Self> {
        let panels = args.panels as usize;
        if args.width > u32::from(MAX_COLUMNS) || args.height > u32::from(MAX_ROWS) {
            bail!(
                "a panel is addressed with at most {MAX_COLUMNS} columns and {MAX_ROWS} rows, \
                 not {}x{}",
                args.width,
                args.height
            );
        }
        if panels > MAX_PANELS {
            bail!("the adapter board selects at most {MAX_PANELS} panels, not {panels}");
        }
        let select: Vec<usize> = if args.panel.select.is_empty() {
            (0..panels).collect()
        } else {
            args.panel
                .select
                .iter()
                .map(|line| *line as usize)
                .collect()
        };
        if select.len() != panels {
            bail!(
                "--select names {} COL-SEL lines for {panels} panels",
                select.len()
            );
        }
        Ok(Self {
            width: args.width,
            height: args.height,
            layout: args.layout,
            select,
            mirror_x: args.panel.mirror_x,
            mirror_y: args.panel.mirror_y,
        })
    }

    /// Size of the whole display in dots.
    fn display_size(&self) -> (u32, u32) {
        let panels = self.select.len() as u32;
        match self.layout {
            Layout::Row => (self.width * panels, self.height),
            Layout::Column => (self.width, self.height * panels),
        }
    }

    /// The panel, row and column of dot `x`, `y` of the display. `None`
    /// outside the display.
    fn place(&self, x: u32, y: u32) -> Option<Place> {
        let (width, height) = self.display_size();
        if x >= width || y >= height {
            return None;
        }
        let x = if self.mirror_x { width - 1 - x } else { x };
        let y = if self.mirror_y { height - 1 - y } else { y };
        let (panel, column, row) = match self.layout {
            Layout::Row => (x / self.width, x % self.width, y),
            Layout::Column => (y / self.height, x, y % self.height),
        };
        Some(Place {
            select: self.select[panel as usize],
            row: address(row),
            column: address(column),
        })
    }
}

/// The panels behind the adapter board.
///
/// Every line is low again when the panel is dropped.
pub struct Panel<P: Pins> {
    board: Board<P>,
    geometry: Geometry,
    invert: bool,
}

impl Panel<GpioPins> {
    /// Take the pins of the adapter board. The calling thread gets
    /// real-time priority where the system allows it, because it is the
    /// one that times the pulses.
    pub fn open(args: &DisplayArgs) -> Result<Self> {
        let geometry = Geometry::new(args)?;
        let pins = GpioPins::open(&args.panel.chip)?;
        let priority = if enable_realtime() {
            "real-time priority"
        } else {
            "normal priority, pulses may run long (run as root for real-time priority)"
        };
        eprintln!(
            "adsb-flipdot: panels on {}, {priority}",
            args.panel.chip.display()
        );
        let pulse = Duration::from_micros(args.panel.pulse_us.into());
        Self::new(Board::new(pins, pulse)?, geometry, args.panel.invert)
    }
}

impl<P: Pins> Panel<P> {
    fn new(board: Board<P>, geometry: Geometry, invert: bool) -> Result<Self> {
        let mut panel = Self {
            board,
            geometry,
            invert,
        };
        panel.clear_unused_rows()?;
        Ok(panel)
    }

    /// Turn off the rows of the panels below the display. A display lower
    /// than its panels never draws there, and the dots would keep whatever
    /// they showed when the power went.
    fn clear_unused_rows(&mut self) -> Result<()> {
        let Geometry { width, height, .. } = self.geometry;
        for select in self.geometry.select.clone() {
            for column in 0..width {
                for row in height..u32::from(MAX_ROWS) {
                    self.board
                        .flip(select, address(row), address(column), self.invert)?;
                }
            }
        }
        self.board.rest()
    }
}

impl<P: Pins> Output for Panel<P> {
    fn flip(&mut self, flip: Flip) -> Result<()> {
        let Some(place) = self.geometry.place(flip.x, flip.y) else {
            return Ok(());
        };
        self.board.flip(
            place.select,
            place.row,
            place.column,
            flip.on != self.invert,
        )
    }

    fn flush(&mut self, _note: &Note) -> Result<()> {
        self.board.rest()
    }
}

impl<P: Pins> Drop for Panel<P> {
    fn drop(&mut self) {
        eprintln!("adsb-flipdot: {}", self.board.pulse_report());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::line_number;
    use crate::board::tests::{FakePins, Log, PULSE, named};
    use crate::display::Display;
    use crate::scene::FrameBuffer;
    use crate::tests::parse_args;

    fn args(extra: &[&str]) -> DisplayArgs {
        parse_args(extra)
    }

    fn geometry(extra: &[&str]) -> Geometry {
        Geometry::new(&args(extra)).unwrap()
    }

    /// A panel on fake pins, with what it writes at its start left in the log.
    fn panel(extra: &[&str]) -> (Panel<FakePins>, Log) {
        let args = args(extra);
        let log = Log::default();
        let pins = FakePins {
            log: log.clone(),
            broken: None,
        };
        let board = Board::new(pins, PULSE).unwrap();
        log.borrow_mut().clear();
        let panel = Panel::new(board, Geometry::new(&args).unwrap(), args.panel.invert).unwrap();
        (panel, log)
    }

    /// The pulses in the log, as the name of the COL-SEL line.
    fn pulses(log: &Log) -> Vec<&'static str> {
        named(log)
            .into_iter()
            .filter(|(name, high)| name.starts_with("COL-SEL") && *high)
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn addresses_skip_every_eighth_code() {
        assert_eq!(address(0), 1);
        assert_eq!(address(6), 7);
        assert_eq!(address(7), 9);
        assert_eq!(address(27), 31);
        assert_eq!(address(15), 18, "row 16");
        // The same numbering `debug pi` shows, counted from 1 there.
        for line in 0..28 {
            assert_eq!(line_number(address(line)), Some(line as u8 + 1));
        }
    }

    #[test]
    fn panels_in_a_row_share_the_rows() {
        let g = geometry(&[]);
        let place = |x, y| g.place(x, y).unwrap();
        assert_eq!(
            place(0, 0),
            Place {
                select: 0,
                row: 1,
                column: 1
            }
        );
        assert_eq!(
            place(27, 13),
            Place {
                select: 0,
                row: 15,
                column: 31
            }
        );
        // Column 29 is column 1 of the next panel.
        assert_eq!(
            place(28, 0),
            Place {
                select: 1,
                row: 1,
                column: 1
            }
        );
        assert_eq!(g.place(56, 0), None);
        assert_eq!(g.place(0, 14), None);
    }

    #[test]
    fn panels_in_a_column_share_the_columns() {
        let g = geometry(&["--layout", "column"]);
        let place = |x, y| g.place(x, y).unwrap();
        assert_eq!(place(3, 13).select, 0);
        assert_eq!(
            place(3, 14),
            Place {
                select: 1,
                row: 1,
                column: 4
            }
        );
        assert_eq!(g.place(28, 0), None);
    }

    #[test]
    fn mirroring_turns_the_whole_display_round() {
        let g = geometry(&["--mirror-x", "--mirror-y"]);
        assert_eq!(
            g.place(0, 0).unwrap(),
            Place {
                select: 1,
                row: 15,
                column: 31
            }
        );
        assert_eq!(geometry(&["--mirror-x"]).place(0, 0).unwrap().row, 1);
    }

    #[test]
    fn select_lines_can_be_assigned() {
        let g = geometry(&["--select", "3,0"]);
        assert_eq!(g.place(0, 0).unwrap().select, 3);
        assert_eq!(g.place(28, 0).unwrap().select, 0);
        assert!(
            Geometry::new(&args(&["--select", "1"])).is_err(),
            "two panels"
        );
    }

    #[test]
    fn displays_the_board_cannot_address_are_refused() {
        assert!(Geometry::new(&args(&["--panels", "5"])).is_ok());
        assert!(Geometry::new(&args(&["--panels", "6"])).is_err());
        assert!(Geometry::new(&args(&["--width", "29"])).is_err());
        assert!(Geometry::new(&args(&["--height", "16"])).is_ok());
        assert!(Geometry::new(&args(&["--height", "17"])).is_err());
    }

    #[test]
    fn rows_below_the_display_are_turned_off_at_the_start() {
        // Rows 15 and 16 of two 28 column panels.
        let (panel, log) = panel(&[]);
        assert_eq!(pulses(&log).len(), 2 * 28 * 2);
        let written = named(&log);
        assert!(written.contains(&("ROW-HIGH", true)), "turned off");
        assert!(!written.contains(&("ROW-LOW", true)));
        assert_eq!(written.last(), Some(&("ROW-HIGH", false)), "rows at rest");
        drop(panel);

        let (_panel, log) = self::panel(&["--height", "16"]);
        assert!(
            pulses(&log).is_empty(),
            "nothing below a full height display"
        );
    }

    #[test]
    fn a_frame_pulses_once_per_dot_that_differs() {
        let (panel, log) = panel(&["--height", "16"]);
        let mut display = Display::new(56, 16);
        display.attach(panel);

        let mut frame = FrameBuffer::new(56, 16);
        frame.set(30, 2, true);
        display.show(&frame, Note::default()).unwrap();
        assert_eq!(pulses(&log).len(), 56 * 16, "first frame: every dot");
        assert_eq!(named(&log).last(), Some(&("ROW-LOW", false)), "at rest");

        log.borrow_mut().clear();
        display.show(&frame, Note::default()).unwrap();
        assert!(log.borrow().is_empty(), "same frame: no line moves");

        frame.set(30, 2, false);
        frame.set(1, 0, true);
        display.show(&frame, Note::default()).unwrap();
        assert_eq!(pulses(&log), ["COL-SEL1", "COL-SEL0"], "off, then on");
    }

    #[test]
    fn invert_swaps_the_direction() {
        let flip = Flip {
            x: 0,
            y: 0,
            on: true,
        };
        let (mut plain, log) = panel(&["--height", "16"]);
        plain.flip(flip).unwrap();
        assert!(named(&log).contains(&("ROW-LOW", true)));

        let (mut inverted, log) = panel(&["--height", "16", "--invert"]);
        inverted.flip(flip).unwrap();
        assert!(named(&log).contains(&("ROW-HIGH", true)));
    }
}
