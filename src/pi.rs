//! `debug pi`: switch the signals of the adapter board by hand.
//!
//! Every signal the board passes on to the panel is a row in a table. The
//! address and direction lines are toggled; the COL-SEL lines are pulsed.
//! The rules are those of [`Board`], shared with the panel output.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Cell, Row, Table};
use tokio::signal::unix::{SignalKind, signal};

use crate::board::{
    Board, COL_ADDRESS, GpioPins, MAX_ROWS, NoPins, OVERRUN_FACTOR, Pins, ROW_ADDRESS, SIGNALS,
    enable_realtime, line_number, micros,
};
use crate::tui::TerminalGuard;

#[derive(clap::Args, Debug)]
pub struct PiArgs {
    /// GPIO character device of the Pi's header pins.
    #[arg(long, value_name = "PATH", default_value = "/dev/gpiochip0")]
    pub chip: PathBuf,

    /// Length of a COL-SEL pulse, in microseconds.
    #[arg(long, value_name = "MICROSECONDS", default_value_t = 190,
          value_parser = clap::value_parser!(u32).range(1..=25_000))]
    pub pulse_us: u32,

    /// Touch no pin, to try the interface away from the Pi.
    #[arg(long)]
    pub dry_run: bool,
}

fn describe_address(what: &str, address: u8, max: u8) -> String {
    match line_number(address).filter(|line| *line <= max) {
        Some(line) => format!("{what} address {address} = {what} {line}"),
        None => format!("{what} address {address} = no {what}"),
    }
}

/// What a key press asks the event loop to do.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Quit,
    Up,
    Down,
    Press,
    AllLow,
    Redraw,
    None,
}

fn handle_event(event: &Event) -> Action {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            // Raw mode turns Ctrl-C into a plain key press instead of SIGINT.
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            KeyCode::Up | KeyCode::Char('k') => Action::Up,
            KeyCode::Down | KeyCode::Char('j') => Action::Down,
            KeyCode::Char(' ') | KeyCode::Enter => Action::Press,
            KeyCode::Char('0') => Action::AllLow,
            _ => Action::None,
        },
        Event::Resize(..) => Action::Redraw,
        _ => Action::None,
    }
}

/// Title, width in characters, right aligned.
const COLUMNS: [(&str, u16, bool); 6] = [
    ("SIGNAL", 8, false),
    ("GPIO", 4, true),
    ("PI PIN", 6, true),
    ("PANEL PIN", 9, true),
    ("LEVEL", 5, true),
    ("MEANING", 40, false),
];

fn draw<P: Pins>(frame: &mut Frame, board: &Board<P>, selected: usize, status: &str) {
    let [header_area, table_area, lines_area, pulse_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
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

    let cell = |text: String, right: bool| {
        let line = Line::from(text);
        Cell::from(if right { line.right_aligned() } else { line })
    };
    let titles = Row::new(COLUMNS.iter().map(|c| cell(c.0.to_string(), c.2)))
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
                .zip(COLUMNS)
                .map(|(text, column)| cell(text, column.2)),
        )
        .style(style)
    });
    let table = Table::new(rows, COLUMNS.iter().map(|c| Constraint::Length(c.1)))
        .header(titles)
        .column_spacing(2);
    frame.render_widget(table, table_area);

    frame.render_widget(
        Line::from(format!(
            "{} | {} | {}",
            describe_address("row", board.address(ROW_ADDRESS), MAX_ROWS),
            describe_address("column", board.address(COL_ADDRESS), u8::MAX),
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

/// Show the board until the user quits. Every line is low afterwards.
async fn show<P: Pins>(mut board: Board<P>, status: &str) -> Result<()> {
    // `try_init` enters raw mode and the alternate screen, and installs a
    // panic hook that restores the terminal.
    let mut terminal =
        ratatui::try_init().context("cannot set up the terminal (is stdout a TTY?)")?;
    let guard = TerminalGuard;

    let mut events = EventStream::new();
    let sigint = tokio::signal::ctrl_c();
    tokio::pin!(sigint);
    let mut sigterm = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;

    let mut selected = 0usize;
    loop {
        terminal
            .draw(|frame| draw(frame, &board, selected, status))
            .context("cannot draw to the terminal")?;
        tokio::select! {
            // Signals sent from outside, e.g. `kill`.
            _ = &mut sigint => break,
            _ = sigterm.recv() => break,
            event = events.next() => match event {
                Some(Ok(event)) => match handle_event(&event) {
                    Action::Quit => break,
                    Action::Up => selected = selected.saturating_sub(1),
                    Action::Down => selected = (selected + 1).min(SIGNALS.len() - 1),
                    Action::Press => board.press(selected)?,
                    Action::AllLow => board.all_low()?,
                    Action::Redraw | Action::None => {}
                },
                Some(Err(e)) => return Err(e).context("terminal input failed"),
                None => break,
            },
        }
    }

    drop(guard);
    let result = board.all_low();
    eprintln!("adsb-flipdot: {}; all lines low", board.pulse_report());
    result
}

pub async fn run(args: PiArgs) -> Result<()> {
    let pulse = Duration::from_micros(args.pulse_us.into());
    if args.dry_run {
        return show(Board::new(NoPins, pulse)?, "dry run, no pin is touched").await;
    }
    let pins = GpioPins::open(&args.chip)
        .context("cannot use the adapter board (--dry-run needs no pins)")?;
    let priority = if enable_realtime() {
        "real-time priority"
    } else {
        "normal priority, pulses may run long (run as root for real-time priority)"
    };
    let status = format!("{} | {priority}", args.chip.display());
    show(Board::new(pins, pulse)?, &status).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::ROW_LOW;
    use crate::board::tests::{board, index};
    use crate::tui::tests::render;

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
            describe_address("column", 31, u8::MAX),
            "column address 31 = column 28"
        );
    }

    #[test]
    fn keys_select_press_and_quit() {
        use crossterm::event::KeyEvent;
        let key = |code, modifiers| handle_event(&Event::Key(KeyEvent::new(code, modifiers)));
        let none = KeyModifiers::NONE;

        assert_eq!(key(KeyCode::Char('q'), none), Action::Quit);
        assert_eq!(key(KeyCode::Esc, none), Action::Quit);
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), Action::Quit);
        assert_eq!(key(KeyCode::Char('c'), none), Action::None);
        assert_eq!(key(KeyCode::Up, none), Action::Up);
        assert_eq!(key(KeyCode::Char('j'), none), Action::Down);
        assert_eq!(key(KeyCode::Char(' '), none), Action::Press);
        assert_eq!(key(KeyCode::Enter, none), Action::Press);
        assert_eq!(key(KeyCode::Char('0'), none), Action::AllLow);
        assert_eq!(handle_event(&Event::Resize(80, 24)), Action::Redraw);
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
