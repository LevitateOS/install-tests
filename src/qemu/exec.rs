//! Command execution for QEMU console.
//!
//! Provides exec(), exec_ok(), and exec_streaming() for running commands
//! with exit code capture and error pattern detection.

use anyhow::{bail, Result};
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::ansi::strip_ansi_codes;
use super::console::{CommandResult, Console};
use super::patterns::FATAL_ERROR_PATTERNS;
use super::sync::{self, generate_command_markers, is_marker_line, SyncConfig};

impl Console {
    /// Execute a command and capture output + exit code.
    pub fn exec(&mut self, command: &str, timeout: Duration) -> Result<CommandResult> {
        // Synchronize with the shell before sending command
        let sync_config = SyncConfig::default();
        sync::sync_shell(&mut self.stdin, &self.rx, &mut self.output_buffer, &sync_config)?;

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

    /// Execute a long-running command with STALL DETECTION instead of hard timeout.
    ///
    /// BEST PRACTICE FOR LONG-RUNNING PROCESSES:
    /// - Output is streamed and checked for errors in real-time
    /// - Error patterns cause IMMEDIATE failure (fail-fast)
    /// - No hard timeout - process runs as long as it's producing output
    /// - Only fails if process STALLS (no output for stall_timeout)
    ///
    /// Use this for commands like dracut that legitimately take a long time
    /// but should fail fast if they error.
    pub fn exec_streaming(
        &mut self,
        command: &str,
        stall_timeout: Duration,
        error_patterns: &[&str],
    ) -> Result<CommandResult> {
        // Generate unique markers
        let (start_marker, done_marker) = generate_command_markers();

        // Build command with markers
        let full_cmd = format!(
            "echo '{}'; {}; echo '{}' $?\n",
            start_marker, command, done_marker
        );

        self.stdin.write_all(full_cmd.as_bytes())?;
        self.stdin.flush()?;

        let mut last_output_time = Instant::now();
        let mut output = String::new();
        let mut collecting = false;

        loop {
            // STALL DETECTION: Only fail if no output for stall_timeout
            if last_output_time.elapsed() > stall_timeout {
                return Ok(CommandResult {
                    completed: false,
                    exit_code: -1,
                    output,
                    aborted_on_error: false,
                    stalled: true,
                });
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    // Got output - reset stall timer
                    last_output_time = Instant::now();
                    self.output_buffer.push(line.clone());

                    let clean_line = strip_ansi_codes(&line);
                    let trimmed = clean_line.trim();

                    // FAIL FAST: Check for error patterns IMMEDIATELY
                    for pattern in error_patterns {
                        if trimmed.contains(pattern) {
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

                    // Also check fatal patterns
                    for pattern in FATAL_ERROR_PATTERNS {
                        if trimmed.contains(pattern) {
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

                    // Start marker detection
                    if trimmed.contains(&start_marker) {
                        collecting = true;
                        continue;
                    }

                    // Done marker detection
                    if let Some(pos) = trimmed.find(&done_marker) {
                        let rest = &trimmed[pos + done_marker.len()..];
                        let rest_trimmed = rest.trim();
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

                    // Collect output
                    if collecting {
                        let is_prompt = line.contains("root@") || line.contains("# ");
                        let is_command_echo =
                            line.contains(&done_marker) || line.contains(&start_marker);
                        if !is_prompt && !is_command_echo {
                            output.push_str(&line);
                            output.push('\n');
                        }
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
}
