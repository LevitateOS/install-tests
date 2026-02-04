//! QMP client for QEMU control.
//!
//! Implements the QEMU Machine Protocol over Unix sockets.
//! QMP uses JSON-RPC style messages for all communication.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// QMP client for communicating with QEMU.
pub struct QmpClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    /// Services that failed during boot (tracked for diagnostics).
    failed_services: Vec<String>,
}

/// QMP greeting message sent by QEMU on connection.
/// Fields are read by serde during deserialization.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct QmpGreeting {
    #[serde(rename = "QMP")]
    qmp: QmpVersion,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct QmpVersion {
    version: QmpVersionInfo,
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct QmpVersionInfo {
    qemu: QemuVersion,
    package: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct QemuVersion {
    micro: u32,
    minor: u32,
    major: u32,
}

/// QMP response structure.
/// Fields are read by serde during deserialization.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct QmpResponse {
    #[serde(rename = "return")]
    return_value: Option<Value>,
    error: Option<QmpError>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct QmpError {
    class: String,
    desc: String,
}

/// QMP command structure.
#[derive(Debug, Serialize)]
struct QmpCommand {
    execute: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<Value>,
}

impl QmpClient {
    /// Connect to a QMP socket.
    ///
    /// # Arguments
    /// * `socket_path` - Path to the QMP Unix socket
    ///
    /// # Returns
    /// Connected QMP client ready for commands.
    pub fn connect<P: AsRef<Path>>(socket_path: P) -> Result<Self> {
        let path = socket_path.as_ref();

        // Connect with timeout
        let stream = UnixStream::connect(path)
            .with_context(|| format!("Failed to connect to QMP socket: {}", path.display()))?;

        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;

        let reader = BufReader::new(stream.try_clone()?);

        let mut client = Self {
            stream,
            reader,
            failed_services: Vec::new(),
        };

        // Wait for QMP greeting
        client.wait_for_greeting()?;

        // Enable QMP command mode
        client.qmp_capabilities()?;

        Ok(client)
    }

    /// Wait for the QMP greeting message.
    fn wait_for_greeting(&mut self) -> Result<()> {
        let mut line = String::new();
        self.reader.read_line(&mut line)?;

        let _greeting: QmpGreeting =
            serde_json::from_str(&line).context("Failed to parse QMP greeting")?;

        Ok(())
    }

    /// Send qmp_capabilities to enable command mode.
    fn qmp_capabilities(&mut self) -> Result<()> {
        self.execute("qmp_capabilities", None)?;
        Ok(())
    }

    /// Execute a QMP command.
    ///
    /// # Arguments
    /// * `command` - QMP command name
    /// * `arguments` - Optional JSON arguments
    ///
    /// # Returns
    /// The return value from QMP, or error if command failed.
    pub fn execute(&mut self, command: &str, arguments: Option<Value>) -> Result<Value> {
        let cmd = QmpCommand {
            execute: command.to_string(),
            arguments,
        };

        let cmd_json = serde_json::to_string(&cmd)?;
        writeln!(self.stream, "{}", cmd_json)?;
        self.stream.flush()?;

        // Read response (may need to skip event messages)
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line)?;

            let value: Value =
                serde_json::from_str(&line).context("Failed to parse QMP response")?;

            // Skip event messages
            if value.get("event").is_some() {
                continue;
            }

            // Check for error
            if let Some(error) = value.get("error") {
                let class = error
                    .get("class")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                let desc = error
                    .get("desc")
                    .and_then(|v| v.as_str())
                    .unwrap_or("No description");
                bail!("QMP error ({}): {}", class, desc);
            }

            // Return the value
            return Ok(value.get("return").cloned().unwrap_or(Value::Null));
        }
    }

    /// Send a key press event.
    ///
    /// # Arguments
    /// * `key` - QMP key code (e.g., "a", "ret", "shift")
    pub fn send_key(&mut self, key: &str) -> Result<()> {
        self.execute(
            "send-key",
            Some(json!({
                "keys": [{"type": "qcode", "data": key}]
            })),
        )?;
        Ok(())
    }

    /// Send a key press with modifiers.
    ///
    /// # Arguments
    /// * `keys` - List of QMP key codes to press simultaneously
    pub fn send_keys(&mut self, keys: &[&str]) -> Result<()> {
        let key_specs: Vec<Value> = keys
            .iter()
            .map(|k| json!({"type": "qcode", "data": k}))
            .collect();

        self.execute(
            "send-key",
            Some(json!({
                "keys": key_specs
            })),
        )?;
        Ok(())
    }

    /// Type a string by sending individual key presses.
    ///
    /// # Arguments
    /// * `text` - Text to type
    pub fn send_text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            let key = char_to_qcode(ch)?;
            if key.needs_shift {
                self.send_keys(&["shift", &key.code])?;
            } else {
                self.send_key(&key.code)?;
            }
            // Brief delay between keystrokes
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    /// Take a screenshot and save to file.
    ///
    /// # Arguments
    /// * `filename` - Path to save the screenshot (PPM format)
    pub fn screendump(&mut self, filename: &str) -> Result<()> {
        self.execute(
            "screendump",
            Some(json!({
                "filename": filename
            })),
        )?;
        Ok(())
    }

    /// Send a mouse move + click event.
    ///
    /// # Arguments
    /// * `x` - X coordinate (0-32767 absolute)
    /// * `y` - Y coordinate (0-32767 absolute)
    /// * `button` - Mouse button ("left", "right", "middle")
    pub fn mouse_click(&mut self, x: i32, y: i32, button: &str) -> Result<()> {
        // Move mouse to position
        self.execute(
            "input-send-event",
            Some(json!({
                "events": [
                    {"type": "abs", "data": {"axis": "x", "value": x}},
                    {"type": "abs", "data": {"axis": "y", "value": y}}
                ]
            })),
        )?;

        // Click
        self.execute(
            "input-send-event",
            Some(json!({
                "events": [
                    {"type": "btn", "data": {"button": button, "down": true}},
                    {"type": "btn", "data": {"button": button, "down": false}}
                ]
            })),
        )?;

        Ok(())
    }

    /// Get failed services tracked during boot.
    pub fn failed_services(&self) -> &[String] {
        &self.failed_services
    }

    /// Track a service failure (for diagnostics).
    pub fn track_service_failure(&mut self, service: String) {
        self.failed_services.push(service);
    }
}

/// Result of converting a character to QMP key code.
struct QCodeResult {
    code: String,
    needs_shift: bool,
}

/// Convert a character to QMP key code.
fn char_to_qcode(ch: char) -> Result<QCodeResult> {
    let (code, needs_shift) = match ch {
        // Lowercase letters
        'a'..='z' => (ch.to_string(), false),
        // Uppercase letters
        'A'..='Z' => (ch.to_ascii_lowercase().to_string(), true),
        // Numbers
        '0' => ("0".to_string(), false),
        '1' => ("1".to_string(), false),
        '2' => ("2".to_string(), false),
        '3' => ("3".to_string(), false),
        '4' => ("4".to_string(), false),
        '5' => ("5".to_string(), false),
        '6' => ("6".to_string(), false),
        '7' => ("7".to_string(), false),
        '8' => ("8".to_string(), false),
        '9' => ("9".to_string(), false),
        // Symbols
        ' ' => ("spc".to_string(), false),
        '\n' => ("ret".to_string(), false),
        '\t' => ("tab".to_string(), false),
        '!' => ("1".to_string(), true),
        '@' => ("2".to_string(), true),
        '#' => ("3".to_string(), true),
        '$' => ("4".to_string(), true),
        '%' => ("5".to_string(), true),
        '^' => ("6".to_string(), true),
        '&' => ("7".to_string(), true),
        '*' => ("8".to_string(), true),
        '(' => ("9".to_string(), true),
        ')' => ("0".to_string(), true),
        '-' => ("minus".to_string(), false),
        '_' => ("minus".to_string(), true),
        '=' => ("equal".to_string(), false),
        '+' => ("equal".to_string(), true),
        '[' => ("bracket_left".to_string(), false),
        '{' => ("bracket_left".to_string(), true),
        ']' => ("bracket_right".to_string(), false),
        '}' => ("bracket_right".to_string(), true),
        '\\' => ("backslash".to_string(), false),
        '|' => ("backslash".to_string(), true),
        ';' => ("semicolon".to_string(), false),
        ':' => ("semicolon".to_string(), true),
        '\'' => ("apostrophe".to_string(), false),
        '"' => ("apostrophe".to_string(), true),
        ',' => ("comma".to_string(), false),
        '<' => ("comma".to_string(), true),
        '.' => ("dot".to_string(), false),
        '>' => ("dot".to_string(), true),
        '/' => ("slash".to_string(), false),
        '?' => ("slash".to_string(), true),
        '`' => ("grave_accent".to_string(), false),
        '~' => ("grave_accent".to_string(), true),
        _ => bail!("Unsupported character: {:?}", ch),
    };

    Ok(QCodeResult { code, needs_shift })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_to_qcode_lowercase() {
        let result = char_to_qcode('a').unwrap();
        assert_eq!(result.code, "a");
        assert!(!result.needs_shift);
    }

    #[test]
    fn test_char_to_qcode_uppercase() {
        let result = char_to_qcode('A').unwrap();
        assert_eq!(result.code, "a");
        assert!(result.needs_shift);
    }

    #[test]
    fn test_char_to_qcode_special() {
        let result = char_to_qcode('\n').unwrap();
        assert_eq!(result.code, "ret");
        assert!(!result.needs_shift);
    }
}
