//! QMP input handling - keystrokes and mouse events.
//!
//! This module provides higher-level input functions built on QmpClient.

use crate::qemu::qmp::QmpClient;
use anyhow::Result;

/// QMP key codes for common keys.
pub enum KeyCode {
    // Letters
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    // Numbers
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    // Special keys
    Enter, Tab, Space, Backspace, Escape,
    Up, Down, Left, Right,
    Home, End, PageUp, PageDown,
    Insert, Delete,
    // Function keys
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    // Modifiers
    Shift, Ctrl, Alt,
}

impl KeyCode {
    /// Convert to QMP qcode string.
    pub fn to_qcode(&self) -> &'static str {
        match self {
            // Letters
            KeyCode::A => "a", KeyCode::B => "b", KeyCode::C => "c",
            KeyCode::D => "d", KeyCode::E => "e", KeyCode::F => "f",
            KeyCode::G => "g", KeyCode::H => "h", KeyCode::I => "i",
            KeyCode::J => "j", KeyCode::K => "k", KeyCode::L => "l",
            KeyCode::M => "m", KeyCode::N => "n", KeyCode::O => "o",
            KeyCode::P => "p", KeyCode::Q => "q", KeyCode::R => "r",
            KeyCode::S => "s", KeyCode::T => "t", KeyCode::U => "u",
            KeyCode::V => "v", KeyCode::W => "w", KeyCode::X => "x",
            KeyCode::Y => "y", KeyCode::Z => "z",
            // Numbers
            KeyCode::Num0 => "0", KeyCode::Num1 => "1", KeyCode::Num2 => "2",
            KeyCode::Num3 => "3", KeyCode::Num4 => "4", KeyCode::Num5 => "5",
            KeyCode::Num6 => "6", KeyCode::Num7 => "7", KeyCode::Num8 => "8",
            KeyCode::Num9 => "9",
            // Special keys
            KeyCode::Enter => "ret",
            KeyCode::Tab => "tab",
            KeyCode::Space => "spc",
            KeyCode::Backspace => "backspace",
            KeyCode::Escape => "esc",
            KeyCode::Up => "up",
            KeyCode::Down => "down",
            KeyCode::Left => "left",
            KeyCode::Right => "right",
            KeyCode::Home => "home",
            KeyCode::End => "end",
            KeyCode::PageUp => "pgup",
            KeyCode::PageDown => "pgdn",
            KeyCode::Insert => "insert",
            KeyCode::Delete => "delete",
            // Function keys
            KeyCode::F1 => "f1", KeyCode::F2 => "f2", KeyCode::F3 => "f3",
            KeyCode::F4 => "f4", KeyCode::F5 => "f5", KeyCode::F6 => "f6",
            KeyCode::F7 => "f7", KeyCode::F8 => "f8", KeyCode::F9 => "f9",
            KeyCode::F10 => "f10", KeyCode::F11 => "f11", KeyCode::F12 => "f12",
            // Modifiers
            KeyCode::Shift => "shift",
            KeyCode::Ctrl => "ctrl",
            KeyCode::Alt => "alt",
        }
    }
}

/// Send a single key press.
pub fn send_key(client: &mut QmpClient, key: KeyCode) -> Result<()> {
    client.send_key(key.to_qcode())
}

/// Send text as a series of keystrokes.
pub fn send_text(client: &mut QmpClient, text: &str) -> Result<()> {
    client.send_text(text)
}

/// Send Ctrl+C to interrupt current process.
pub fn send_ctrl_c(client: &mut QmpClient) -> Result<()> {
    client.send_keys(&["ctrl", "c"])
}

/// Send Ctrl+D to signal EOF.
pub fn send_ctrl_d(client: &mut QmpClient) -> Result<()> {
    client.send_keys(&["ctrl", "d"])
}

/// Send Alt+F2 to switch to second virtual console.
pub fn send_alt_f2(client: &mut QmpClient) -> Result<()> {
    client.send_keys(&["alt", "f2"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keycode_to_qcode() {
        assert_eq!(KeyCode::Enter.to_qcode(), "ret");
        assert_eq!(KeyCode::A.to_qcode(), "a");
        assert_eq!(KeyCode::Num0.to_qcode(), "0");
    }
}
