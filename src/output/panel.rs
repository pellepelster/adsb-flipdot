//! The flipdot panels as an [`Output`]: every flip becomes one pulse on the
//! adapter board.
//!
//! An [`Arrangement`] says which panels make up the display. [`Geometry`]
//! finds the panel, row and column of a dot of the display. The pins and
//! their rules are those of [`Board`]. The pulses are timed on the thread
//! that shows the frame, which is busy for about a quarter of a millisecond
//! per dot.

use anyhow::{Result, bail};
use clap::ValueEnum;

use crate::output::board::{Board, BoardArgs, GpioPins, MAX_COLUMNS, MAX_PANELS, MAX_ROWS, Pins};
use crate::output::display::{Flip, Note, Output};

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

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Panels side by side: a wide display.
    Row,
    /// Panels stacked on top of each other: a tall display.
    Column,
}

/// The panels a display is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrangement {
    /// Number of panels.
    pub panels: u32,
    /// Size of one panel in dots.
    pub width: u32,
    pub height: u32,
    pub layout: Layout,
}

impl Arrangement {
    /// Size of the whole display in dots.
    pub fn display_size(&self) -> (u32, u32) {
        match self.layout {
            Layout::Row => (self.width * self.panels, self.height),
            Layout::Column => (self.width, self.height * self.panels),
        }
    }
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
    arrangement: Arrangement,
    /// COL-SEL line of each panel.
    select: Vec<usize>,
    mirror_x: bool,
    mirror_y: bool,
}

impl Geometry {
    /// Fail if the board or the panels cannot address this display.
    fn new(arrangement: Arrangement, args: &PanelArgs) -> Result<Self> {
        let Arrangement { width, height, .. } = arrangement;
        let panels = arrangement.panels as usize;
        if width > MAX_COLUMNS || height > MAX_ROWS {
            bail!(
                "a panel is addressed with at most {MAX_COLUMNS} columns and {MAX_ROWS} rows, \
                 not {width}x{height}"
            );
        }
        if panels > MAX_PANELS {
            bail!("the adapter board selects at most {MAX_PANELS} panels, not {panels}");
        }
        let select: Vec<usize> = if args.select.is_empty() {
            (0..panels).collect()
        } else {
            args.select.iter().map(|line| *line as usize).collect()
        };
        if select.len() != panels {
            bail!(
                "--select names {} COL-SEL lines for {panels} panels",
                select.len()
            );
        }
        Ok(Self {
            arrangement,
            select,
            mirror_x: args.mirror_x,
            mirror_y: args.mirror_y,
        })
    }

    /// The panel, row and column of dot `x`, `y` of the display. `None`
    /// outside the display.
    fn place(&self, x: u32, y: u32) -> Option<Place> {
        let panel = self.arrangement;
        let (width, height) = panel.display_size();
        if x >= width || y >= height {
            return None;
        }
        let x = if self.mirror_x { width - 1 - x } else { x };
        let y = if self.mirror_y { height - 1 - y } else { y };
        let (index, column, row) = match panel.layout {
            Layout::Row => (x / panel.width, x % panel.width, y),
            Layout::Column => (y / panel.height, x, y % panel.height),
        };
        Some(Place {
            select: self.select[index as usize],
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
    pub fn open(arrangement: Arrangement, args: &PanelArgs) -> Result<Self> {
        let geometry = Geometry::new(arrangement, args)?;
        let (board, priority) = args.board.open()?;
        eprintln!(
            "adsb-flipdot: panels on {}, {priority}",
            args.board.chip.display()
        );
        Self::new(board, geometry, args.invert)
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
        let Arrangement { width, height, .. } = self.geometry.arrangement;
        for select in self.geometry.select.clone() {
            for column in 0..width {
                for row in height..MAX_ROWS {
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
    use crate::domain::scene::FrameBuffer;
    use crate::output::board::line_number;
    use crate::output::board::tests::{FakePins, Log, PULSE, named};
    use crate::output::display::Display;
    use crate::tests::parse_args;

    /// Two 28x14 panels side by side.
    const TWO: Arrangement = Arrangement {
        panels: 2,
        width: 28,
        height: 14,
        layout: Layout::Row,
    };
    /// Panels as high as the board can address: no rows below the display.
    const FULL_HEIGHT: Arrangement = Arrangement { height: 16, ..TWO };

    fn try_geometry(arrangement: Arrangement, extra: &[&str]) -> Result<Geometry> {
        Geometry::new(arrangement, &parse_args(extra))
    }

    fn geometry(arrangement: Arrangement, extra: &[&str]) -> Geometry {
        try_geometry(arrangement, extra).unwrap()
    }

    /// A panel on fake pins, with what it writes at its start left in the log.
    fn panel(arrangement: Arrangement, extra: &[&str]) -> (Panel<FakePins>, Log) {
        let args: PanelArgs = parse_args(extra);
        let log = Log::default();
        let pins = FakePins {
            log: log.clone(),
            broken: None,
        };
        let board = Board::new(pins, PULSE).unwrap();
        log.borrow_mut().clear();
        let geometry = Geometry::new(arrangement, &args).unwrap();
        let panel = Panel::new(board, geometry, args.invert).unwrap();
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
    fn display_size_follows_panels_and_layout() {
        assert_eq!(TWO.display_size(), (56, 14));
        let column = Arrangement {
            layout: Layout::Column,
            ..TWO
        };
        assert_eq!(column.display_size(), (28, 28));
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
        let g = geometry(TWO, &[]);
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
        let column = Arrangement {
            layout: Layout::Column,
            ..TWO
        };
        let g = geometry(column, &[]);
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
        let g = geometry(TWO, &["--mirror-x", "--mirror-y"]);
        assert_eq!(
            g.place(0, 0).unwrap(),
            Place {
                select: 1,
                row: 15,
                column: 31
            }
        );
        assert_eq!(geometry(TWO, &["--mirror-x"]).place(0, 0).unwrap().row, 1);
    }

    #[test]
    fn select_lines_can_be_assigned() {
        let g = geometry(TWO, &["--select", "3,0"]);
        assert_eq!(g.place(0, 0).unwrap().select, 3);
        assert_eq!(g.place(28, 0).unwrap().select, 0);
        assert!(try_geometry(TWO, &["--select", "1"]).is_err(), "two panels");
    }

    #[test]
    fn displays_the_board_cannot_address_are_refused() {
        let with = |panels, width, height| {
            let arrangement = Arrangement {
                panels,
                width,
                height,
                ..TWO
            };
            try_geometry(arrangement, &[])
        };
        assert!(with(5, 28, 14).is_ok());
        assert!(with(6, 28, 14).is_err());
        assert!(with(2, 29, 14).is_err());
        assert!(with(2, 28, 16).is_ok());
        assert!(with(2, 28, 17).is_err());
    }

    #[test]
    fn rows_below_the_display_are_turned_off_at_the_start() {
        // Rows 15 and 16 of two 28 column panels.
        let (panel, log) = panel(TWO, &[]);
        assert_eq!(pulses(&log).len(), 2 * 28 * 2);
        let written = named(&log);
        assert!(written.contains(&("ROW-HIGH", true)), "turned off");
        assert!(!written.contains(&("ROW-LOW", true)));
        assert_eq!(written.last(), Some(&("ROW-HIGH", false)), "rows at rest");
        drop(panel);

        let (_panel, log) = self::panel(FULL_HEIGHT, &[]);
        assert!(
            pulses(&log).is_empty(),
            "nothing below a full height display"
        );
    }

    #[test]
    fn a_frame_pulses_once_per_dot_that_differs() {
        let (panel, log) = panel(FULL_HEIGHT, &[]);
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
        let (mut plain, log) = panel(FULL_HEIGHT, &[]);
        plain.flip(flip).unwrap();
        assert!(named(&log).contains(&("ROW-LOW", true)));

        let (mut inverted, log) = panel(FULL_HEIGHT, &["--invert"]);
        inverted.flip(flip).unwrap();
        assert!(named(&log).contains(&("ROW-HIGH", true)));
    }
}
