//! `debug live`: a continuously updated table with one row per aircraft.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Row, Table as TableWidget, TableState};
use rs1090::decode::cpr::{AircraftState, decode_position};
use rs1090::decode::{DF, ICAO, Message, TimedMessage};
use serde_json::Value;

use crate::source::{FrameStream, SourceArgs};

/// How often the screen is checked for redraw.
const TICK: Duration = Duration::from_millis(200);
/// Redraw at least this often so the age column keeps counting.
const IDLE_REDRAW: Duration = Duration::from_secs(1);
/// A row is highlighted for this long after it received a message.
const FRESH: Duration = Duration::from_secs(1);
/// Rows scrolled per PageUp / PageDown.
const PAGE: usize = 10;

#[derive(clap::Args, Debug)]
pub struct LiveArgs {
    #[command(flatten)]
    pub source: SourceArgs,

    /// Remove an aircraft after this many seconds without a message.
    #[arg(long, value_name = "SECONDS", default_value_t = 30)]
    pub expire: u64,
}

/// Everything known about one aircraft, merged from all its messages.
#[derive(Debug, Default, Clone, PartialEq)]
struct Aircraft {
    callsign: Option<String>,
    squawk: Option<String>,
    altitude_ft: Option<i64>,
    groundspeed_kt: Option<f64>,
    track_deg: Option<f64>,
    vertical_rate_fpm: Option<i64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    rssi_db: Option<f32>,
    messages: u64,
    last_seen: Option<Instant>,
}

/// The aircraft table. Pure state, no I/O, so it can be unit tested.
struct Table {
    rows: BTreeMap<u32, Aircraft>,
    /// rs1090's own per-aircraft state, needed to pair CPR position frames.
    cpr: BTreeMap<ICAO, AircraftState>,
    expire: Duration,
    frames: u64,
}

impl Table {
    fn new(expire: Duration) -> Self {
        Self {
            rows: BTreeMap::new(),
            cpr: BTreeMap::new(),
            expire,
            frames: 0,
        }
    }

    /// Merge one frame into the table. Returns true if a row changed.
    fn update(&mut self, msg: &TimedMessage, now: Instant) -> bool {
        self.frames += 1;
        let Ok(mut decoded) = Message::try_from(msg.frame.as_slice()) else {
            return false;
        };

        // Resolve latitude/longitude from pairs of CPR encoded position frames.
        if let DF::ExtendedSquitterADSB(adsb) = &mut decoded.df {
            let icao = adsb.icao24;
            decode_position(
                &mut adsb.message,
                msg.timestamp,
                &icao,
                &mut self.cpr,
                &mut None,
                &None,
            );
        }

        let Ok(json) = serde_json::to_value(&decoded) else {
            return false;
        };
        let df = json.get("df").and_then(Value::as_str).unwrap_or("");

        // Only all-call replies and extended squitters carry a checksum that
        // proves the address. For every other format the address is derived
        // from the checksum itself, so noise produces random aircraft. Those
        // formats may update a known aircraft but never create a row.
        let verified = matches!(df, "11" | "17" | "18");
        let icao = match json.get("icao24").and_then(Value::as_str) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => Some(decoded.crc),
        };
        let Some(icao) = icao else { return false };
        if !verified && !self.rows.contains_key(&icao) {
            return false;
        }

        let row = self.rows.entry(icao).or_default();
        row.messages += 1;
        row.last_seen = Some(now);
        if let Some(rssi) = msg.metadata.first().and_then(|m| m.rssi) {
            row.rssi_db = Some(rssi);
        }
        if let Some(v) = json.get("callsign").and_then(Value::as_str) {
            row.callsign = Some(v.trim_end_matches([' ', '_']).to_string());
        }
        if let Some(v) = json.get("squawk").and_then(Value::as_str) {
            row.squawk = Some(v.to_string());
        }
        if let Some(v) = json.get("altitude").and_then(Value::as_i64) {
            row.altitude_ft = Some(v);
        }
        if let Some(v) = json.get("groundspeed").and_then(Value::as_f64) {
            row.groundspeed_kt = Some(v);
        }
        if let Some(v) = json.get("track").and_then(Value::as_f64) {
            row.track_deg = Some(v);
        }
        if let Some(v) = json.get("vertical_rate").and_then(Value::as_i64) {
            row.vertical_rate_fpm = Some(v);
        }
        if let (Some(lat), Some(lon)) = (
            json.get("latitude").and_then(Value::as_f64),
            json.get("longitude").and_then(Value::as_f64),
        ) {
            row.latitude = Some(lat);
            row.longitude = Some(lon);
        }
        true
    }

    /// Drop aircraft not heard from within the expiry time. Returns true if
    /// any row was removed.
    fn expire(&mut self, now: Instant) -> bool {
        let before = self.rows.len();
        let expire = self.expire;
        self.rows.retain(|_, a| {
            a.last_seen
                .is_some_and(|seen| now.duration_since(seen) <= expire)
        });
        let rows = &self.rows;
        self.cpr.retain(|icao, _| rows.contains_key(&icao.0));
        self.rows.len() != before
    }
}

fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> String) -> String {
    value.map(f).unwrap_or_default()
}

/// Column titles, widths and whether the column is right aligned.
const COLUMNS: [(&str, u16, bool); 12] = [
    ("ICAO", 6, false),
    ("CALLSIGN", 8, false),
    ("SQUAWK", 6, true),
    ("ALT ft", 7, true),
    ("GS kt", 6, true),
    ("TRK", 5, true),
    ("VS fpm", 7, true),
    ("LAT", 9, true),
    ("LON", 10, true),
    ("RSSI", 6, true),
    ("MSGS", 6, true),
    ("AGE", 4, true),
];

fn cell(text: String, column: usize) -> Cell<'static> {
    let line = Line::from(text);
    Cell::from(if COLUMNS[column].2 {
        line.right_aligned()
    } else {
        line
    })
}

/// Draw header, aircraft table and footer into `frame`.
fn draw(frame: &mut Frame, table: &Table, scroll: &mut usize, status: &str, now: Instant) {
    let [header_area, table_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Line::from(format!(
            "adsb-flipdot live | {status} | {} aircraft | {} frames | expire {} s",
            table.rows.len(),
            table.frames,
            table.expire.as_secs()
        ))
        .style(Style::new().add_modifier(Modifier::BOLD)),
        header_area,
    );

    let header = Row::new(
        COLUMNS
            .iter()
            .enumerate()
            .map(|(i, (title, _, _))| cell(title.to_string(), i)),
    )
    .style(Style::new().add_modifier(Modifier::REVERSED));

    let rows = table.rows.iter().map(|(icao, a)| {
        let age = a
            .last_seen
            .map(|seen| now.duration_since(seen))
            .unwrap_or_default();
        let style = if age < FRESH {
            Style::new().add_modifier(Modifier::BOLD)
        } else if age > table.expire / 2 {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new()
        };
        let cells = [
            format!("{icao:06x}"),
            a.callsign.clone().unwrap_or_default(),
            a.squawk.clone().unwrap_or_default(),
            opt(a.altitude_ft, |v| v.to_string()),
            opt(a.groundspeed_kt, |v| format!("{v:.0}")),
            opt(a.track_deg, |v| format!("{v:.0}°")),
            opt(a.vertical_rate_fpm, |v| format!("{v:+}")),
            opt(a.latitude, |v| format!("{v:.4}")),
            opt(a.longitude, |v| format!("{v:.4}")),
            opt(a.rssi_db, |v| format!("{v:.1}")),
            a.messages.to_string(),
            format!("{}s", age.as_secs()),
        ];
        Row::new(cells.into_iter().enumerate().map(|(i, text)| cell(text, i))).style(style)
    });

    let mut block = Block::new().borders(Borders::BOTTOM);
    if table.rows.is_empty() {
        block = block.title_bottom(Line::from(" waiting for aircraft ").centered());
    }
    let widget = TableWidget::new(rows, COLUMNS.map(|(_, width, _)| Constraint::Length(width)))
        .header(header)
        .column_spacing(2)
        .block(block);

    // Keep the scroll position inside the table as rows come and go.
    *scroll = (*scroll).min(table.rows.len().saturating_sub(1));
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

pub async fn run(args: LiveArgs) -> Result<()> {
    let mut stream = FrameStream::open(&args.source).await?;
    let status = stream.description().to_string();
    let mut table = Table::new(Duration::from_secs(args.expire));
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
                Some(msg) => dirty |= table.update(&msg, Instant::now()),
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
                dirty |= table.expire(now);
                if dirty || now.duration_since(last_draw) >= IDLE_REDRAW {
                    terminal
                        .draw(|frame| draw(frame, &table, &mut scroll, &status, now))
                        .context("cannot draw to the terminal")?;
                    dirty = false;
                    last_draw = now;
                }
            }
        }
    }

    drop(guard);
    let result = stream.close().await;
    eprintln!(
        "adsb-flipdot: {} frames received, {} aircraft in view at exit",
        table.frames,
        table.rows.len()
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::tests::timed;

    // Reference frames from "The 1090 Megahertz Riddle" (Junzi Sun).
    const IDENT: &str = "8d4840d6202cc371c32ce0576098"; // 4840d6, KLM1023
    const VELOCITY: &str = "8d485020994409940838175b284f"; // 485020, 159 kt
    const POS_EVEN: &str = "8d40621d58c382d690c8ac2863a7"; // 40621d
    const POS_ODD: &str = "8d40621d58c386435cc412692ad6";
    const SECS_30: Duration = Duration::from_secs(30);

    #[test]
    fn identification_creates_row_with_callsign() {
        let mut t = Table::new(SECS_30);
        assert!(t.update(&timed(IDENT, 1.0), Instant::now()));
        let a = &t.rows[&0x4840d6];
        assert_eq!(a.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(a.messages, 1);
    }

    #[test]
    fn velocity_fills_speed_track_and_vertical_rate() {
        let mut t = Table::new(SECS_30);
        t.update(&timed(VELOCITY, 1.0), Instant::now());
        let a = &t.rows[&0x485020];
        assert!((a.groundspeed_kt.unwrap() - 159.2).abs() < 0.1);
        assert!((a.track_deg.unwrap() - 182.88).abs() < 0.01);
        assert_eq!(a.vertical_rate_fpm, Some(-832));
    }

    #[test]
    fn position_pair_resolves_latitude_and_longitude() {
        let mut t = Table::new(SECS_30);
        let now = Instant::now();
        t.update(&timed(POS_ODD, 1.0), now);
        assert_eq!(t.rows[&0x40621d].latitude, None, "one frame is not enough");
        t.update(&timed(POS_EVEN, 2.0), now);
        let a = &t.rows[&0x40621d];
        assert!((a.latitude.unwrap() - 52.2572).abs() < 0.001, "{a:?}");
        assert!((a.longitude.unwrap() - 3.9194).abs() < 0.001, "{a:?}");
        assert_eq!(a.altitude_ft, Some(38000));
        assert_eq!(a.messages, 2);
    }

    #[test]
    fn unverified_format_does_not_create_a_row() {
        // DF4 altitude reply: the address comes from the checksum, so it
        // cannot be trusted on its own.
        let mut t = Table::new(SECS_30);
        assert!(!t.update(&timed("200004bb11d2c1", 1.0), Instant::now()));
        assert!(t.rows.is_empty());
    }

    #[test]
    fn unverified_format_updates_a_known_aircraft() {
        let mut t = Table::new(SECS_30);
        let now = Instant::now();
        // The aircraft is known from an earlier verified message.
        t.rows.insert(0x503d48, Aircraft::default());
        assert!(t.update(&timed("200004bb11d2c1", 2.0), now));
        let a = &t.rows[&0x503d48];
        assert_eq!(a.altitude_ft, Some(6875));
        assert_eq!(a.messages, 1);
    }

    #[test]
    fn rows_expire_after_the_timeout() {
        let mut t = Table::new(SECS_30);
        let start = Instant::now();
        t.update(&timed(IDENT, 1.0), start);
        t.update(&timed(VELOCITY, 1.0), start + Duration::from_secs(20));

        assert!(!t.expire(start + Duration::from_secs(30)), "30 s is kept");
        assert_eq!(t.rows.len(), 2);

        assert!(t.expire(start + Duration::from_secs(31)));
        assert!(!t.rows.contains_key(&0x4840d6), "stale row removed");
        assert!(t.rows.contains_key(&0x485020), "fresh row kept");

        assert!(t.expire(start + Duration::from_secs(60)));
        assert!(t.rows.is_empty());
        assert!(t.cpr.is_empty(), "decoder state is pruned too");
    }

    /// Draw the table into an in-memory terminal and return its text lines.
    fn screen(t: &Table, scroll: &mut usize, now: Instant, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(110, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, t, scroll, "test", now))
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
    fn draw_shows_header_columns_and_aircraft_row() {
        let mut t = Table::new(SECS_30);
        let start = Instant::now();
        t.update(&timed(IDENT, 1.0), start);
        t.update(&timed(VELOCITY, 1.0), start);

        let lines = screen(&t, &mut 0, start + Duration::from_secs(7), 10);
        assert!(
            lines[0].contains("2 aircraft | 2 frames | expire 30 s"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].starts_with("ICAO    CALLSIGN  SQUAWK"),
            "{}",
            lines[1]
        );
        assert!(lines[2].starts_with("4840d6  KLM1023"), "{}", lines[2]);
        assert!(lines[2].ends_with("  7s"), "{}", lines[2]);
        // 485020: 159 kt, track 183, descending 832 ft/min.
        assert!(lines[3].starts_with("485020"), "{}", lines[3]);
        assert!(lines[3].contains("159"), "{}", lines[3]);
        assert!(lines[3].contains("183°"), "{}", lines[3]);
        assert!(lines[3].contains("-832"), "{}", lines[3]);
    }

    #[test]
    fn draw_empty_table_shows_placeholder() {
        let t = Table::new(SECS_30);
        let lines = screen(&t, &mut 0, Instant::now(), 8);
        assert!(
            lines.iter().any(|l| l.contains("waiting for aircraft")),
            "{lines:#?}"
        );
    }

    #[test]
    fn draw_scrolls_and_clamps_the_offset() {
        let mut t = Table::new(SECS_30);
        let now = Instant::now();
        t.update(&timed(IDENT, 1.0), now);
        t.update(&timed(VELOCITY, 1.0), now);

        // Scrolled by one: the first aircraft is off screen.
        let mut scroll = 1;
        let lines = screen(&t, &mut scroll, now, 10);
        assert!(lines[2].starts_with("485020"), "{}", lines[2]);

        // Far past the end: clamped to the last row.
        let mut scroll = 99;
        screen(&t, &mut scroll, now, 10);
        assert_eq!(scroll, 1);
    }

    #[test]
    fn keys_quit_and_scroll() {
        use crossterm::event::KeyEvent;
        let key = |code, modifiers| Event::Key(KeyEvent::new(code, modifiers));
        let none = KeyModifiers::NONE;
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
            handle_event(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut scroll),
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
