//! Test patterns for the display: every dot both ways, every dot against
//! its neighbours, the rows and the columns on their own, and the edges and
//! the top left corner to check how the panels are mounted.

use crate::domain::dots::FrameBuffer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Every dot on.
    On,
    /// Every dot off.
    Off,
    /// Every dot against its four neighbours.
    Checkerboard,
    /// The checkerboard the other way round: every dot flips.
    CheckerboardInverted,
    /// Every other row.
    Rows,
    /// Every other column.
    Columns,
    /// The outermost dots.
    Border,
    /// A block in the top left corner with a line along the top: shows
    /// whether the picture is mirrored or upside down.
    TopLeft,
}

impl Pattern {
    /// In the order they are shown.
    pub const ALL: [Pattern; 8] = [
        Pattern::On,
        Pattern::Off,
        Pattern::Checkerboard,
        Pattern::CheckerboardInverted,
        Pattern::Rows,
        Pattern::Columns,
        Pattern::Border,
        Pattern::TopLeft,
    ];

    /// In words, for a caption.
    pub fn name(self) -> &'static str {
        match self {
            Pattern::On => "all on",
            Pattern::Off => "all off",
            Pattern::Checkerboard => "checkerboard",
            Pattern::CheckerboardInverted => "checkerboard, inverted",
            Pattern::Rows => "every other row",
            Pattern::Columns => "every other column",
            Pattern::Border => "border",
            Pattern::TopLeft => "top left corner",
        }
    }

    /// Whether dot `x`, `y` of a `width` × `height` display is on.
    fn on(self, x: u32, y: u32, width: u32, height: u32) -> bool {
        match self {
            Pattern::On => true,
            Pattern::Off => false,
            Pattern::Checkerboard => (x + y).is_multiple_of(2),
            Pattern::CheckerboardInverted => !(x + y).is_multiple_of(2),
            Pattern::Rows => y.is_multiple_of(2),
            Pattern::Columns => x.is_multiple_of(2),
            Pattern::Border => x == 0 || y == 0 || x + 1 == width || y + 1 == height,
            Pattern::TopLeft => (x < 4 && y < 4) || y == 0,
        }
    }

    pub fn frame(self, width: u32, height: u32) -> FrameBuffer {
        let mut frame = FrameBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                frame.set(x, y, self.on(x, y, width, height));
            }
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dots as rows of `#` and `.`.
    fn rows(pattern: Pattern, width: u32, height: u32) -> Vec<String> {
        let frame = pattern.frame(width, height);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| if frame.get(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn patterns_look_as_named() {
        assert_eq!(rows(Pattern::On, 3, 2), ["###", "###"]);
        assert_eq!(rows(Pattern::Off, 3, 2), ["...", "..."]);
        assert_eq!(rows(Pattern::Checkerboard, 4, 2), ["#.#.", ".#.#"]);
        assert_eq!(rows(Pattern::Rows, 3, 3), ["###", "...", "###"]);
        assert_eq!(rows(Pattern::Columns, 4, 2), ["#.#.", "#.#."]);
        assert_eq!(
            rows(Pattern::Border, 5, 4),
            ["#####", "#...#", "#...#", "#####"]
        );
        assert_eq!(
            rows(Pattern::TopLeft, 6, 5),
            ["######", "####..", "####..", "####..", "......"]
        );
    }

    #[test]
    fn the_two_checkerboards_flip_every_dot() {
        let (a, b) = (
            Pattern::Checkerboard.frame(28, 16),
            Pattern::CheckerboardInverted.frame(28, 16),
        );
        for y in 0..16 {
            for x in 0..28 {
                assert_ne!(a.get(x, y), b.get(x, y), "{x}/{y}");
            }
        }
    }
}
