//! Shared terminal UI: the terminal in full-screen mode with its keys, and
//! a scrollable table fed by the frame stream.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::{DefaultTerminal, Frame};
use rs1090::decode::TimedMessage;

use crate::commands::shutdown::Shutdown;
use crate::source::stream::{FrameStream, SourceArgs};

/// How often the screen is checked for redraw.
const TICK: Duration = Duration::from_millis(200);
/// Redraw at least this often so age columns keep counting.
const IDLE_REDRAW: Duration = Duration::from_secs(1);
/// A row is highlighted for this long after it received a message.
const FRESH: Duration = Duration::from_secs(1);
/// Rows scrolled per PageUp / PageDown.
const PAGE: usize = 10;

/// What the user did at the terminal.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Input {
    /// q, Esc or Ctrl-C, or the terminal is gone.
    Quit,
    /// Any other key.
    Key(KeyCode),
    /// The window has a new size.
    Resize,
    /// Nothing a command reacts to.
    Other,
}

fn input(event: &Event) -> Input {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Input::Quit,
            // Raw mode turns Ctrl-C into a plain key press instead of SIGINT.
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Input::Quit,
            code => Input::Key(code),
        },
        Event::Resize(..) => Input::Resize,
        _ => Input::Other,
    }
}

/// The terminal in raw mode on the alternate screen. It is restored when
/// this is dropped, also when a command returns an error.
pub(crate) struct Screen {
    terminal: DefaultTerminal,
    events: EventStream,
}

impl Screen {
    pub(crate) fn open() -> Result<Self> {
        // `try_init` enters raw mode and the alternate screen, and installs
        // a panic hook that restores the terminal.
        let terminal =
            ratatui::try_init().context("cannot set up the terminal (is stdout a TTY?)")?;
        Ok(Self {
            terminal,
            events: EventStream::new(),
        })
    }

    pub(crate) fn draw(&mut self, draw: impl FnOnce(&mut Frame)) -> Result<()> {
        self.terminal
            .draw(draw)
            .context("cannot draw to the terminal")?;
        Ok(())
    }

    /// Wait for the next thing the user does. Cancel safe.
    pub(crate) async fn input(&mut self) -> Result<Input> {
        match self.events.next().await {
            Some(Ok(event)) => Ok(input(&event)),
            Some(Err(e)) => Err(e).context("terminal input failed"),
            None => Ok(Input::Quit),
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

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

/// One cell of a table, aligned like its column.
pub(crate) fn cell(text: String, column: &Column) -> Cell<'static> {
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

/// Move the scroll position if `key` is a scroll key. Returns whether it
/// was one.
fn scroll_by(key: KeyCode, scroll: &mut usize) -> bool {
    *scroll = match key {
        KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
        KeyCode::PageDown => scroll.saturating_add(PAGE),
        KeyCode::PageUp => scroll.saturating_sub(PAGE),
        KeyCode::Home => 0,
        _ => return false,
    };
    true
}

/// Open the frame source and show `view` until the user quits or the source
/// ends.
pub async fn run(source: &SourceArgs, view: &mut impl View) -> Result<()> {
    let mut shutdown = Shutdown::listen()?;
    let mut stream = FrameStream::open(source).await?;
    let status = stream.description().to_string();
    let mut scroll = 0usize;

    let mut screen = Screen::open()?;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut dirty = true;
    let mut last_draw = Instant::now();
    loop {
        tokio::select! {
            // Signals sent from outside, e.g. `kill`.
            _ = shutdown.recv() => break,
            msg = stream.recv() => match msg {
                Some(msg) => dirty |= view.update(&msg, Instant::now()),
                None => break,
            },
            input = screen.input() => match input? {
                Input::Quit => break,
                Input::Key(key) => dirty |= scroll_by(key, &mut scroll),
                Input::Resize => dirty = true,
                Input::Other => {}
            },
            _ = tick.tick() => {
                let now = Instant::now();
                dirty |= view.expire(now);
                if dirty || now.duration_since(last_draw) >= IDLE_REDRAW {
                    screen.draw(|frame| view.draw(frame, &mut scroll, &status, now))?;
                    dirty = false;
                    last_draw = now;
                }
            }
        }
    }

    drop(screen);
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
        render(width, height, |frame| view.draw(frame, scroll, "test", now))
    }

    /// Draw into an in-memory terminal and return its text lines.
    pub(crate) fn render(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(draw).unwrap();
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
    fn q_esc_and_ctrl_c_quit() {
        use crossterm::event::{KeyEvent, KeyEventKind};
        let key = |code, modifiers| input(&Event::Key(KeyEvent::new(code, modifiers)));
        let none = KeyModifiers::NONE;

        assert_eq!(key(KeyCode::Char('q'), none), Input::Quit);
        assert_eq!(key(KeyCode::Esc, none), Input::Quit);
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), Input::Quit);
        assert_eq!(
            key(KeyCode::Char('c'), none),
            Input::Key(KeyCode::Char('c'))
        );
        assert_eq!(key(KeyCode::Down, none), Input::Key(KeyCode::Down));
        assert_eq!(input(&Event::Resize(80, 24)), Input::Resize);
        assert_eq!(input(&Event::FocusGained), Input::Other);

        // A key counts when it goes down, not when it comes up again.
        let release = KeyEvent::new_with_kind(KeyCode::Char('q'), none, KeyEventKind::Release);
        assert_eq!(input(&Event::Key(release)), Input::Other);
    }

    #[test]
    fn keys_scroll() {
        let mut scroll = 0;
        assert!(scroll_by(KeyCode::Down, &mut scroll));
        assert_eq!(scroll, 1);
        assert!(scroll_by(KeyCode::PageDown, &mut scroll));
        assert_eq!(scroll, 11);
        assert!(scroll_by(KeyCode::Char('k'), &mut scroll));
        assert_eq!(scroll, 10);
        assert!(scroll_by(KeyCode::Home, &mut scroll));
        assert_eq!(scroll, 0);
        assert!(scroll_by(KeyCode::Up, &mut scroll));
        assert_eq!(scroll, 0, "does not underflow");

        assert!(!scroll_by(KeyCode::Char('x'), &mut scroll));
        assert_eq!(scroll, 0);
    }
}
