//! Shared terminal UI: a scrollable table fed by the frame stream.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use rs1090::decode::TimedMessage;

use crate::source::{FrameStream, SourceArgs};

/// How often the screen is checked for redraw.
const TICK: Duration = Duration::from_millis(200);
/// Redraw at least this often so age columns keep counting.
const IDLE_REDRAW: Duration = Duration::from_secs(1);
/// A row is highlighted for this long after it received a message.
const FRESH: Duration = Duration::from_secs(1);
/// Rows scrolled per PageUp / PageDown.
const PAGE: usize = 10;

/// A table column: title, width in characters, right aligned.
pub type Column = (&'static str, u16, bool);

/// Something that turns frames into a table on screen.
pub trait View {
    /// Merge one frame. Returns true if the screen needs a redraw.
    fn update(&mut self, msg: &TimedMessage, now: Instant) -> bool;
    /// Drop stale rows. Returns true if the screen needs a redraw.
    fn expire(&mut self, now: Instant) -> bool;
    /// Draw the whole screen. `status` describes the frame source.
    fn draw(&self, frame: &mut Frame, scroll: &mut usize, status: &str, now: Instant);
    /// One line printed after the UI has closed.
    fn summary(&self) -> String;
}

/// Bold right after an update, dimmed once silent for half the expiry time.
pub fn age_style(age: Duration, expire: Duration) -> Style {
    if age < FRESH {
        Style::new().add_modifier(Modifier::BOLD)
    } else if age > expire / 2 {
        Style::new().add_modifier(Modifier::DIM)
    } else {
        Style::new()
    }
}

/// Format an optional value, empty when absent.
pub fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> String) -> String {
    value.map(f).unwrap_or_default()
}

fn cell(text: String, column: &Column) -> Cell<'static> {
    let line = Line::from(text);
    Cell::from(if column.2 { line.right_aligned() } else { line })
}

/// Draw a header line, a scrollable table and a footer with the key bindings.
pub fn draw_table(
    frame: &mut Frame,
    scroll: &mut usize,
    header: String,
    columns: &[Column],
    rows: Vec<(Vec<String>, Style)>,
    empty: &str,
) {
    let [header_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Line::from(header).style(Style::new().add_modifier(Modifier::BOLD)),
        header_area,
    );

    let titles = Row::new(columns.iter().map(|c| cell(c.0.to_string(), c)))
        .style(Style::new().add_modifier(Modifier::REVERSED));

    let mut block = Block::new().borders(Borders::BOTTOM);
    if rows.is_empty() {
        block = block.title_bottom(Line::from(format!(" {empty} ")).centered());
    }
    // Keep the scroll position inside the table as rows come and go.
    *scroll = (*scroll).min(rows.len().saturating_sub(1));

    let rows = rows.into_iter().map(|(cells, style)| {
        Row::new(
            cells
                .into_iter()
                .zip(columns)
                .map(|(text, column)| cell(text, column)),
        )
        .style(style)
    });
    let widget = Table::new(rows, columns.iter().map(|c| Constraint::Length(c.1)))
        .header(titles)
        .column_spacing(2)
        .block(block);

    let mut state = TableState::new().with_offset(*scroll);
    frame.render_stateful_widget(widget, table_area, &mut state);

    frame.render_widget(
        Line::from("q / Esc / Ctrl-C quit   ↑ ↓ PgUp PgDn Home scroll")
            .style(Style::new().add_modifier(Modifier::DIM)),
        footer_area,
    );
}

/// What a key press asks the event loop to do.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Quit,
    Redraw,
    None,
}

fn handle_event(event: &Event, scroll: &mut usize) -> Action {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            // Raw mode turns Ctrl-C into a plain key press instead of SIGINT.
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => {
                *scroll = scroll.saturating_add(1);
                Action::Redraw
            }
            KeyCode::Up | KeyCode::Char('k') => {
                *scroll = scroll.saturating_sub(1);
                Action::Redraw
            }
            KeyCode::PageDown => {
                *scroll = scroll.saturating_add(PAGE);
                Action::Redraw
            }
            KeyCode::PageUp => {
                *scroll = scroll.saturating_sub(PAGE);
                Action::Redraw
            }
            KeyCode::Home => {
                *scroll = 0;
                Action::Redraw
            }
            _ => Action::None,
        },
        Event::Resize(..) => Action::Redraw,
        _ => Action::None,
    }
}

/// Restores the terminal when dropped, also when `run` returns an error.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Open the frame source and show `view` until the user quits or the source
/// ends.
pub async fn run(source: &SourceArgs, view: &mut impl View) -> Result<()> {
    let mut stream = FrameStream::open(source).await?;
    let status = stream.description().to_string();
    let mut scroll = 0usize;

    // `try_init` enters raw mode and the alternate screen, and installs a
    // panic hook that restores the terminal.
    let mut terminal =
        ratatui::try_init().context("cannot set up the terminal (is stdout a TTY?)")?;
    let guard = TerminalGuard;

    let mut events = EventStream::new();
    let sigint = tokio::signal::ctrl_c();
    tokio::pin!(sigint);
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut dirty = true;
    let mut last_draw = Instant::now();
    loop {
        tokio::select! {
            // SIGINT sent from outside, e.g. `kill -INT`.
            _ = &mut sigint => break,
            msg = stream.recv() => match msg {
                Some(msg) => dirty |= view.update(&msg, Instant::now()),
                None => break,
            },
            event = events.next() => match event {
                Some(Ok(event)) => match handle_event(&event, &mut scroll) {
                    Action::Quit => break,
                    Action::Redraw => dirty = true,
                    Action::None => {}
                },
                Some(Err(e)) => return Err(e).context("terminal input failed"),
                None => break,
            },
            _ = tick.tick() => {
                let now = Instant::now();
                dirty |= view.expire(now);
                if dirty || now.duration_since(last_draw) >= IDLE_REDRAW {
                    terminal
                        .draw(|frame| view.draw(frame, &mut scroll, &status, now))
                        .context("cannot draw to the terminal")?;
                    dirty = false;
                    last_draw = now;
                }
            }
        }
    }

    drop(guard);
    let result = stream.close().await;
    eprintln!("adsb-flipdot: {}", view.summary());
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Draw a view into an in-memory terminal and return its text lines.
    pub(crate) fn screen(
        view: &impl View,
        scroll: &mut usize,
        now: Instant,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| view.draw(frame, scroll, "test", now))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn keys_quit_and_scroll() {
        use crossterm::event::KeyEvent;
        let key = |code, modifiers| Event::Key(KeyEvent::new(code, modifiers));
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let mut scroll = 0;

        assert_eq!(
            handle_event(&key(KeyCode::Char('q'), none), &mut scroll),
            Action::Quit
        );
        assert_eq!(
            handle_event(&key(KeyCode::Esc, none), &mut scroll),
            Action::Quit
        );
        assert_eq!(
            handle_event(&key(KeyCode::Char('c'), ctrl), &mut scroll),
            Action::Quit
        );
        assert_eq!(
            handle_event(&key(KeyCode::Char('c'), none), &mut scroll),
            Action::None
        );

        assert_eq!(
            handle_event(&key(KeyCode::Down, none), &mut scroll),
            Action::Redraw
        );
        assert_eq!(scroll, 1);
        handle_event(&key(KeyCode::PageDown, none), &mut scroll);
        assert_eq!(scroll, 11);
        handle_event(&key(KeyCode::Up, none), &mut scroll);
        assert_eq!(scroll, 10);
        handle_event(&key(KeyCode::Home, none), &mut scroll);
        assert_eq!(scroll, 0);
        handle_event(&key(KeyCode::Up, none), &mut scroll);
        assert_eq!(scroll, 0, "does not underflow");

        assert_eq!(
            handle_event(&Event::Resize(80, 24), &mut scroll),
            Action::Redraw
        );
    }
}
