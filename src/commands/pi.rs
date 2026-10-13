//! `debug pi`: switch the signals of the adapter board by hand.
//!
//! Every signal the board passes on to the panel is a row in a table. The
//! address and direction lines are toggled; the COL-SEL lines are pulsed.
//! The rules are those of [`Board`], shared with the panel output.

use anyhow::{Context, Result};
use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Row, Table};

use crate::cli::PiArgs;
use crate::commands::shutdown::Shutdown;
use crate::commands::tui::{Column, Input, Screen, cell};
use crate::output::board::{
    Board, COL_ADDRESS, MAX_COLUMNS, MAX_ROWS, NoPins, OVERRUN_FACTOR, Pins, ROW_ADDRESS, SIGNALS,
    line_number, micros,
};

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

/// Show the board until the user quits. Every line is low afterwards.
async fn show<P: Pins>(mut board: Board<P>, status: &str) -> Result<()> {
    let mut shutdown = Shutdown::listen()?;
    let mut screen = Screen::open()?;

    let mut selected = 0usize;
    loop {
        screen.draw(|frame| draw(frame, &board, selected, status))?;
        tokio::select! {
            // Signals sent from outside, e.g. `kill`.
            _ = shutdown.recv() => break,
            input = screen.input() => match input? {
                Input::Quit => break,
                Input::Key(key) => match action(key) {
                    Some(Action::Up) => selected = selected.saturating_sub(1),
                    Some(Action::Down) => selected = (selected + 1).min(SIGNALS.len() - 1),
                    Some(Action::Press) => board.press(selected)?,
                    Some(Action::AllLow) => board.all_low()?,
                    None => {}
                },
                Input::Resize | Input::Other => {}
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
    if args.dry_run {
        let board = Board::new(NoPins, config.pulse)?;
        return show(board, "dry run, no pin is touched").await;
    }
    let (board, priority) = config
        .open()
        .context("cannot use the adapter board (--dry-run needs no pins)")?;
    let status = format!("{} | {priority}", config.chip.display());
    show(board, &status).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tui::tests::render;
    use crate::output::board::ROW_LOW;
    use crate::output::board::tests::{board, index};

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
