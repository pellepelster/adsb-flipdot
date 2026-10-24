//! The adapter board between the Pi and the panels: its signals, the pins
//! behind them and the rules for driving them.
//!
//! The address and direction lines are set to a level; the COL-SEL lines,
//! which let the coil current flow, can only be pulsed. [`Board`] holds
//! those rules and is independent of the terminal and of real pins. It is
//! shared by the panel output and `debug pi`.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gpiocdev::line::Value;

/// Real-time priority of the thread that times the pulses, 1 to 99.
const REALTIME_PRIORITY: libc::c_int = 50;
/// A pulse this many times longer than asked for counts as an overrun.
pub(crate) const OVERRUN_FACTOR: u32 = 2;

/// How to reach the adapter board. Shared by the panel output and
/// `debug pi`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardConfig {
    /// GPIO character device of the Pi's header pins.
    pub chip: PathBuf,
    /// Length of the COL-SEL pulse that flips one dot.
    pub pulse: Duration,
}

impl BoardConfig {
    /// Take the pins of the board. The calling thread gets real-time
    /// priority where the system allows it, because it is the one that
    /// times the pulses. Returns the board and, in words, the priority.
    pub(crate) fn open(&self) -> Result<(Board<GpioPins>, &'static str)> {
        let pins = GpioPins::open(&self.chip)?;
        let priority = if enable_realtime() {
            "real-time priority"
        } else {
            "normal priority, pulses may run long (run as root for real-time priority)"
        };
        Ok((Board::new(pins, self.pulse)?, priority))
    }
}

/// How a signal is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Stays at the level it is set to.
    Level,
    /// Goes high for one pulse and returns low by itself.
    Pulse,
}

/// One signal of the adapter board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Signal {
    pub(crate) name: &'static str,
    /// BCM number of the GPIO.
    pub(crate) gpio: u32,
    /// Pin on the Pi's 40-pin header.
    pub(crate) header_pin: u8,
    /// Pin on the panel's 26-pin header.
    pub(crate) panel_pin: u8,
    pub(crate) kind: Kind,
    pub(crate) meaning: &'static str,
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
pub(crate) const SIGNALS: [Signal; 18] = [
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
pub(crate) const ROW_ADDRESS: Range<usize> = 0..5;
pub(crate) const ROW_HIGH: usize = 5;
pub(crate) const ROW_LOW: usize = 6;
pub(crate) const COL_ADDRESS: Range<usize> = 7..12;
pub(crate) const COL_HIGH_LOW: usize = 12;
const COL_SELECT: Range<usize> = 13..18;

/// Rows a panel can have at most.
pub(crate) const MAX_ROWS: u32 = 16;
/// Columns a panel can have at most: four decoder chips of seven lines.
pub(crate) const MAX_COLUMNS: u32 = 28;
/// Panels the board can select.
pub(crate) const MAX_PANELS: usize = COL_SELECT.end - COL_SELECT.start;
/// Rest between two pulses.
const PULSE_GAP: Duration = Duration::from_micros(10);

/// The pins behind the signals.
pub(crate) trait Pins {
    fn set(&mut self, gpio: u32, high: bool) -> Result<()>;
}

/// The Pi's header pins, through the kernel's GPIO character device.
pub(crate) struct GpioPins(gpiocdev::Request);

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
                     use GPIO, is another program holding the pins?)",
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

/// Pins for a dry run: every level is accepted and nothing happens.
pub(crate) struct NoPins;

impl Pins for NoPins {
    fn set(&mut self, _gpio: u32, _high: bool) -> Result<()> {
        Ok(())
    }
}

/// The levels of all signals and the rules for changing them.
///
/// Every line is driven low when the board is dropped, so that no coil stays
/// energised however the program ends.
pub(crate) struct Board<P: Pins> {
    pins: P,
    pub(crate) levels: [bool; SIGNALS.len()],
    /// Length asked for.
    pub(crate) pulse: Duration,
    pub(crate) pulses: u64,
    /// Measured length of the latest pulse.
    pub(crate) last_pulse: Option<Duration>,
    pub(crate) longest_pulse: Option<Duration>,
    pub(crate) overruns: u64,
}

impl<P: Pins> Board<P> {
    /// Start with every line low.
    pub(crate) fn new(pins: P, pulse: Duration) -> Result<Self> {
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

    /// What a key press does to a signal: toggle a level, fire a pulse.
    pub(crate) fn press(&mut self, index: usize) -> Result<()> {
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
    fn pulse(&mut self, index: usize) -> Result<()> {
        let gpio = SIGNALS[index].gpio;
        if let Err(error) = self.pins.set(gpio, true) {
            // The line may be high all the same.
            let _ = self.pins.set(gpio, false);
            return Err(error);
        }
        let raised = Instant::now();
        spin_until(raised + self.pulse);
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

    /// Set a line, unless it is at that level already.
    fn put(&mut self, index: usize, high: bool) -> Result<()> {
        if self.levels[index] == high {
            return Ok(());
        }
        self.set(index, high)
    }

    /// Put `address` on a group of address lines.
    fn put_address(&mut self, bits: Range<usize>, address: u8) -> Result<()> {
        for (bit, index) in bits.enumerate() {
            self.put(index, address >> bit & 1 == 1)?;
        }
        Ok(())
    }

    /// Flip one dot: put its addresses and the direction on the lines, then
    /// pulse the COL-SEL line of its panel. `select` counts the COL-SEL
    /// lines from 0. Every line is driven low if this fails.
    pub(crate) fn flip(&mut self, select: usize, row: u8, column: u8, on: bool) -> Result<()> {
        let result = self.try_flip(select, row, column, on);
        if result.is_err() {
            let _ = self.all_low();
        }
        result
    }

    fn try_flip(&mut self, select: usize, row: u8, column: u8, on: bool) -> Result<()> {
        let Some(select) = COL_SELECT.clone().nth(select) else {
            anyhow::bail!("the board has no COL-SEL{select}");
        };
        // A dot comes on with the column feeding and the row draining.
        let (active, other) = if on {
            (ROW_LOW, ROW_HIGH)
        } else {
            (ROW_HIGH, ROW_LOW)
        };
        // ROW-HIGH and ROW-LOW together short the coil supply, so the other
        // one goes low first.
        self.put(other, false)?;
        self.put(active, true)?;
        self.put(COL_HIGH_LOW, !on)?;
        self.put_address(ROW_ADDRESS, row)?;
        self.put_address(COL_ADDRESS, column)?;
        self.pulse(select)?;
        spin_until(Instant::now() + PULSE_GAP);
        Ok(())
    }

    /// Stop driving the rows, for the time between two frames.
    pub(crate) fn rest(&mut self) -> Result<()> {
        self.put(ROW_HIGH, false)?;
        self.put(ROW_LOW, false)
    }

    /// The pulses sent so far and how well their length was kept.
    pub(crate) fn pulse_report(&self) -> String {
        format!(
            "{} pulses sent, longest {}, {} overruns",
            self.pulses,
            micros(self.longest_pulse),
            self.overruns
        )
    }

    /// Drive every line low, the COL-SEL lines first. Goes on after a
    /// failure and returns the first one.
    pub(crate) fn all_low(&mut self) -> Result<()> {
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
    pub(crate) fn address(&self, bits: Range<usize>) -> u8 {
        self.levels[bits]
            .iter()
            .enumerate()
            .map(|(bit, high)| u8::from(*high) << bit)
            .sum()
    }

    /// Where the current would flow during a COL-SEL pulse.
    pub(crate) fn direction(&self) -> &'static str {
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
pub(crate) fn line_number(address: u8) -> Option<u8> {
    let (chip, line) = (address >> 3, address & 7);
    (line != 0).then_some(chip * 7 + line)
}

/// The address of a row or column, counted from 0. The inverse of
/// [`line_number`].
pub(crate) fn address(line: u32) -> u8 {
    (line / 7 * 8 + line % 7 + 1) as u8
}

/// A duration in microseconds, `-` for none.
pub(crate) fn micros(duration: Option<Duration>) -> String {
    match duration {
        Some(duration) => format!("{} µs", duration.as_micros()),
        None => "-".to_string(),
    }
}

/// Wait for a point in time that is microseconds away. A busy loop: a sleep
/// hands the CPU to the scheduler, which may return it much later.
fn spin_until(deadline: Instant) {
    while Instant::now() < deadline {
        std::hint::spin_loop();
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

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// Every level written, in order, as (GPIO, high).
    pub(crate) type Log = Rc<RefCell<Vec<(u32, bool)>>>;

    /// Pins that record what is written to them. Writing to the GPIO in
    /// `broken` fails.
    pub(crate) struct FakePins {
        pub(crate) log: Log,
        pub(crate) broken: Option<u32>,
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

    pub(crate) const PULSE: Duration = Duration::from_micros(50);

    /// A board on fake pins, with the writes of its start left out of the log.
    pub(crate) fn board() -> (Board<FakePins>, Log) {
        let log = Log::default();
        let pins = FakePins {
            log: log.clone(),
            broken: None,
        };
        let board = Board::new(pins, PULSE).unwrap();
        log.borrow_mut().clear();
        (board, log)
    }

    /// Position of the signal called `name` in [`SIGNALS`].
    pub(crate) fn index(name: &str) -> usize {
        SIGNALS.iter().position(|s| s.name == name).unwrap()
    }

    fn gpio(name: &str) -> u32 {
        SIGNALS[index(name)].gpio
    }

    /// The signals in `pcb/build_board.py` as (name, Pi pin, panel pin).
    fn board_script_signals() -> Vec<(String, u8, u8)> {
        include_str!("../../pcb/build_board.py")
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

    /// The names of the lines written, with their levels.
    pub(crate) fn named(log: &Log) -> Vec<(&'static str, bool)> {
        let name = |gpio| SIGNALS.iter().find(|s| s.gpio == gpio).unwrap().name;
        log.borrow()
            .iter()
            .map(|(gpio, high)| (name(*gpio), *high))
            .collect()
    }

    #[test]
    fn flip_sets_the_lines_of_the_documented_example_and_pulses() {
        // "Dot 1/1 on" in docs/mobitec/README.md.
        let (mut board, log) = board();
        board.flip(0, 1, 1, true).unwrap();
        assert_eq!(
            named(&log),
            [
                ("ROW-LOW", true),
                ("ROW0", true),
                ("COL0", true),
                ("COL-SEL0", true),
                ("COL-SEL0", false)
            ]
        );
        assert_eq!(board.pulses, 1);

        // "Dot 1/1 off": the same addresses, both directions turned round.
        log.borrow_mut().clear();
        board.flip(0, 1, 1, false).unwrap();
        assert_eq!(
            named(&log),
            [
                ("ROW-LOW", false),
                ("ROW-HIGH", true),
                ("COL-H/L", true),
                ("COL-SEL0", true),
                ("COL-SEL0", false)
            ]
        );
    }

    #[test]
    fn flip_writes_only_the_lines_that_change() {
        let (mut board, log) = board();
        board.flip(1, 0b01101, 0b10010, true).unwrap();
        assert_eq!(board.address(ROW_ADDRESS), 0b01101);
        assert_eq!(board.address(COL_ADDRESS), 0b10010);
        log.borrow_mut().clear();
        // The next row of the same column: one address bit and the pulse.
        board.flip(1, 0b01111, 0b10010, true).unwrap();
        assert_eq!(
            named(&log),
            [("ROW1", true), ("COL-SEL1", true), ("COL-SEL1", false)]
        );
    }

    #[test]
    fn flip_never_has_row_high_and_row_low_high_together() {
        let (mut board, log) = board();
        for on in [true, false, false, true, true, false] {
            board.flip(0, 3, 5, on).unwrap();
        }
        let (mut high, mut low) = (false, false);
        for (name, level) in named(&log) {
            match name {
                "ROW-HIGH" => high = level,
                "ROW-LOW" => low = level,
                _ => {}
            }
            assert!(!(high && low), "both high");
        }
    }

    #[test]
    fn rest_stops_driving_the_rows() {
        let (mut board, log) = board();
        board.flip(0, 1, 1, false).unwrap();
        log.borrow_mut().clear();
        board.rest().unwrap();
        board.rest().unwrap();
        assert_eq!(named(&log), [("ROW-HIGH", false)]);
    }

    #[test]
    fn a_failed_flip_drives_every_line_low() {
        let (mut board, _log) = board();
        board.flip(0, 1, 1, true).unwrap();
        board.pins.broken = Some(gpio("COL1"));
        assert!(board.flip(0, 1, 2, true).is_err());
        assert!(board.levels.iter().all(|high| !high));
        assert_eq!(board.pulses, 1, "no pulse for the failed flip");
        board.pins.broken = None;
        assert!(board.flip(9, 1, 1, true).is_err(), "no such COL-SEL line");
    }

    #[test]
    fn press_toggles_a_level_line() {
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
        let (mut board, log) = board();
        board.pins.broken = Some(gpio("COL-SEL0"));

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
}
