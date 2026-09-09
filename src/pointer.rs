//! The mouse pointer's shape, via OSC 22.
//!
//! A terminal application cannot draw the pointer — the emulator owns it — but
//! it can ask for a shape. `OSC 22 ; <css-shape> ST` sets one and
//! `OSC 22 ; ST` restores the default. Ghostty, kitty, WezTerm, foot and xterm
//! implement it; everything else parses the sequence and discards it, so
//! sending it is safe even where it does nothing.
//!
//! Nothing here writes to the terminal: `next` decides *whether* there is a
//! change worth sending, and the runtime does the writing, so the decision
//! stays testable.

/// The two shapes pgterm asks for: a hand over anything clickable, and the
/// terminal's own default everywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Default,
    Pointer,
}

impl Shape {
    /// The escape sequence that asks for this shape.
    pub fn escape(self) -> &'static str {
        match self {
            // No shape name = "back to whatever you had".
            Shape::Default => "\x1b]22;\x1b\\",
            Shape::Pointer => "\x1b]22;pointer\x1b\\",
        }
    }
}

/// What to send, given the shape the terminal is currently showing and whether
/// the pointer is now over something clickable. `None` means nothing changed,
/// so a mouse dragged across a row does not spray escapes at the terminal.
pub fn next(current: Shape, over_clickable: bool) -> Option<Shape> {
    let want = if over_clickable {
        Shape::Pointer
    } else {
        Shape::Default
    };
    (want != current).then_some(want)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_change_is_worth_sending() {
        assert_eq!(next(Shape::Default, true), Some(Shape::Pointer));
        assert_eq!(next(Shape::Pointer, true), None, "still over it");
        assert_eq!(next(Shape::Pointer, false), Some(Shape::Default));
        assert_eq!(next(Shape::Default, false), None, "still over nothing");
    }

    #[test]
    fn the_escapes_are_the_ones_terminals_document() {
        // OSC 22 ; pointer ST — kitty's pointer-shapes protocol, also
        // implemented by Ghostty, WezTerm, foot and xterm.
        assert_eq!(Shape::Pointer.escape(), "\u{1b}]22;pointer\u{1b}\\");
        // A missing shape name resets to the terminal's default.
        assert_eq!(Shape::Default.escape(), "\u{1b}]22;\u{1b}\\");
        for s in [Shape::Default, Shape::Pointer] {
            let e = s.escape();
            assert!(e.starts_with("\u{1b}]22;"), "must be OSC 22: {e:?}");
            assert!(e.ends_with("\u{1b}\\"), "must end with ST: {e:?}");
        }
    }
}
