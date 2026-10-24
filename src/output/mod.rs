//! Where the dots are shown: the preview in the terminal and the flipdot
//! panels.
//!
//! [`Display`] sends the flips that a new frame needs to every attached
//! [`Output`]. All of them get the same flips, so the terminal shows
//! exactly what a panel is told to do, including which dots move.

pub mod board;
pub mod panel;
pub mod terminal;

use anyhow::Result;

use crate::domain::dots::{Flip, FrameBuffer, Shown};
use crate::output::panel::{Arrangement, Panel, PanelConfig};
use crate::output::terminal::{Stacking, Terminal};

/// What a frame is about, for outputs that show more than dots.
#[derive(Debug, Clone, Copy, Default)]
pub struct Note<'a> {
    /// A line of plain text that goes with the frame.
    pub caption: Option<&'a str>,
    /// The frame shows new text, as opposed to a scrolling line merely
    /// moving on.
    pub new_text: bool,
}

/// Something that shows the dots: the terminal, a panel.
pub trait Output {
    /// Flip one dot. Only called for dots that change.
    fn flip(&mut self, flip: Flip) -> Result<()>;

    /// All flips of one frame are made.
    fn flush(&mut self, _note: &Note) -> Result<()> {
        Ok(())
    }
}

/// The outputs, and what they show.
pub struct Display {
    width: u32,
    height: u32,
    shown: Shown,
    outputs: Vec<Box<dyn Output>>,
}

impl Display {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            shown: Shown::new(width, height),
            outputs: Vec::new(),
        }
    }

    /// The display with its outputs: the preview in the terminal and, if
    /// `panels` says how they are wired, the panels. Opening the panels
    /// gives the calling thread real-time priority, which threads started
    /// afterwards inherit.
    pub fn open(
        arrangement: Arrangement,
        panels: Option<&PanelConfig>,
        stacking: Stacking,
    ) -> Result<Self> {
        let (width, height) = arrangement.display_size();
        let mut display = Self::new(width, height);
        // The panels first: the terminal then shows only what they did.
        if let Some(config) = panels {
            display.attach(Panel::open(arrangement, config)?);
        }
        display.attach(Terminal::stdout(width, height, stacking));
        Ok(display)
    }

    /// Add an output. It gets every flip from now on.
    pub fn attach(&mut self, output: impl Output + 'static) {
        self.outputs.push(Box::new(output));
    }

    /// Turn every dot to its dark side, whatever it shows now: each dot
    /// gets a flip, also those known to be dark already.
    pub fn wipe(&mut self) -> Result<usize> {
        self.shown = Shown::new(self.width, self.height);
        self.show(&FrameBuffer::new(self.width, self.height), Note::default())
    }

    /// Bring the dots to `target`. Returns the number of dots flipped.
    ///
    /// A dot counts as flipped as soon as every output has done so, so the
    /// record stays right when an output fails half way.
    pub fn show(&mut self, target: &FrameBuffer, note: Note) -> Result<usize> {
        let flips = self.shown.diff(target);
        for flip in &flips {
            for output in &mut self.outputs {
                output.flip(*flip)?;
            }
            self.shown.record(*flip);
        }
        for output in &mut self.outputs {
            output.flush(&note)?;
        }
        Ok(flips.len())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::domain::dots::tests::frame;

    /// An output that records the flips it is given.
    #[derive(Clone, Default)]
    struct Recorder(Rc<RefCell<Vec<Flip>>>);

    impl Recorder {
        fn take(&self) -> Vec<Flip> {
            self.0.borrow_mut().drain(..).collect()
        }
    }

    impl Output for Recorder {
        fn flip(&mut self, flip: Flip) -> Result<()> {
            self.0.borrow_mut().push(flip);
            Ok(())
        }
    }

    /// An output that fails from its `good + 1`th flip on.
    struct Failing {
        good: usize,
    }

    impl Output for Failing {
        fn flip(&mut self, _flip: Flip) -> Result<()> {
            if self.good == 0 {
                anyhow::bail!("no more flips");
            }
            self.good -= 1;
            Ok(())
        }
    }

    fn recorded(width: u32, height: u32) -> (Display, Recorder) {
        let recorder = Recorder::default();
        let mut display = Display::new(width, height);
        display.attach(recorder.clone());
        (display, recorder)
    }

    #[test]
    fn an_output_gets_only_the_flips_a_frame_needs() {
        let (mut display, recorder) = recorded(3, 2);
        let fb = frame(3, 2, &[(1, 0)]);
        assert_eq!(display.show(&fb, Note::default()).unwrap(), 6);
        assert_eq!(recorder.take().len(), 6, "first frame: every dot");

        assert_eq!(display.show(&fb, Note::default()).unwrap(), 0);
        assert!(recorder.take().is_empty(), "same frame: nothing");

        let next = frame(3, 2, &[(2, 1)]);
        assert_eq!(display.show(&next, Note::default()).unwrap(), 2);
        assert_eq!(recorder.take().len(), 2);
    }

    #[test]
    fn every_output_gets_the_same_flips() {
        let (mut display, first) = recorded(4, 4);
        let second = Recorder::default();
        display.attach(second.clone());
        display
            .show(&frame(4, 4, &[(3, 3)]), Note::default())
            .unwrap();
        let flips = first.take();
        assert_eq!(flips.len(), 16);
        assert_eq!(flips, second.take());
    }

    #[test]
    fn wipe_flips_every_dot_dark() {
        let (mut display, recorder) = recorded(3, 2);
        display
            .show(&frame(3, 2, &[(1, 0)]), Note::default())
            .unwrap();
        recorder.take();
        assert_eq!(display.wipe().unwrap(), 6);
        let flips = recorder.take();
        assert_eq!(flips.len(), 6);
        assert!(flips.iter().all(|flip| !flip.on));
        assert_eq!(display.show(&frame(3, 2, &[]), Note::default()).unwrap(), 0);
    }

    #[test]
    fn a_failed_flip_is_made_again() {
        let recorder = Recorder::default();
        let mut display = Display::new(3, 2);
        display.attach(recorder.clone());
        display.attach(Failing { good: 4 });
        let fb = frame(3, 2, &[]);
        assert!(display.show(&fb, Note::default()).is_err());
        // Four dots are done. The fifth reached the first output only.
        assert_eq!(recorder.take().len(), 5);

        display.outputs.pop();
        assert_eq!(display.show(&fb, Note::default()).unwrap(), 2);
        assert_eq!(recorder.take().len(), 2);
    }
}
