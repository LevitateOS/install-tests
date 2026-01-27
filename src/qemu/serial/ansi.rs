//! ANSI escape code handling for serial console output.
//!
//! Serial console output contains ANSI escape sequences for colors,
//! cursor movement, etc. This module provides utilities to strip them
//! for clean pattern matching.

/// Strip ANSI escape codes from a string.
///
/// Handles:
/// - CSI sequences: \x1b[...
/// - OSC sequences: \x1b]... (terminated by BEL or ST)
/// - DCS sequences: \x1bP... (terminated by ST)
/// - Single-character sequences: \x1b followed by single char
/// - Control characters: BEL, NUL, SI, SO
pub fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Handle various escape sequence types
            match chars.peek() {
                // CSI sequences: \x1b[...
                Some(&'[') => {
                    chars.next(); // consume '['
                    // Skip until we find a letter (the command terminator)
                    // CSI sequences can have parameters (numbers, ;) and intermediate bytes
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        // Final byte is 0x40-0x7E (@ through ~)
                        if next.is_ascii_alphabetic()
                            || next == '@'
                            || next == '`'
                            || (next as u8 >= 0x70 && next as u8 <= 0x7E)
                        {
                            break;
                        }
                    }
                }
                // OSC sequences: \x1b]... (terminated by BEL \x07 or ST \x1b\\)
                Some(&']') => {
                    chars.next(); // consume ']'
                    while let Some(next) = chars.next() {
                        if next == '\x07' {
                            break; // BEL terminator
                        }
                        if next == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next(); // consume '\\'
                            break; // ST terminator
                        }
                    }
                }
                // DCS sequences: \x1b P... (terminated by ST)
                Some(&'P') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Single-character sequences: \x1b followed by single char
                Some(&c2)
                    if c2.is_ascii_alphabetic()
                        || c2 == '>'
                        || c2 == '='
                        || c2 == '('
                        || c2 == ')' =>
                {
                    chars.next(); // consume the character
                    // Some sequences like \x1b( have one more char (charset selection)
                    if c2 == '(' || c2 == ')' {
                        chars.next();
                    }
                }
                _ => {
                    // Unknown escape, skip just the ESC
                }
            }
        } else if c == '\x07' || c == '\x00' || c == '\x0f' || c == '\x0e' {
            // Skip BEL, NUL, SI, SO control characters
        } else {
            result.push(c);
        }
    }
    result
}
