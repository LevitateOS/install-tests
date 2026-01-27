//! Command execution for QEMU console.
//!
//! Provides exec(), exec_ok(), and exec_streaming() for running commands
//! with exit code capture and error pattern detection.
//!
//! # Shell Marker Protocol
//!
//! The shell instrumentation (00-levitate-test.sh) emits markers:
//! - `___SHELL_READY___` - Shell initialized and ready
//! - `___PROMPT___` - Shell ready for next command (after each command completes)
//! - `___CMD_START_{id}_{cmd}___` - Command starting
//! - `___CMD_END_{id}_{exitcode}___` - Command finished
//!
//! We use `___PROMPT___` to know the shell is ready instead of the expensive
//! sync_shell() protocol, saving ~5-8 seconds per command.

use anyhow::{bail, Result};
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::ansi::strip_ansi_codes;
use super::console::{CommandResult, Console};
use super::sync::{generate_command_markers, is_marker_line};
use crate::qemu::patterns::FATAL_ERROR_PATTERNS;

/// Marker emitted by instrumented shell when ready for next command.
const PROMPT_MARKER: &str = "___PROMPT___";

impl Console {
    /// Wait for the shell to be ready by looking for the ___PROMPT___ marker.
    ///
    /// This is much faster than sync_shell() because the instrumented shell
    /// already emits this marker after each command completes.
    ///
    /// Returns Ok(true) if prompt was found, Ok(false) if timeout.
    fn wait_for_prompt(&mut self, timeout: Duration) -> Result<bool> {
        let start = Instant::now();

        // First, drain any pending output and check for prompt
        while let Ok(line) = self.rx.try_recv() {
            self.output_buffer.push(line.clone());
            let clean = strip_ansi_codes(&line);
            if clean.contains(PROMPT_MARKER) {
                return Ok(true);
            }
        }

        // Not found in pending output, wait for it
        while start.elapsed() < timeout {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());
                    let clean = strip_ansi_codes(&line);
                    if clean.contains(PROMPT_MARKER) {
                        return Ok(true);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("Console disconnected while waiting for prompt");
                }
            }
        }

        Ok(false)
    }

    /// Execute a command and capture output + exit code.
    pub fn exec(&mut self, command: &str, timeout: Duration) -> Result<CommandResult> {
        // Wait for shell to be ready via ___PROMPT___ marker
        // Timeout is short since marker should appear immediately after previous command
        // If marker not seen, continue anyway - the marker is a sync mechanism, not a gate
        // The command will still execute and we'll detect actual failures via exit codes
        let _ = self.wait_for_prompt(Duration::from_millis(500))?;

        // Generate unique markers for this command
        let (start_marker, done_marker) = generate_command_markers();

        // Build command with unique start and end markers
        let full_cmd = format!(
            "echo '{}'; {}; echo '{}' $?\n",
            start_marker, command, done_marker
        );

        self.stdin.write_all(full_cmd.as_bytes())?;
        self.stdin.flush()?;

        let exec_start = Instant::now();
        let mut output = String::new();
        let mut collecting = false;

        loop {
            if exec_start.elapsed() > timeout {
                return Ok(CommandResult {
                    completed: false,
                    exit_code: -1,
                    output,
                    aborted_on_error: false,
                    stalled: false,
                });
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());

                    // Strip ANSI escape codes for cleaner matching
                    let clean_line = strip_ansi_codes(&line);
                    let trimmed = clean_line.trim();

                    // FAIL FAST: Check for fatal error patterns IMMEDIATELY
                    for pattern in FATAL_ERROR_PATTERNS {
                        if trimmed.contains(pattern) {
                            eprintln!("  FATAL ERROR DETECTED: {}", trimmed);
                            output.push_str(&line);
                            output.push('\n');
                            return Ok(CommandResult {
                                completed: false,
                                exit_code: 1,
                                output,
                                aborted_on_error: true,
                                stalled: false,
                            });
                        }
                    }

                    // Wait for start marker before collecting output
                    if trimmed.contains(&start_marker) {
                        collecting = true;
                        continue;
                    }

                    // Check for completion marker (unique per command)
                    if let Some(pos) = trimmed.find(&done_marker) {
                        let rest = &trimmed[pos + done_marker.len()..];
                        let rest_trimmed = rest.trim();
                        // Only match if the rest starts with a digit (the exit code)
                        if rest_trimmed
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_digit())
                        {
                            let exit_code = rest_trimmed
                                .split_whitespace()
                                .next()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(-1);
                            return Ok(CommandResult {
                                completed: true,
                                exit_code,
                                output,
                                aborted_on_error: false,
                                stalled: false,
                            });
                        }
                    }

                    // Only collect output after we've seen the start marker
                    if !collecting {
                        continue;
                    }

                    // Filter out shell prompts and marker lines
                    let is_prompt = line.contains("root@") || line.contains("# ");
                    if !is_prompt && !is_marker_line(trimmed) {
                        output.push_str(&line);
                        output.push('\n');
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok(CommandResult {
                        completed: false,
                        exit_code: -1,
                        output,
                        aborted_on_error: false,
                        stalled: false,
                    });
                }
            }
        }
    }

    /// Execute a command that's expected to succeed.
    pub fn exec_ok(&mut self, command: &str, timeout: Duration) -> Result<String> {
        let result = self.exec(command, timeout)?;
        if !result.success() {
            bail!(
                "Command failed (exit {}): {}\nOutput: {}",
                result.exit_code,
                command,
                result.output
            );
        }
        Ok(result.output)
    }
}
