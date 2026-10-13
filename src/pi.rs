//! `debug pi`: switch the signals of the adapter board by hand.
//!
//! Every signal the board passes on to the panel is a row in a table. The
//! address and direction lines are toggled; the COL-SEL lines, which let the
//! coil current flow, can only be pulsed. [`Board`] holds those rules and is
//! independent of the terminal and of real pins.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use gpiocdev::line::Value;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Cell, Row, Table};
use tokio::signal::unix::{SignalKind, signal};

use crate::tui::TerminalGuard;

/// Real-time priority of the thread that times the pulses, 1 to 99.
const REALTIME_PRIORITY: libc::c_int = 50;
/// A pulse this many times longer than asked for counts as an overrun.
const OVERRUN_FACTOR: u32 = 2;

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

/// How a signal reacts to the space bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Stays at the level it is set to.
    Level,
    /// Goes high for one pulse and returns low by itself.
    Pulse,
}

/// One signal of the adapter board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Signal {
    name: &'static str,
    /// BCM number of the GPIO.
    gpio: u32,
    /// Pin on the Pi's 40-pin header.
    header_pin: u8,
    /// Pin on the panel's 26-pin header.
    panel_pin: u8,
    kind: Kind,
    meaning: &'static str,
}

const fn level(
    name: &'static str,
    gpio: u32,
    header_pin: u8,
    panel_pin: u8,
    meaning: &'static str,
) -> Signal {
    Signal {
        name,
        gpio,
        header_pin,
        panel_pin,
        kind: Kind::Level,
        meaning,
    }
}

const fn pulse(
    name: &'static str,
    gpio: u32,
    header_pin: u8,
    panel_pin: u8,
    meaning: &'static str,
) -> Signal {
    Signal {
        name,
        gpio,
        header_pin,
        panel_pin,
        kind: Kind::Pulse,
        meaning,
    }
}

/// The signals as wired by `pcb/build_board.py`.
const SIGNALS: [Signal; 18] = [
    level("ROW0", 17, 11, 5, "Row address, bit 0"),
    level("ROW1", 27, 13, 7, "Row address, bit 1"),
    level("ROW2", 22, 15, 6, "Row address, bit 2"),
    level("ROW3", 10, 19, 8, "Row address, bit 3"),
    level("ROW4", 9, 21, 9, "Row address, bit 4"),
    level(
        "ROW-HIGH",
        11,
        23,
        10,
        "Row feeds current from the coil supply",
    ),
    level("ROW-LOW", 13, 33, 11, "Row drains current to ground"),
    level("COL0", 18, 12, 13, "Column address, bit 0"),
    level("COL1", 23, 16, 14, "Column address, bit 1"),
    level("COL2", 24, 18, 15, "Column address, bit 2"),
    level("COL3", 25, 22, 16, "Column address, bit 3"),
    level("COL4", 4, 7, 17, "Column address, bit 4"),
    level("COL-H/L", 12, 32, 18, "Column: 0 = supply, 1 = ground"),
    pulse("COL-SEL0", 16, 36, 19, "Pulse: column driver of panel 1"),
    pulse("COL-SEL1", 20, 38, 20, "Pulse: column driver of panel 2"),
    pulse("COL-SEL2", 21, 40, 21, "Pulse: column driver of panel 3"),
    pulse("COL-SEL3", 19, 35, 22, "Pulse: column driver of panel 4"),
    pulse("COL-SEL4", 26, 37, 23, "Pulse: column driver of panel 5"),
];

/// Positions in [`SIGNALS`].
const ROW_ADDRESS: Range<usize> = 0..5;
const ROW_HIGH: usize = 5;
const ROW_LOW: usize = 6;
const COL_ADDRESS: Range<usize> = 7..12;
const COL_HIGH_LOW: usize = 12;

/// Rows a panel can have at most.
const MAX_ROWS: u8 = 16;

/// The pins behind the signals.
trait Pins {
    fn set(&mut self, gpio: u32, high: bool) -> Result<()>;
}

/// The Pi's header pins, through the kernel's GPIO character device.
struct GpioPins(gpiocdev::Request);

impl GpioPins {
    /// Take every signal's line as an output, low.
    fn open(chip: &Path) -> Result<Self> {
        let lines: Vec<u32> = SIGNALS.iter().map(|signal| signal.gpio).collect();
        let request = gpiocdev::Request::builder()
            .on_chip(chip)
            .with_consumer("adsb-flipdot")
            .with_lines(&lines)
            .as_output(Value::Inactive)
            .request()
            .with_context(|| {
                format!(
                    "cannot take the GPIO lines on {} (is this a Raspberry Pi, may this user \
                     use GPIO, is another program holding the pins? --dry-run needs no pins)",
                    chip.display()
                )
            })?;
        Ok(Self(request))
    }
}

impl Pins for GpioPins {
    fn set(&mut self, gpio: u32, high: bool) -> Result<()> {
        let value = if high { Value::Active } else { Value::Inactive };
        self.0
            .set_value(gpio, value)
            .with_context(|| format!("cannot set GPIO {gpio}"))
    }
}

/// Pins for `--dry-run`: every level is accepted and nothing happens.
struct NoPins;

impl Pins for NoPins {
    fn set(&mut self, _gpio: u32, _high: bool) -> Result<()> {
        Ok(())
    }
}

/// The levels of all signals and the rules for changing them.
///
/// Every line is driven low when the board is dropped, so that no coil stays
/// energised however the program ends.
struct Board<P: Pins> {
    pins: P,
    levels: [bool; SIGNALS.len()],
    /// Length asked for.
    pulse: Duration,
    pulses: u64,
    /// Measured length of the latest pulse.
    last_pulse: Option<Duration>,
    longest_pulse: Option<Duration>,
    overruns: u64,
}

impl<P: Pins> Board<P> {
    /// Start with every line low.
    fn new(pins: P, pulse: Duration) -> Result<Self> {
        let mut board = Self {
            pins,
            levels: [false; SIGNALS.len()],
            pulse,
            pulses: 0,
            last_pulse: None,
            longest_pulse: None,
            overruns: 0,
        };
        board.all_low()?;
        Ok(board)
    }

    fn set(&mut self, index: usize, high: bool) -> Result<()> {
        self.pins.set(SIGNALS[index].gpio, high)?;
        self.levels[index] = high;
        Ok(())
    }

    /// What the space bar does to a signal: toggle a level, fire a pulse.
    fn press(&mut self, index: usize) -> Result<()> {
        match SIGNALS[index].kind {
            Kind::Pulse => self.pulse(index),
            Kind::Level => {
                let high = !self.levels[index];
                // ROW-HIGH and ROW-LOW together short the coil supply, so the
                // other one goes low first.
                if high && index == ROW_HIGH {
                    self.set(ROW_LOW, false)?;
                } else if high && index == ROW_LOW {
                    self.set(ROW_HIGH, false)?;
                }
                self.set(index, high)
            }
        }
    }

    /// Raise a line for the pulse length and lower it again.
    ///
    /// The wait is a busy loop: a sleep hands the CPU to the scheduler, which
    /// may return it much later.
    fn pulse(&mut self, index: usize) -> Result<()> {
        let gpio = SIGNALS[index].gpio;
        if let Err(error) = self.pins.set(gpio, true) {
            // The line may be high all the same.
            let _ = self.pins.set(gpio, false);
            return Err(error);
        }
        let raised = Instant::now();
        while raised.elapsed() < self.pulse {
            std::hint::spin_loop();
        }
        let lowered = self.pins.set(gpio, false);
        let length = raised.elapsed();
        lowered?;

        self.pulses += 1;
        self.last_pulse = Some(length);
        self.longest_pulse = self.longest_pulse.max(Some(length));
        if length > self.pulse * OVERRUN_FACTOR {
            self.overruns += 1;
        }
        Ok(())
    }

    /// Drive every line low, the COL-SEL lines first. Goes on after a
    /// failure and returns the first one.
    fn all_low(&mut self) -> Result<()> {
        let pulsed = |index: &usize| SIGNALS[*index].kind == Kind::Pulse;
        let order = (0..SIGNALS.len())
            .filter(pulsed)
            .chain((0..SIGNALS.len()).filter(|index| !pulsed(index)));
        let mut result = Ok(());
        for index in order {
            let outcome = self.set(index, false);
            if result.is_ok() {
                result = outcome;
            }
        }
        result
    }

    /// The number on a group of address lines.
    fn address(&self, bits: Range<usize>) -> u8 {
        self.levels[bits]
            .iter()
            .enumerate()
            .map(|(bit, high)| u8::from(*high) << bit)
            .sum()
    }

    /// Where the current would flow during a COL-SEL pulse.
    fn direction(&self) -> &'static str {
        match (
            self.levels[ROW_HIGH],
            self.levels[ROW_LOW],
            self.levels[COL_HIGH_LOW],
        ) {
            (true, false, true) => "row feeds, column drains",
            (false, true, false) => "column feeds, row drains",
            (true, false, false) => "row and column both at the supply",
            (false, true, true) => "row and column both at ground",
            (false, false, _) => "row not driven",
            (true, true, _) => "ROW-HIGH and ROW-LOW both high",
        }
    }
}

impl<P: Pins> Drop for Board<P> {
    fn drop(&mut self) {
        let _ = self.all_low();
    }
}

/// The row or column, counted from 1, that an address selects. Each decoder
/// chip serves seven lines and leaves every eighth code unused.
fn line_number(address: u8) -> Option<u8> {
    let (chip, line) = (address >> 3, address & 7);
    (line != 0).then_some(chip * 7 + line)
}

fn describe_address(what: &str, address: u8, max: u8) -> String {
    match line_number(address).filter(|line| *line <= max) {
        Some(line) => format!("{what} address {address} = {what} {line}"),
        None => format!("{what} address {address} = no {what}"),
    }
}

fn micros(duration: Option<Duration>) -> String {
    match duration {
        Some(duration) => format!("{} µs", duration.as_micros()),
        None => "-".to_string(),
    }
}

/// Give the calling thread real-time priority, so that ordinary processes
/// cannot interrupt a pulse, and keep its memory in RAM. Returns false when
/// the system does not allow it.
fn enable_realtime() -> bool {
    // SAFETY: `sched_param` is plain data for which all zeroes is valid.
    let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
    param.sched_priority = REALTIME_PRIORITY;
    // SAFETY: changes the scheduling of the calling thread only; `param`
    // lives until the call returns.
    let realtime =
        unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) } == 0;
    if realtime {
        // SAFETY: takes no pointers. A failure only means that a page fault
        // can still delay a pulse.
        unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
    }
    realtime
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
    eprintln!(
        "adsb-flipdot: {} pulses sent, longest {}, {} overruns; all lines low",
        board.pulses,
        micros(board.longest_pulse),
        board.overruns
    );
    result
}

pub async fn run(args: PiArgs) -> Result<()> {
    let pulse = Duration::from_micros(args.pulse_us.into());
    if args.dry_run {
        return show(Board::new(NoPins, pulse)?, "dry run, no pin is touched").await;
    }
    let pins = GpioPins::open(&args.chip)?;
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
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::tui::tests::render;

    /// Every level written, in order, as (GPIO, high).
    type Log = Rc<RefCell<Vec<(u32, bool)>>>;

    /// Pins that record what is written to them. Writing to the GPIO in
    /// `broken` fails.
    struct FakePins {
        log: Log,
        broken: Option<u32>,
    }

    impl Pins for FakePins {
        fn set(&mut self, gpio: u32, high: bool) -> Result<()> {
            self.log.borrow_mut().push((gpio, high));
            if self.broken == Some(gpio) {
                anyhow::bail!("GPIO {gpio} is broken");
            }
            Ok(())
        }
    }

    const PULSE: Duration = Duration::from_micros(50);

    /// A board on fake pins, with the writes of its start left out of the log.
    fn board() -> (Board<FakePins>, Log) {
        let log = Log::default();
        let pins = FakePins {
            log: log.clone(),
            broken: None,
        };
        let board = Board::new(pins, PULSE).unwrap();
        log.borrow_mut().clear();
        (board, log)
    }

    fn index(name: &str) -> usize {
        SIGNALS.iter().position(|s| s.name == name).unwrap()
    }

    fn gpio(name: &str) -> u32 {
        SIGNALS[index(name)].gpio
    }

    /// The signals in `pcb/build_board.py` as (name, Pi pin, panel pin).
    fn board_script_signals() -> Vec<(String, u8, u8)> {
        include_str!("../pcb/build_board.py")
            .split("(\"")
            .filter_map(|chunk| {
                let fields: Vec<&str> = chunk.split(')').next()?.split(',').collect();
                let [name, pi_pin, panel_pin, _buffer, _channel] = fields.as_slice() else {
                    return None;
                };
                Some((
                    name.trim_end_matches('"').to_string(),
                    pi_pin.trim().parse().ok()?,
                    panel_pin.trim().parse().ok()?,
                ))
            })
            .collect()
    }

    /// BCM GPIO behind each pin of the Pi's 40-pin header that the board uses.
    const HEADER_GPIO: [(u8, u32); 18] = [
        (7, 4),
        (11, 17),
        (12, 18),
        (13, 27),
        (15, 22),
        (16, 23),
        (18, 24),
        (19, 10),
        (21, 9),
        (22, 25),
        (23, 11),
        (32, 12),
        (33, 13),
        (35, 19),
        (36, 16),
        (37, 26),
        (38, 20),
        (40, 21),
    ];

    #[test]
    fn signals_match_the_board_script() {
        let ours: Vec<_> = SIGNALS
            .iter()
            .map(|s| (s.name.to_string(), s.header_pin, s.panel_pin))
            .collect();
        assert_eq!(ours, board_script_signals());

        for signal in SIGNALS {
            let expected = HEADER_GPIO
                .iter()
                .find(|(pin, _)| *pin == signal.header_pin)
                .map(|(_, gpio)| *gpio);
            assert_eq!(Some(signal.gpio), expected, "{}", signal.name);
        }
    }

    #[test]
    fn positions_name_the_right_signals() {
        let names =
            |range: Range<usize>| -> Vec<_> { SIGNALS[range].iter().map(|s| s.name).collect() };
        assert_eq!(names(ROW_ADDRESS), ["ROW0", "ROW1", "ROW2", "ROW3", "ROW4"]);
        assert_eq!(names(COL_ADDRESS), ["COL0", "COL1", "COL2", "COL3", "COL4"]);
        assert_eq!(SIGNALS[ROW_HIGH].name, "ROW-HIGH");
        assert_eq!(SIGNALS[ROW_LOW].name, "ROW-LOW");
        assert_eq!(SIGNALS[COL_HIGH_LOW].name, "COL-H/L");
        // Exactly the COL-SEL lines pulse.
        for signal in SIGNALS {
            assert_eq!(
                signal.kind == Kind::Pulse,
                signal.name.starts_with("COL-SEL"),
                "{}",
                signal.name
            );
        }
    }

    #[test]
    fn a_new_board_drives_every_line_low() {
        let log = Log::default();
        let pins = FakePins {
            log: log.clone(),
            broken: None,
        };
        let board = Board::new(pins, PULSE).unwrap();
        let written = log.borrow().clone();
        assert_eq!(written.len(), SIGNALS.len());
        assert!(written.iter().all(|(_, high)| !high));
        assert_eq!(written[0].0, gpio("COL-SEL0"), "COL-SEL lines first");
        assert!(board.levels.iter().all(|high| !high));
    }

    #[test]
    fn space_toggles_a_level_line() {
        let (mut board, log) = board();
        board.press(index("COL2")).unwrap();
        assert!(board.levels[index("COL2")]);
        board.press(index("COL2")).unwrap();
        assert!(!board.levels[index("COL2")]);
        assert_eq!(*log.borrow(), [(gpio("COL2"), true), (gpio("COL2"), false)]);
    }

    #[test]
    fn row_high_and_row_low_are_never_high_together() {
        let (mut board, log) = board();
        board.press(ROW_HIGH).unwrap();
        board.press(ROW_LOW).unwrap();
        assert!(!board.levels[ROW_HIGH]);
        assert!(board.levels[ROW_LOW]);
        assert_eq!(
            log.borrow()[2..],
            [(gpio("ROW-HIGH"), false), (gpio("ROW-LOW"), true)],
            "the other one goes low first"
        );

        board.press(ROW_HIGH).unwrap();
        assert!(board.levels[ROW_HIGH]);
        assert!(!board.levels[ROW_LOW]);

        // Lowering one does not touch the other.
        log.borrow_mut().clear();
        board.press(ROW_HIGH).unwrap();
        assert_eq!(*log.borrow(), [(gpio("ROW-HIGH"), false)]);
    }

    #[test]
    fn a_col_sel_line_pulses_and_returns_low() {
        let (mut board, log) = board();
        let select = index("COL-SEL1");
        board.press(select).unwrap();

        assert_eq!(
            *log.borrow(),
            [(gpio("COL-SEL1"), true), (gpio("COL-SEL1"), false)]
        );
        assert!(!board.levels[select], "never shown or left high");
        assert_eq!(board.pulses, 1);
        let length = board.last_pulse.unwrap();
        assert!(length >= PULSE, "{length:?}");
        assert_eq!(board.longest_pulse, Some(length));

        board.press(select).unwrap();
        assert_eq!(board.pulses, 2);
        assert!(board.longest_pulse >= board.last_pulse);
    }

    #[test]
    fn a_pulse_that_cannot_be_raised_is_lowered_all_the_same() {
        let log = Log::default();
        let pins = FakePins {
            log: log.clone(),
            broken: None,
        };
        let mut board = Board::new(pins, PULSE).unwrap();
        board.pins.broken = Some(gpio("COL-SEL0"));
        log.borrow_mut().clear();

        assert!(board.press(index("COL-SEL0")).is_err());
        assert_eq!(
            *log.borrow(),
            [(gpio("COL-SEL0"), true), (gpio("COL-SEL0"), false)]
        );
        assert_eq!(board.pulses, 0);
    }

    #[test]
    fn all_low_goes_on_after_a_failure() {
        let (mut board, log) = board();
        board.press(index("ROW0")).unwrap();
        board.press(index("COL4")).unwrap();
        board.pins.broken = Some(gpio("ROW0"));
        log.borrow_mut().clear();

        assert!(board.all_low().is_err());
        assert_eq!(log.borrow().len(), SIGNALS.len(), "every line was tried");
        assert!(!board.levels[index("COL4")]);
    }

    #[test]
    fn dropping_the_board_drives_every_line_low() {
        let (mut board, log) = board();
        board.press(index("ROW-HIGH")).unwrap();
        board.press(index("COL0")).unwrap();
        log.borrow_mut().clear();

        drop(board);
        let written = log.borrow().clone();
        assert_eq!(written.len(), SIGNALS.len());
        assert!(written.iter().all(|(_, high)| !high));
    }

    #[test]
    fn addresses_skip_every_eighth_code() {
        assert_eq!(line_number(0), None);
        assert_eq!(line_number(1), Some(1));
        assert_eq!(line_number(7), Some(7));
        assert_eq!(line_number(8), None);
        assert_eq!(line_number(9), Some(8));
        assert_eq!(line_number(15), Some(14));
        assert_eq!(line_number(16), None);
        assert_eq!(line_number(17), Some(15));
        assert_eq!(line_number(24), None);
        assert_eq!(line_number(31), Some(28));

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
    fn address_and_direction_follow_the_levels() {
        let (mut board, _log) = board();
        assert_eq!(board.address(ROW_ADDRESS), 0);
        assert_eq!(board.direction(), "row not driven");

        board.press(index("ROW0")).unwrap();
        board.press(index("ROW3")).unwrap();
        board.press(index("COL4")).unwrap();
        assert_eq!(board.address(ROW_ADDRESS), 9);
        assert_eq!(board.address(COL_ADDRESS), 16);

        board.press(ROW_LOW).unwrap();
        assert_eq!(board.direction(), "column feeds, row drains");
        board.press(COL_HIGH_LOW).unwrap();
        assert_eq!(board.direction(), "row and column both at ground");
        board.press(ROW_HIGH).unwrap();
        assert_eq!(board.direction(), "row feeds, column drains");
        board.press(COL_HIGH_LOW).unwrap();
        assert_eq!(board.direction(), "row and column both at the supply");
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
