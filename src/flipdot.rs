//! `debug flipdot`: render UTF-8 text as dots, the way the flipdot panel will
//! show it.
//!
//! [`DisplayArgs`] describes the display and how text is set on it, for
//! this command and for `run`. The text is laid out by [`Scene`], and every
//! frame reaches the terminal preview through the same [`Display`] as the
//! hardware driver, so both show exactly the same dots.

use std::io::{BufRead, IsTerminal};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use tokio::signal::unix::{SignalKind, signal};

use crate::board::MAX_COLUMNS;
use crate::display::{Display, Note, Stacking, Terminal};
use crate::panel::{Panel, PanelArgs};
use crate::scene::{Align, Font, Scene, Scroll};

/// Size of one real panel in dots: as wide as the board can address, two
/// rows short of it.
pub const PANEL_WIDTH: u32 = MAX_COLUMNS;
pub const PANEL_HEIGHT: u32 = 14;
/// Number of panels the display is built from.
pub const PANELS: u32 = 2;
/// How often a wait for the next text looks whether the program should end.
const STOP_POLL: Duration = Duration::from_millis(50);

#[derive(clap::Args, Debug)]
pub struct FlipdotArgs {
    /// Text to show. Each argument is one line; `\n` inside an argument also
    /// starts a new line. Without any text, lines are read from stdin and
    /// each one is rendered as its own frame.
    pub text: Vec<String>,

    #[command(flatten)]
    pub display: DisplayArgs,
}

/// The display and how text is set on it. Shared by `debug flipdot` and
/// `run`.
#[derive(clap::Args, Debug)]
pub struct DisplayArgs {
    /// Font, named by the size of one character cell in dots.
    #[arg(long, value_enum, default_value_t = Font::F5x7)]
    pub font: Font,

    /// Horizontal alignment of every line.
    #[arg(long, value_enum, default_value_t = Align::Left)]
    pub align: Align,

    /// Number of panels the display is built from.
    #[arg(long, default_value_t = PANELS, value_parser = clap::value_parser!(u32).range(1..=16))]
    pub panels: u32,

    /// How the panels are mounted.
    #[arg(long, value_enum, default_value_t = Layout::Row)]
    pub layout: Layout,

    /// Width of one panel in dots.
    #[arg(long, default_value_t = PANEL_WIDTH, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub width: u32,

    /// Height of one panel in dots.
    #[arg(long, default_value_t = PANEL_HEIGHT, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub height: u32,

    /// Which lines scroll.
    #[arg(long, value_enum, default_value_t = Scroll::Auto)]
    pub scroll: Scroll,

    /// Scroll speed, in dots per second.
    #[arg(long, value_name = "DOTS_PER_SECOND", default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=60))]
    pub speed: u32,

    #[command(flatten)]
    pub panel: PanelArgs,
}

impl DisplayArgs {
    /// Size of the whole display in dots.
    pub fn display_size(&self) -> (u32, u32) {
        match self.layout {
            Layout::Row => (self.width * self.panels, self.height),
            Layout::Column => (self.width, self.height * self.panels),
        }
    }

    /// Time between two frames of a scrolling line.
    pub fn frame_time(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.speed))
    }

    /// Fail if the font cannot fit on the display at all.
    pub fn check(&self) -> Result<()> {
        let (cell_width, cell_height) = self.font.cell_size();
        let (_, height) = self.display_size();
        if cell_height > height {
            bail!("font {cell_width}x{cell_height} is taller than the {height} dot high display");
        }
        Ok(())
    }

    /// The display with its outputs: the preview in the terminal and, with
    /// `--gpio`, the panels. Opening the panels gives the calling thread
    /// real-time priority, which threads started afterwards inherit.
    pub fn open(&self, stacking: Stacking) -> Result<Display> {
        let (width, height) = self.display_size();
        let mut display = Display::new(width, height);
        // The panels first: the terminal then shows only what they did.
        if self.panel.gpio {
            display.attach(Panel::open(self)?);
        }
        display.attach(Terminal::stdout(width, height, stacking));
        Ok(display)
    }

    /// Lay out `lines` for this display.
    pub fn scene<S: AsRef<str>>(&self, lines: &[S]) -> Scene {
        let refs: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();
        let (width, height) = self.display_size();
        // Rest for one second before each pass so the start is readable.
        Scene::build(&refs, self.font, self.align, width, height, self.scroll).with_hold(self.speed)
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Panels side by side: a wide display.
    Row,
    /// Panels stacked on top of each other: a tall display.
    Column,
}

/// Split arguments into display lines. `\n`, typed literally or as a real
/// newline, starts a new line.
fn split_lines(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|arg| {
            arg.replace("\\n", "\n")
                .split('\n')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A flag that is set when the program is asked to end. From here on the
/// signals no longer end it by themselves, which could happen in the middle
/// of a pulse and leave a coil of the panel energised.
fn stop_on_signals() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    for kind in [
        SignalKind::interrupt(),
        SignalKind::terminate(),
        SignalKind::hangup(),
    ] {
        let mut signal = signal(kind).context("cannot listen for signals")?;
        let stop = stop.clone();
        tokio::spawn(async move {
            signal.recv().await;
            stop.store(true, Ordering::Relaxed);
        });
    }
    Ok(stop)
}

/// What a wait for the next text ended with.
enum Next {
    Text(Vec<String>),
    /// The deadline has come.
    Due,
    /// The program should end, or no text can come and nothing is due.
    End,
}

/// The texts to show, one after another.
struct Input {
    texts: Receiver<Vec<String>>,
    /// More texts may come.
    open: bool,
    stop: Arc<AtomicBool>,
}

impl Input {
    /// Wait for the next text, until `deadline` if there is one.
    fn next(&mut self, deadline: Option<Instant>) -> Next {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Next::End;
            }
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if left == Some(Duration::ZERO) {
                return Next::Due;
            }
            let step = left.map_or(STOP_POLL, |left| left.min(STOP_POLL));
            if !self.open {
                if deadline.is_none() {
                    return Next::End;
                }
                std::thread::sleep(step);
                continue;
            }
            match self.texts.recv_timeout(step) {
                Ok(lines) => return Next::Text(lines),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.open = false,
            }
        }
    }
}

pub fn run(args: FlipdotArgs) -> Result<()> {
    args.display.check()?;
    // Animation needs a terminal to redraw in place, or the panels.
    let animate = std::io::stdout().is_terminal() || args.display.panel.gpio;
    let frame_time = args.display.frame_time();
    let stop = stop_on_signals()?;

    // Texts to show, one after another. Stdin is read on its own thread so a
    // scrolling text keeps moving while waiting for the next line.
    let (tx, rx) = mpsc::channel::<Vec<String>>();
    if args.text.is_empty() {
        let stdin = std::io::stdin();
        if stdin.is_terminal() {
            eprintln!(
                "adsb-flipdot: type text and press Enter, use \\n for a second line, Ctrl-D to quit"
            );
        }
        std::thread::spawn(move || {
            for line in stdin.lock().lines().map_while(Result::ok) {
                if tx.send(split_lines(&[line])).is_err() {
                    break;
                }
            }
        });
    } else {
        tx.send(split_lines(&args.text))?;
        // Closing the channel marks the end of input.
        drop(tx);
    }

    let mut input = Input {
        texts: rx,
        open: true,
        stop,
    };
    let mut display = args.display.open(Stacking::Blocks)?;
    let new_text = Note {
        caption: None,
        new_text: true,
    };
    let mut next = input.next(None);
    while let Next::Text(lines) = next {
        let scene = args.display.scene(&lines);
        // Before the frame: a line below it would end up inside the redraw.
        for warning in scene.warnings() {
            eprintln!("adsb-flipdot: {warning}");
        }
        display.show(&scene.frame(0), new_text)?;

        if !scene.scrolls() {
            next = input.next(None);
            continue;
        }
        if !animate {
            eprintln!(
                "adsb-flipdot: text is wider than the display; it scrolls when run in a terminal \
                 or with --gpio"
            );
            next = input.next(None);
            continue;
        }

        // Scroll until the next text arrives. With text given as arguments
        // there is no next text, so this runs until Ctrl-C.
        let mut tick: u64 = 0;
        let mut due = Instant::now() + frame_time;
        next = loop {
            match input.next(Some(due)) {
                Next::Due => {}
                other => break other,
            }
            // End of stdin: stop there. Arguments: keep scrolling.
            if !input.open && args.text.is_empty() {
                break Next::End;
            }
            tick += 1;
            display.show(&scene.frame(tick), Note::default())?;
            // A frame that took too long is not caught up on.
            due = (due + frame_time).max(Instant::now());
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::parse_args;

    fn args(extra: &[&str]) -> DisplayArgs {
        parse_args(extra)
    }

    #[test]
    fn arguments_and_escaped_newlines_become_lines() {
        let args = vec![
            "SAS51B".to_string(),
            "a\\nb".to_string(),
            "c\nd".to_string(),
        ];
        assert_eq!(split_lines(&args), ["SAS51B", "a", "b", "c", "d"]);
    }

    #[test]
    fn display_size_follows_panels_and_layout() {
        assert_eq!(
            args(&[]).display_size(),
            (56, 14),
            "default: 2 panels in a row"
        );
        assert_eq!(args(&["--panels", "1"]).display_size(), (28, 14));
        assert_eq!(args(&["--panels", "4"]).display_size(), (112, 14));
        assert_eq!(args(&["--layout", "column"]).display_size(), (28, 28));
        assert_eq!(
            args(&[
                "--panels", "3", "--layout", "column", "--width", "20", "--height", "10"
            ])
            .display_size(),
            (20, 30)
        );
    }

    #[test]
    fn a_font_taller_than_the_display_is_refused() {
        assert!(args(&[]).check().is_ok());
        assert!(args(&["--font", "7x14", "--height", "14"]).check().is_ok());
        let error = args(&["--font", "7x14", "--height", "10"])
            .check()
            .unwrap_err()
            .to_string();
        assert!(error.contains("7x14 is taller than the 10 dot"), "{error}");
    }
}
