//! The display drawn as text in the terminal.

use std::io::{IsTerminal, Write};

use anyhow::Result;

use crate::domain::dots::{Flip, FrameBuffer};
use crate::output::{Note, Output};

/// Where the terminal puts a frame with new text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stacking {
    /// Over the previous frame: one display that changes.
    InPlace,
    /// Below the previous frame, so that earlier texts stay readable.
    Blocks,
}

/// The dots as text: two terminal columns per dot, so dots come out round
/// and roughly square like on the real panel. The dots in `flipped` stand
/// out from the ones that stayed as they were.
fn dots_text(dots: &FrameBuffer, color: bool, flipped: &[Flip]) -> String {
    let (width, height) = (dots.width(), dots.height());
    let mut marked = vec![false; (width * height) as usize];
    for flip in flipped {
        if flip.x < width && flip.y < height {
            marked[(flip.y * width + flip.x) as usize] = true;
        }
    }
    // Yellow for a flipped dot, dim grey for the black side. A dot that
    // just came on is white, one that just went off a red ring.
    let (on, off, came_on, went_off, reset) = if color {
        (
            "\x1b[93m●",
            "\x1b[90m·",
            "\x1b[97m●",
            "\x1b[91m○",
            "\x1b[0m",
        )
    } else {
        ("●", "·", "◉", "○", "")
    };
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            if x > 0 {
                out.push(' ');
            }
            let marked = marked[(y * width + x) as usize];
            out.push_str(match (dots.get(x, y), marked) {
                (true, false) => on,
                (false, false) => off,
                (true, true) => came_on,
                (false, true) => went_off,
            });
        }
        out.push_str(reset);
        out.push('\n');
    }
    out
}

/// What the terminal drew last.
struct Drawn {
    /// Lines to go back up to draw over it.
    lines: u32,
    caption: Option<String>,
    /// Some dots stand out as flipped.
    marked: bool,
}

/// The display drawn as text. It keeps its own dots and changes them only
/// by the flips it is given.
///
/// On a terminal the frame is redrawn in place, and the dots flipped for
/// it stand out: white for a dot that came on, a red ring for one that
/// went off. The line below counts them. Piped or logged, a plain frame is
/// written for every new text and nothing for a scrolling line moving on.
pub struct Terminal<W: Write> {
    out: W,
    tty: bool,
    stacking: Stacking,
    dots: FrameBuffer,
    /// Flips since the last frame was drawn.
    flipped: Vec<Flip>,
    drawn: Option<Drawn>,
}

impl Terminal<std::io::Stdout> {
    pub fn stdout(width: u32, height: u32, stacking: Stacking) -> Self {
        let out = std::io::stdout();
        let tty = out.is_terminal();
        Self::new(out, tty, width, height, stacking)
    }
}

impl<W: Write> Terminal<W> {
    pub fn new(out: W, tty: bool, width: u32, height: u32, stacking: Stacking) -> Self {
        Self {
            out,
            tty,
            stacking,
            dots: FrameBuffer::new(width, height),
            flipped: Vec::new(),
            drawn: None,
        }
    }

    /// The frame as text, or `None` if there is nothing new to draw.
    fn render(&mut self, note: &Note) -> Option<String> {
        let mut text = String::new();
        let mut lines = self.dots.height();
        if self.tty {
            let stack = self.stacking == Stacking::Blocks && note.new_text;
            match &self.drawn {
                Some(_) if stack => text.push('\n'),
                Some(drawn) => {
                    // A frame with marked dots is drawn again to settle them.
                    if self.flipped.is_empty()
                        && !drawn.marked
                        && drawn.caption.as_deref() == note.caption
                    {
                        return None;
                    }
                    text.push_str(&format!("\x1b[{}A", drawn.lines));
                }
                None => {}
            }
            text.push_str(&dots_text(&self.dots, true, &self.flipped));
            text.push_str(&format!("{:>4} flipped", self.flipped.len()));
            if let Some(caption) = note.caption {
                text.push_str("  ");
                text.push_str(caption);
            }
            // Clear what is left of a longer previous line.
            text.push_str("\x1b[K\n");
            lines += 1;
        } else {
            if !note.new_text {
                return None;
            }
            if self.drawn.is_some() {
                text.push('\n');
            }
            text.push_str(&dots_text(&self.dots, false, &[]));
            if let Some(caption) = note.caption {
                text.push_str(caption);
                text.push('\n');
            }
        }
        self.drawn = Some(Drawn {
            lines,
            caption: note.caption.map(str::to_string),
            marked: self.tty && !self.flipped.is_empty(),
        });
        Some(text)
    }
}

impl<W: Write> Output for Terminal<W> {
    fn flip(&mut self, flip: Flip) -> Result<()> {
        self.dots.set(flip.x, flip.y, flip.on);
        self.flipped.push(flip);
        Ok(())
    }

    fn flush(&mut self, note: &Note) -> Result<()> {
        let text = self.render(note);
        self.flipped.clear();
        if let Some(text) = text {
            // A closed pipe must not stop the display.
            let _ = self.out.write_all(text.as_bytes());
            let _ = self.out.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scene::{Align, Font, Scene};

    #[test]
    fn dots_text_has_one_row_per_dot_row() {
        let fb = Scene::new(&["A"], Font::F5x7, Align::Left, 28, 14).frame(0);
        let text = dots_text(&fb, false, &[]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 14);
        // 28 dots separated by single spaces.
        assert!(lines.iter().all(|l| l.chars().count() == 55), "{text}");
        assert!(text.contains('●') && text.contains('·'));
        assert!(!text.contains('\x1b'), "no colour codes when disabled");
        assert!(dots_text(&fb, true, &[]).contains("\x1b[93m●"));
    }

    /// A terminal output that writes into a string.
    fn terminal(tty: bool, stacking: Stacking) -> Terminal<Vec<u8>> {
        Terminal::new(Vec::new(), tty, 3, 2, stacking)
    }

    /// What was written since the last call.
    fn written(terminal: &mut Terminal<Vec<u8>>) -> String {
        String::from_utf8(std::mem::take(&mut terminal.out)).unwrap()
    }

    fn flip(terminal: &mut Terminal<Vec<u8>>, x: u32, y: u32, on: bool) {
        terminal.flip(Flip { x, y, on }).unwrap();
    }

    const NEW_TEXT: Note = Note {
        caption: None,
        new_text: true,
    };

    #[test]
    fn terminal_marks_flipped_dots_and_settles_them_on_the_next_frame() {
        let mut t = terminal(true, Stacking::InPlace);
        flip(&mut t, 0, 0, true);
        flip(&mut t, 1, 0, false);
        t.flush(&NEW_TEXT).unwrap();
        let first = written(&mut t);
        assert!(
            !first.contains("\x1b[2A"),
            "nothing to draw over: {first:?}"
        );
        assert!(first.contains("\x1b[97m●"), "came on: {first:?}");
        assert!(first.contains("\x1b[91m○"), "went off: {first:?}");
        assert!(first.contains("   2 flipped"), "{first:?}");

        // Nothing flips: the same dots, drawn over the frame and its count.
        t.flush(&Note::default()).unwrap();
        let second = written(&mut t);
        assert!(second.starts_with("\x1b[3A"), "{second:?}");
        assert!(second.contains("\x1b[93m●"), "settled on: {second:?}");
        assert!(!second.contains("\x1b[97m") && !second.contains("\x1b[91m"));
        assert!(second.contains("   0 flipped"), "{second:?}");

        // Now there is nothing left to change on the screen.
        t.flush(&Note::default()).unwrap();
        assert_eq!(written(&mut t), "");
    }

    #[test]
    fn terminal_redraws_for_a_new_caption() {
        let mut t = terminal(true, Stacking::InPlace);
        let note = |caption| Note {
            caption: Some(caption),
            new_text: false,
        };
        t.flush(&note("one")).unwrap();
        assert!(written(&mut t).contains("   0 flipped  one\x1b[K\n"));
        t.flush(&note("one")).unwrap();
        assert_eq!(written(&mut t), "");
        t.flush(&note("two")).unwrap();
        assert!(written(&mut t).contains("  two\x1b[K\n"));
    }

    #[test]
    fn terminal_in_blocks_puts_new_text_below_the_previous() {
        let mut t = terminal(true, Stacking::Blocks);
        t.flush(&NEW_TEXT).unwrap();
        written(&mut t);
        flip(&mut t, 0, 0, true);
        t.flush(&NEW_TEXT).unwrap();
        let block = written(&mut t);
        assert!(
            block.starts_with("\n\x1b[9"),
            "blank line, no jump: {block:?}"
        );
        // The next step of a scrolling line replaces that block.
        flip(&mut t, 0, 0, false);
        t.flush(&Note::default()).unwrap();
        assert!(written(&mut t).starts_with("\x1b[3A"));
    }

    #[test]
    fn piped_output_is_one_plain_frame_per_new_text() {
        let mut t = terminal(false, Stacking::InPlace);
        flip(&mut t, 0, 0, true);
        t.flush(&Note {
            caption: Some("status"),
            new_text: true,
        })
        .unwrap();
        assert_eq!(written(&mut t), "● · ·\n· · ·\nstatus\n");

        // A scrolling line moving on is not written, but not lost either.
        flip(&mut t, 2, 1, true);
        t.flush(&Note::default()).unwrap();
        assert_eq!(written(&mut t), "");
        t.flush(&NEW_TEXT).unwrap();
        assert_eq!(written(&mut t), "\n● · ·\n· · ●\n");
    }
}
