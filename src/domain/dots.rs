//! Dots, and which of them have to flip.
//!
//! A [`FrameBuffer`] is a grid of dots and an `embedded-graphics` draw
//! target. Everything that draws text goes through it.
//!
//! A flipdot panel cannot be repainted for every frame: each dot costs a
//! pulse. [`Shown`] remembers what the dots show and turns every new frame
//! into the [`Flip`]s that differ from it.

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;

/// A grid of dots, each either flipped on or off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameBuffer {
    width: u32,
    height: u32,
    dots: Vec<bool>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            dots: vec![false; (width * height) as usize],
        }
    }

    /// Number of dot columns.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Number of dot rows.
    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn get(&self, x: u32, y: u32) -> bool {
        x < self.width && y < self.height && self.dots[(y * self.width + x) as usize]
    }

    /// Set one dot. Positions outside the display are ignored.
    pub fn set(&mut self, x: u32, y: u32, on: bool) {
        if x < self.width && y < self.height {
            self.dots[(y * self.width + x) as usize] = on;
        }
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            // Anything outside the display is clipped.
            if let (Ok(x), Ok(y)) = (u32::try_from(point.x), u32::try_from(point.y)) {
                self.set(x, y, color.is_on());
            }
        }
        Ok(())
    }
}

/// One dot turned to its other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flip {
    pub x: u32,
    pub y: u32,
    /// The side the dot shows afterwards.
    pub on: bool,
}

/// What the dots of a display show, as far as that is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    width: u32,
    height: u32,
    /// `None` until a dot has been flipped once: a panel keeps its dots
    /// without power and cannot be read back.
    dots: Vec<Option<bool>>,
}

impl Shown {
    /// Nothing is known about the dots yet.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            dots: vec![None; (width * height) as usize],
        }
    }

    /// The flips that turn what is shown into `target`: first the dots
    /// that go off, then the ones that come on, each column by column. A
    /// panel then changes the direction of its drivers only twice a frame.
    pub fn diff(&self, target: &FrameBuffer) -> Vec<Flip> {
        let mut flips = Vec::new();
        for on in [false, true] {
            for x in 0..self.width {
                for y in 0..self.height {
                    let shown = self.dots[(y * self.width + x) as usize];
                    if target.get(x, y) == on && shown != Some(on) {
                        flips.push(Flip { x, y, on });
                    }
                }
            }
        }
        flips
    }

    /// Take note that a flip was made. Flips outside the display are
    /// ignored.
    pub fn record(&mut self, flip: Flip) {
        if flip.x < self.width && flip.y < self.height {
            self.dots[(flip.y * self.width + flip.x) as usize] = Some(flip.on);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::scene::{Align, Font, Scene};

    /// A frame with the dots at `on` flipped on.
    pub(crate) fn frame(width: u32, height: u32, on: &[(u32, u32)]) -> FrameBuffer {
        let mut fb = FrameBuffer::new(width, height);
        for (x, y) in on {
            fb.set(*x, *y, true);
        }
        fb
    }

    /// Bring `shown` to `target` and return the flips it took.
    fn show(shown: &mut Shown, target: &FrameBuffer) -> Vec<Flip> {
        let flips = shown.diff(target);
        for flip in &flips {
            shown.record(*flip);
        }
        flips
    }

    #[test]
    fn dots_outside_the_frame_are_off_and_cannot_be_set() {
        let mut fb = FrameBuffer::new(3, 2);
        fb.set(3, 0, true);
        fb.set(0, 2, true);
        assert_eq!(fb, FrameBuffer::new(3, 2));
        assert!(!fb.get(3, 0));
        assert_eq!((fb.width(), fb.height()), (3, 2));
    }

    #[test]
    fn first_frame_flips_every_dot_once() {
        let mut shown = Shown::new(3, 2);
        let flips = show(&mut shown, &frame(3, 2, &[(1, 0)]));
        assert_eq!(
            flips.len(),
            6,
            "nothing is known about the dots at the start"
        );
        assert_eq!(
            flips.iter().filter(|f| f.on).copied().collect::<Vec<_>>(),
            [Flip {
                x: 1,
                y: 0,
                on: true
            }]
        );
        let mut places: Vec<(u32, u32)> = flips.iter().map(|f| (f.x, f.y)).collect();
        places.sort();
        places.dedup();
        assert_eq!(places.len(), 6, "no dot twice");
    }

    #[test]
    fn same_frame_flips_nothing() {
        let mut shown = Shown::new(3, 2);
        let fb = frame(3, 2, &[(1, 0)]);
        show(&mut shown, &fb);
        assert!(shown.diff(&fb).is_empty());
    }

    #[test]
    fn only_dots_that_differ_are_flipped() {
        let mut shown = Shown::new(3, 2);
        show(&mut shown, &frame(3, 2, &[(0, 0), (1, 0)]));
        assert_eq!(
            show(&mut shown, &frame(3, 2, &[(1, 0), (2, 1)])),
            [
                Flip {
                    x: 0,
                    y: 0,
                    on: false
                },
                Flip {
                    x: 2,
                    y: 1,
                    on: true
                }
            ]
        );
    }

    #[test]
    fn dots_go_off_before_others_come_on_column_by_column() {
        let mut shown = Shown::new(3, 2);
        show(&mut shown, &frame(3, 2, &[(2, 0), (0, 1), (0, 0)]));
        let order: Vec<(u32, u32, bool)> = show(&mut shown, &frame(3, 2, &[(1, 1), (1, 0)]))
            .iter()
            .map(|f| (f.x, f.y, f.on))
            .collect();
        assert_eq!(
            order,
            [
                (0, 0, false),
                (0, 1, false),
                (2, 0, false),
                (1, 0, true),
                (1, 1, true)
            ]
        );
    }

    #[test]
    fn a_flip_that_was_not_recorded_is_planned_again() {
        let mut shown = Shown::new(3, 2);
        let target = frame(3, 2, &[]);
        let flips = shown.diff(&target);
        for flip in &flips[..4] {
            shown.record(*flip);
        }
        assert_eq!(shown.diff(&target), flips[4..]);
    }

    #[test]
    fn a_scrolling_line_flips_far_fewer_dots_than_a_repaint() {
        // Eleven characters are in view on two panels.
        let scene = Scene::new(&["SAS51B EDDH A320"], Font::F5x7, Align::Left, 56, 14);
        assert!(scene.scrolls());
        let mut shown = Shown::new(56, 14);
        assert_eq!(show(&mut shown, &scene.frame(0)).len(), 784);
        let most = (1..=95)
            .map(|tick| show(&mut shown, &scene.frame(tick)).len())
            .max()
            .unwrap();
        assert!(most > 0);
        assert!(most < 784 / 3, "{most} flips for one step");
    }
}
