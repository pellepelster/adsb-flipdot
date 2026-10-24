//! `debug pi`: switch the signals of the adapter board by hand.
//!
//! Every signal the board passes on to the panel is a row in a table. The
//! address and direction lines are toggled; the COL-SEL lines are pulsed.
//! The rules are those of [`Board`], shared with the panel output.
//!
//! With `--continuous` the dots of the panel on COL-SEL0 are flipped one
//! after the other while the table is shown, see [`Sweep`].
//!
//! In a terminal of at least 32 lines the panel header of the board is
//! drawn under the table, with what a meter on each pin should show.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Row, Table};

use crate::cli::PiArgs;
use crate::commands::shutdown::Shutdown;
use crate::commands::tui::{Column, Input, Screen, cell};
use crate::output::board::{
    Board, COL_ADDRESS, Kind, MAX_COLUMNS, MAX_ROWS, NoPins, OVERRUN_FACTOR, Pins, ROW_ADDRESS,
    SIGNALS, address, line_number, micros,
};

/// What a meter between pin 1 and a pin of the board's panel header
/// should show.
#[derive(Debug, PartialEq, Eq)]
struct HeaderPin {
    /// Short name, to fit the drawing.
    name: &'static str,
    volts: &'static str,
    /// Position in [`SIGNALS`] of the signal on the pin.
    signal: Option<usize>,
    high: bool,
}

fn header_pin<P: Pins>(board: &Board<P>, pin: u8) -> HeaderPin {
    let fixed = |name, volts| HeaderPin {
        name,
        volts,
        signal: None,
        high: false,
    };
    match pin {
        1 | 2 => fixed("GND", "0 V"),
        3 => fixed("+5V", "5 V"),
        // The coil supply from the jack, less the drop across D1.
        26 => fixed("COIL", "VIN"),
        _ => {
            let Some(index) = SIGNALS.iter().position(|s| s.panel_pin == pin) else {
                return fixed("n.c.", "open");
            };
            let signal = SIGNALS[index];
            // A pulse is far too short for a meter.
            let high = signal.kind == Kind::Level && board.levels[index];
            let name = match signal.name {
                "ROW-HIGH" => "R-HI",
                "ROW-LOW" => "R-LO",
                "COL-H/L" => "C-H/L",
                name => name.strip_prefix("COL-").unwrap_or(name),
            };
            HeaderPin {
                name,
                volts: if high { "5 V" } else { "0 V" },
                signal: Some(index),
                high,
            }
        }
    }
}

/// Lines the drawing of the panel header takes.
const HEADER_LINES: u16 = 8;
/// Width of one pin in the drawing.
const PIN_WIDTH: usize = 6;

/// The panel header as seen from the top of the board, the edge towards
/// the panel at the bottom: even pins at the back, odd pins at the front.
fn header_lines<P: Pins>(board: &Board<P>, selected: usize) -> Vec<Line<'static>> {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let dim = Style::new().add_modifier(Modifier::DIM);
    let row = |pins: &[u8], text: &dyn Fn(u8, &HeaderPin) -> String| -> Line<'static> {
        Line::from(
            pins.iter()
                .map(|&pin| {
                    let info = header_pin(board, pin);
                    let mut style = if info.high { bold } else { Style::new() };
                    if info.signal == Some(selected) {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    Span::styled(format!("{:^PIN_WIDTH$}", text(pin, &info)), style)
                })
                .collect::<Vec<_>>(),
        )
    };
    let even: Vec<u8> = (1..=13).map(|n| n * 2).collect();
    let odd: Vec<u8> = (1..=13).map(|n| n * 2 - 1).collect();
    let name = |_: u8, info: &HeaderPin| info.name.to_string();
    let volts = |_: u8, info: &HeaderPin| info.volts.to_string();
    let number = |pin: u8, _: &HeaderPin| pin.to_string();
    vec![
        Line::from("Panel header, top view of the board: what a meter against pin 1 shows")
            .style(bold),
        row(&even, &name),
        row(&even, &volts),
        row(&even, &number).style(dim),
        row(&odd, &number).style(dim),
        row(&odd, &volts),
        row(&odd, &name),
        Line::from(
            "board edge towards the panel | COIL: jack voltage less about 0.4 V | SEL: 5 V only \
             during a pulse",
        )
        .style(dim),
    ]
}

/// Time from one dot of a sweep to the next.
const SWEEP_STEP: Duration = Duration::from_millis(2);
/// The screen is drawn at most this often while a sweep runs.
const REDRAW: Duration = Duration::from_millis(100);

/// Every dot of one panel in turn, row by row: the first black, the second
/// yellow and so on. Each pass swaps the two colours, so that every dot
/// flips on every pass.
#[derive(Debug, Default)]
struct Sweep {
    /// Position of the next dot, counted from 0.
    next: u32,
    passes: u64,
    /// Row and column, counted from 0, of the latest dot and whether it
    /// went yellow.
    last: Option<(u32, u32, bool)>,
}

impl Sweep {
    const DOTS: u32 = MAX_ROWS * MAX_COLUMNS;

    /// The next dot: row and column, counted from 0, and whether it goes
    /// yellow.
    fn step(&mut self) -> (u32, u32, bool) {
        let dot = self.next;
        let yellow = (dot % 2 == 1) != (self.passes % 2 == 1);
        self.next += 1;
        if self.next == Self::DOTS {
            self.next = 0;
            self.passes += 1;
        }
        let step = (dot / MAX_COLUMNS, dot % MAX_COLUMNS, yellow);
        self.last = Some(step);
        step
    }

    /// Flip the next dot on the panel on COL-SEL0. `on` is the side that
    /// shows yellow on our panels.
    fn flip<P: Pins>(&mut self, board: &mut Board<P>) -> Result<()> {
        let (row, column, yellow) = self.step();
        board.flip(0, address(row), address(column), yellow)
    }

    fn describe(&self) -> String {
        let latest = match self.last {
            Some((row, column, yellow)) => format!(
                "dot {}/{} {}",
                row + 1,
                column + 1,
                if yellow { "yellow" } else { "black" }
            ),
            None => "starting".to_string(),
        };
        format!("continuous, pass {} | {latest}", self.passes + 1)
    }
}

fn describe_address(what: &str, address: u8, max: u32) -> String {
    match line_number(address).filter(|line| u32::from(*line) <= max) {
        Some(line) => format!("{what} address {address} = {what} {line}"),
        None => format!("{what} address {address} = no {what}"),
    }
}

/// What a key asks for.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Up,
    Down,
    Press,
    AllLow,
}

fn action(key: KeyCode) -> Option<Action> {
    match key {
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
        KeyCode::Char(' ') | KeyCode::Enter => Some(Action::Press),
        KeyCode::Char('0') => Some(Action::AllLow),
        _ => None,
    }
}

const COLUMNS: [Column; 6] = [
    ("SIGNAL", 8, false),
    ("GPIO", 4, true),
    ("PI PIN", 6, true),
    ("PANEL PIN", 9, true),
    ("LEVEL", 5, true),
    ("MEANING", 40, false),
];

fn draw<P: Pins>(frame: &mut Frame, board: &Board<P>, selected: usize, status: &str) {
    // The panel header only where the table still fits beside it.
    let header_height = if frame.area().height >= 24 + HEADER_LINES {
        HEADER_LINES
    } else {
        0
    };
    let [
        header_area,
        table_area,
        pins_area,
        lines_area,
        pulse_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(header_height),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let dim = Style::new().add_modifier(Modifier::DIM);

    frame.render_widget(
        Line::from(format!("adsb-flipdot pi | {status}")).style(bold),
        header_area,
    );

    let titles = Row::new(COLUMNS.iter().map(|c| cell(c.0.to_string(), c)))
        .style(Style::new().add_modifier(Modifier::REVERSED));
    let rows = SIGNALS.iter().enumerate().map(|(index, signal)| {
        let high = board.levels[index];
        let cells = [
            signal.name.to_string(),
            signal.gpio.to_string(),
            signal.header_pin.to_string(),
            signal.panel_pin.to_string(),
            u8::from(high).to_string(),
            signal.meaning.to_string(),
        ];
        let mut style = if high { bold } else { Style::new() };
        if index == selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        Row::new(
            cells
                .into_iter()
                .zip(&COLUMNS)
                .map(|(text, column)| cell(text, column)),
        )
        .style(style)
    });
    let table = Table::new(rows, COLUMNS.iter().map(|c| Constraint::Length(c.1)))
        .header(titles)
        .column_spacing(2);
    frame.render_widget(table, table_area);
    if header_height > 0 {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(header_lines(board, selected)),
            pins_area,
        );
    }

    frame.render_widget(
        Line::from(format!(
            "{} | {} | {}",
            describe_address("row", board.address(ROW_ADDRESS), MAX_ROWS),
            describe_address("column", board.address(COL_ADDRESS), MAX_COLUMNS),
            board.direction(),
        )),
        lines_area,
    );
    frame.render_widget(
        Line::from(format!(
            "pulse {} | {} sent | last {} | longest {} | {} over {} µs",
            micros(Some(board.pulse)),
            board.pulses,
            micros(board.last_pulse),
            micros(board.longest_pulse),
            board.overruns,
            (board.pulse * OVERRUN_FACTOR).as_micros(),
        )),
        pulse_area,
    );
    frame.render_widget(
        Line::from(
            "↑ ↓ select   Space toggle, pulse on COL-SEL   0 all low   q / Esc / Ctrl-C quit",
        )
        .style(dim),
        footer_area,
    );
}

/// Show the board until the user quits, flipping the dots of `sweep` in
/// the meantime. Every line is low afterwards.
async fn show<P: Pins>(mut board: Board<P>, status: &str, mut sweep: Option<Sweep>) -> Result<()> {
    let mut shutdown = Shutdown::listen()?;
    let mut screen = Screen::open()?;
    let mut steps = tokio::time::interval(SWEEP_STEP);
    steps.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut selected = 0usize;
    // When the screen was last drawn; `None` to draw it now.
    let mut drawn: Option<Instant> = None;
    loop {
        if drawn.is_none_or(|at| at.elapsed() >= REDRAW) {
            let header = match &sweep {
                Some(sweep) => format!("{status} | {}", sweep.describe()),
                None => status.to_string(),
            };
            screen.draw(|frame| draw(frame, &board, selected, &header))?;
            drawn = Some(Instant::now());
        }
        tokio::select! {
            // Signals sent from outside, e.g. `kill`.
            _ = shutdown.recv() => break,
            input = screen.input() => {
                match input? {
                    Input::Quit => break,
                    Input::Key(key) => match action(key) {
                        Some(Action::Up) => selected = selected.saturating_sub(1),
                        Some(Action::Down) => selected = (selected + 1).min(SIGNALS.len() - 1),
                        Some(Action::Press) => board.press(selected)?,
                        Some(Action::AllLow) => board.all_low()?,
                        None => {}
                    },
                    Input::Resize | Input::Other => {}
                }
                drawn = None;
            },
            _ = steps.tick(), if sweep.is_some() => {
                if let Some(sweep) = &mut sweep {
                    sweep.flip(&mut board)?;
                }
            },
        }
    }

    drop(screen);
    let result = board.all_low();
    eprintln!("adsb-flipdot: {}; all lines low", board.pulse_report());
    result
}

pub async fn run(args: PiArgs) -> Result<()> {
    let config = args.board.config();
    let sweep = args.continuous.then(Sweep::default);
    if args.dry_run {
        let board = Board::new(NoPins, config.pulse)?;
        return show(board, "dry run, no pin is touched", sweep).await;
    }
    let (board, priority) = config
        .open()
        .context("cannot use the adapter board (--dry-run needs no pins)")?;
    let status = format!("{} | {priority}", config.chip.display());
    show(board, &status, sweep).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tui::tests::render;
    use crate::output::board::ROW_LOW;
    use crate::output::board::tests::{board, index, named};

    #[test]
    fn header_pins_show_what_a_meter_reads() {
        let (mut board, _log) = board();
        assert_eq!(header_pin(&board, 1).volts, "0 V");
        assert_eq!(header_pin(&board, 3).volts, "5 V");
        assert_eq!(header_pin(&board, 26).name, "COIL");
        for pin in [4, 12, 24, 25] {
            assert_eq!(header_pin(&board, pin).volts, "open", "pin {pin}");
        }
        assert_eq!(header_pin(&board, 7).name, "ROW1");
        assert_eq!(header_pin(&board, 7).volts, "0 V");
        board.press(index("ROW1")).unwrap();
        assert_eq!(header_pin(&board, 7).volts, "5 V");
        assert_eq!(header_pin(&board, 10).name, "R-HI");
        assert_eq!(header_pin(&board, 18).name, "C-H/L");
        board.press(index("COL-SEL0")).unwrap();
        assert_eq!(header_pin(&board, 19).name, "SEL0");
        assert_eq!(
            header_pin(&board, 19).volts,
            "0 V",
            "back low after the pulse"
        );
    }

    #[test]
    fn a_tall_screen_draws_the_panel_header() {
        let (mut board, _log) = board();
        board.press(index("ROW0")).unwrap();
        let lines = render(100, 32, |frame| draw(frame, &board, 0, "test"));
        let at = lines
            .iter()
            .position(|line| line.starts_with("Panel header"))
            .expect("drawn");
        let cells = |line: &str| -> Vec<String> {
            line.as_bytes()
                .chunks(PIN_WIDTH)
                .map(|cell| String::from_utf8_lossy(cell).trim().to_string())
                .collect()
        };
        assert_eq!(cells(&lines[at + 3])[..3], ["2", "4", "6"]);
        assert_eq!(cells(&lines[at + 4])[..3], ["1", "3", "5"]);
        assert_eq!(
            cells(&lines[at + 5])[..3],
            ["0 V", "5 V", "5 V"],
            "ROW0 high"
        );
        assert_eq!(cells(&lines[at + 6])[..3], ["GND", "+5V", "ROW0"]);
        assert_eq!(cells(&lines[at + 1])[12], "COIL");

        let short = render(100, 24, |frame| draw(frame, &board, 0, "test"));
        assert!(!short.iter().any(|line| line.starts_with("Panel header")));
    }

    #[test]
    fn a_sweep_alternates_and_swaps_the_colours_on_every_pass() {
        let mut sweep = Sweep::default();
        assert_eq!(sweep.step(), (0, 0, false), "1/1 black");
        assert_eq!(sweep.step(), (0, 1, true), "1/2 yellow");
        for _ in 2..MAX_COLUMNS {
            sweep.step();
        }
        assert_eq!(sweep.step(), (1, 0, false), "2/1 black: 28 columns");
        for _ in MAX_COLUMNS + 1..Sweep::DOTS - 1 {
            sweep.step();
        }
        assert_eq!(sweep.step(), (15, 27, true), "16/28, the last dot");
        assert_eq!(sweep.passes, 1);
        assert_eq!(sweep.step(), (0, 0, true), "1/1 yellow on the second pass");
        assert_eq!(sweep.step(), (0, 1, false), "1/2 black");
    }

    #[test]
    fn a_sweep_flips_the_dots_of_the_first_panel() {
        let (mut board, log) = board();
        let mut sweep = Sweep::default();
        sweep.flip(&mut board).unwrap();
        // Dot 1/1 black: "Dot 1/1 off" in docs/mobitec/README.md.
        assert_eq!(
            named(&log),
            [
                ("ROW-HIGH", true),
                ("COL-H/L", true),
                ("ROW0", true),
                ("COL0", true),
                ("COL-SEL0", true),
                ("COL-SEL0", false)
            ]
        );
        assert_eq!(sweep.describe(), "continuous, pass 1 | dot 1/1 black");
        sweep.flip(&mut board).unwrap();
        assert_eq!(board.address(COL_ADDRESS), 2);
        assert_eq!(sweep.describe(), "continuous, pass 1 | dot 1/2 yellow");
    }

    #[test]
    fn addresses_are_described_as_lines() {
        assert_eq!(
            describe_address("row", 18, MAX_ROWS),
            "row address 18 = row 16"
        );
        assert_eq!(
            describe_address("row", 19, MAX_ROWS),
            "row address 19 = no row"
        );
        assert_eq!(
            describe_address("column", 31, MAX_COLUMNS),
            "column address 31 = column 28"
        );
    }

    #[test]
    fn keys_select_and_press() {
        assert_eq!(action(KeyCode::Up), Some(Action::Up));
        assert_eq!(action(KeyCode::Char('j')), Some(Action::Down));
        assert_eq!(action(KeyCode::Char(' ')), Some(Action::Press));
        assert_eq!(action(KeyCode::Enter), Some(Action::Press));
        assert_eq!(action(KeyCode::Char('0')), Some(Action::AllLow));
        assert_eq!(action(KeyCode::Char('x')), None);
    }

    #[test]
    fn screen_lists_every_signal_and_what_the_lines_mean() {
        let (mut board, _log) = board();
        board.press(index("ROW0")).unwrap();
        board.press(index("COL0")).unwrap();
        board.press(ROW_LOW).unwrap();
        board.press(index("COL-SEL0")).unwrap();

        let lines = render(100, 24, |frame| draw(frame, &board, 0, "test"));
        assert_eq!(lines[0], "adsb-flipdot pi | test");
        assert!(lines[1].starts_with("SIGNAL"), "{}", lines[1]);
        for (row, signal) in SIGNALS.iter().enumerate() {
            assert!(
                lines[row + 2].starts_with(signal.name),
                "{}",
                lines[row + 2]
            );
        }
        let row0: Vec<_> = lines[2].split_whitespace().take(5).collect();
        assert_eq!(row0, ["ROW0", "17", "11", "5", "1"]);
        let row1: Vec<_> = lines[3].split_whitespace().take(5).collect();
        assert_eq!(row1, ["ROW1", "27", "13", "7", "0"]);
        assert_eq!(lines.len(), 24, "fits a standard terminal");
        assert_eq!(
            lines[21],
            "row address 1 = row 1 | column address 1 = column 1 | column feeds, row drains"
        );
        assert!(
            lines[22].starts_with("pulse 50 µs | 1 sent | last "),
            "{}",
            lines[22]
        );
        assert!(lines[23].contains("Space toggle, pulse on COL-SEL"));
    }
}
