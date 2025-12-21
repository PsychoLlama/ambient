//! ANSI escape sequences for keyboard input.
//!
//! Provides constants and helpers for simulating keypresses via PTY.

#![allow(dead_code)]

/// Carriage return (Enter key).
pub const ENTER: &[u8] = b"\r";

/// Tab character.
pub const TAB: &[u8] = b"\t";

/// Backspace (DEL character).
pub const BACKSPACE: &[u8] = b"\x7f";

/// Arrow key directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Up,
    Down,
    Left,
    Right,
}

impl Arrow {
    /// Get the ANSI escape sequence for this arrow key.
    pub fn sequence(self) -> &'static [u8] {
        match self {
            Arrow::Up => b"\x1b[A",
            Arrow::Down => b"\x1b[B",
            Arrow::Right => b"\x1b[C",
            Arrow::Left => b"\x1b[D",
        }
    }
}

/// Generate the byte for a Ctrl+key combination.
///
/// Ctrl+A = 0x01, Ctrl+B = 0x02, ..., Ctrl+Z = 0x1A
pub fn ctrl(key: char) -> u8 {
    (key.to_ascii_uppercase() as u8) - b'A' + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ctrl_key() {
        assert_eq!(ctrl('a'), 0x01);
        assert_eq!(ctrl('A'), 0x01);
        assert_eq!(ctrl('c'), 0x03);
        assert_eq!(ctrl('z'), 0x1a);
    }

    #[test]
    fn test_arrow_sequences() {
        assert_eq!(Arrow::Up.sequence(), b"\x1b[A");
        assert_eq!(Arrow::Down.sequence(), b"\x1b[B");
        assert_eq!(Arrow::Right.sequence(), b"\x1b[C");
        assert_eq!(Arrow::Left.sequence(), b"\x1b[D");
    }
}
