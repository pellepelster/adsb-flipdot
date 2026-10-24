//! The flipdot panels as an [`Output`]: every flip becomes one pulse on the
//! adapter board.
//!
//! An [`Arrangement`] says which panels make up the display. [`Geometry`]
//! finds the panel, row and column of a dot of the display. The pins and
//! their rules are those of [`Board`]. The pulses are timed on the thread
//! that shows the frame, which is busy for about a quarter of a millisecond
//! per dot.

use anyhow::{Result, bail};

use crate::domain::dots::Flip;
use crate::output::board::{
    Board, BoardConfig, GpioPins, MAX_COLUMNS, MAX_PANELS, MAX_ROWS, Pins, address,
};
use crate::output::{Note, Output};

/// How the panels are wired to the adapter board and mounted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelConfig {
    pub board: BoardConfig,
    /// COL-SEL line of each panel, in the order of the panels. Empty if the
    /// first panel is on COL-SEL0, the second on COL-SEL1 and so on.
    pub select: Vec<u8>,
    /// The first column of the panels is at the right of the display.
    pub mirror_x: bool,
    /// The first row of the panels is at the bottom of the display.
    pub mirror_y: bool,
    /// The two sides of the dots are swapped.
    pub invert: bool,
}

/// The panels a display is built from, side by side in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrangement {
    /// Number of panels.
    pub panels: u32,
    /// Size of one panel in dots.
    pub width: u32,
    pub height: u32,
}

impl Arrangement {
    /// Size of the whole display in dots.
    pub fn display_size(&self) -> (u32, u32) {
        (self.width * self.panels, self.height)
    }
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
    fn new(arrangement: Arrangement, config: &PanelConfig) -> Result<Self> {
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
        let select: Vec<usize> = if config.select.is_empty() {
            (0..panels).collect()
        } else {
            config.select.iter().map(|line| *line as usize).collect()
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
            mirror_x: config.mirror_x,
            mirror_y: config.mirror_y,
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
        Some(Place {
            select: self.select[(x / panel.width) as usize],
            row: address(y),
            column: address(x % panel.width),
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
    pub fn open(arrangement: Arrangement, config: &PanelConfig) -> Result<Self> {
        let geometry = Geometry::new(arrangement, config)?;
        let (board, priority) = config.board.open()?;
        eprintln!(
            "adsb-flipdot: panels on {}, {priority}",
            config.board.chip.display()
        );
        Self::new(board, geometry, config.invert)
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

    /// Turn every dot of the display to its dark side, whatever it shows
    /// now: each dot gets a pulse, also those that are dark already.
    pub fn clear(&mut self) -> Result<()> {
        let (width, height) = self.geometry.arrangement.display_size();
        for y in 0..height {
            for x in 0..width {
                self.flip(Flip { x, y, on: false })?;
            }
        }
        self.board.rest()
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
    use crate::domain::dots::FrameBuffer;
    use crate::output::Display;
    use crate::output::board::line_number;
    use crate::output::board::tests::{FakePins, Log, PULSE, named};

    /// Two 28x14 panels side by side.
    const TWO: Arrangement = Arrangement {
        panels: 2,
        width: 28,
        height: 14,
    };
    /// Panels as high as the board can address: no rows below the display.
    const FULL_HEIGHT: Arrangement = Arrangement { height: 16, ..TWO };

    /// Panels wired in order and mounted upright.
    fn wiring() -> PanelConfig {
        PanelConfig {
            board: BoardConfig {
                chip: "/dev/null".into(),
                pulse: PULSE,
            },
            select: Vec::new(),
            mirror_x: false,
            mirror_y: false,
            invert: false,
        }
    }

    fn geometry(arrangement: Arrangement, config: PanelConfig) -> Geometry {
        Geometry::new(arrangement, &config).unwrap()
    }

    /// A panel on fake pins, with what it writes at its start left in the log.
    fn panel(arrangement: Arrangement, config: PanelConfig) -> (Panel<FakePins>, Log) {
        let log = Log::default();
        let pins = FakePins {
            log: log.clone(),
            broken: None,
        };
        let board = Board::new(pins, PULSE).unwrap();
        log.borrow_mut().clear();
        let geometry = Geometry::new(arrangement, &config).unwrap();
        let panel = Panel::new(board, geometry, config.invert).unwrap();
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
    fn display_size_follows_the_panels() {
        assert_eq!(TWO.display_size(), (56, 14));
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
        let g = geometry(TWO, wiring());
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
    fn mirroring_turns_the_whole_display_round() {
        let g = geometry(
            TWO,
            PanelConfig {
                mirror_x: true,
                mirror_y: true,
                ..wiring()
            },
        );
        assert_eq!(
            g.place(0, 0).unwrap(),
            Place {
                select: 1,
                row: 15,
                column: 31
            }
        );
        let sideways = PanelConfig {
            mirror_x: true,
            ..wiring()
        };
        assert_eq!(geometry(TWO, sideways).place(0, 0).unwrap().row, 1);
    }

    #[test]
    fn select_lines_can_be_assigned() {
        let g = geometry(
            TWO,
            PanelConfig {
                select: vec![3, 0],
                ..wiring()
            },
        );
        assert_eq!(g.place(0, 0).unwrap().select, 3);
        assert_eq!(g.place(28, 0).unwrap().select, 0);
        let one_line = PanelConfig {
            select: vec![1],
            ..wiring()
        };
        assert!(Geometry::new(TWO, &one_line).is_err(), "two panels");
    }

    #[test]
    fn displays_the_board_cannot_address_are_refused() {
        let with = |panels, width, height| {
            let arrangement = Arrangement {
                panels,
                width,
                height,
            };
            Geometry::new(arrangement, &wiring())
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
        let (panel, log) = panel(TWO, wiring());
        assert_eq!(pulses(&log).len(), 2 * 28 * 2);
        let written = named(&log);
        assert!(written.contains(&("ROW-HIGH", true)), "turned off");
        assert!(!written.contains(&("ROW-LOW", true)));
        assert_eq!(written.last(), Some(&("ROW-HIGH", false)), "rows at rest");
        drop(panel);

        let (_panel, log) = self::panel(FULL_HEIGHT, wiring());
        assert!(
            pulses(&log).is_empty(),
            "nothing below a full height display"
        );
    }

    #[test]
    fn a_frame_pulses_once_per_dot_that_differs() {
        let (panel, log) = panel(FULL_HEIGHT, wiring());
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
    fn clear_pulses_every_dot_dark() {
        let (mut cleared, log) = panel(FULL_HEIGHT, wiring());
        cleared.clear().unwrap();
        let sent = pulses(&log);
        assert_eq!(sent.len(), 2 * 28 * 16, "every dot of both panels");
        assert_eq!(sent.iter().filter(|s| **s == "COL-SEL1").count(), 28 * 16);
        // Dark is the side a dot shows with ROW-HIGH and COL-H/L high.
        let written = named(&log);
        assert!(
            !written.contains(&("ROW-LOW", true)),
            "never the bright side"
        );
        assert_eq!(written.last(), Some(&("ROW-HIGH", false)), "rows at rest");

        let (mut inverted, log) = panel(
            FULL_HEIGHT,
            PanelConfig {
                invert: true,
                ..wiring()
            },
        );
        inverted.clear().unwrap();
        assert!(
            named(&log).contains(&("ROW-LOW", true)),
            "--invert swaps the sides"
        );
    }

    #[test]
    fn invert_swaps_the_direction() {
        let flip = Flip {
            x: 0,
            y: 0,
            on: true,
        };
        let (mut plain, log) = panel(FULL_HEIGHT, wiring());
        plain.flip(flip).unwrap();
        assert!(named(&log).contains(&("ROW-LOW", true)));

        let (mut inverted, log) = panel(
            FULL_HEIGHT,
            PanelConfig {
                invert: true,
                ..wiring()
            },
        );
        inverted.flip(flip).unwrap();
        assert!(named(&log).contains(&("ROW-HIGH", true)));
    }
}
