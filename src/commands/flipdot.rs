//! `debug flipdot`: render UTF-8 text as dots, the way the flipdot panel will
//! show it.
//!
//! The text is laid out as a [`Scene`](crate::domain::scene::Scene), and
//! every frame reaches the terminal preview through the same
//! [`Display`](crate::output::Display) as the panels, so both show exactly
//! the same dots. The panels are driven too, unless `--dry-run`.

use std::io::{BufRead, IsTerminal};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::cli::FlipdotArgs;
use crate::commands::shutdown::Shutdown;
use crate::output::Note;
use crate::output::terminal::Stacking;

/// How often a wait for the next text looks whether the program should end.
const STOP_POLL: Duration = Duration::from_millis(50);

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
    let mut shutdown = Shutdown::listen()?;
    let asked = stop.clone();
    tokio::spawn(async move {
        shutdown.recv().await;
        asked.store(true, Ordering::Relaxed);
    });
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
    args.display.check(args.font)?;
    let panels = !args.dry_run;
    // Animation needs a terminal to redraw in place, or the panels.
    let animate = std::io::stdout().is_terminal() || panels;
    let frame_time = args.display.frame_time();
    let typesetting = args.display.typesetting(args.font);
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
    let mut display = args
        .display
        .open(Stacking::Blocks, panels)
        .context("cannot drive the panels (--dry-run only shows the dots in the terminal)")?;
    let new_text = Note {
        caption: None,
        new_text: true,
    };
    let mut next = input.next(None);
    while let Next::Text(lines) = next {
        let scene = typesetting.scene(&lines);
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
                 or without --dry-run"
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

    #[test]
    fn arguments_and_escaped_newlines_become_lines() {
        let args = vec![
            "SAS51B".to_string(),
            "a\\nb".to_string(),
            "c\nd".to_string(),
        ];
        assert_eq!(split_lines(&args), ["SAS51B", "a", "b", "c", "d"]);
    }
}
