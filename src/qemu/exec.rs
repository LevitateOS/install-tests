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

/// Get a timestamp in microseconds for unique marker generation.
/// Falls back to 0 if system time is unavailable (Bug #5 fix).
fn timestamp_micros() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

impl Console {
    /// Execute a command and capture output + exit code.
    pub fn exec(&mut self, command: &str, timeout: Duration) -> Result<CommandResult> {
        // First, aggressively drain any pending output
        // This handles cases where previous commands left output in the buffer
        self.drain_output(Duration::from_millis(200));

        // Send a sync command and wait for its completion
        // This ensures the shell is ready for our command
        let sync_id = timestamp_micros();
        let sync_marker = format!("___SYNC_{}___", sync_id);

        // Send sync command
        let sync_cmd = format!("echo '{}'\n", sync_marker);
        self.stdin.write_all(sync_cmd.as_bytes())?;
        self.stdin.flush()?;

        // Wait for sync marker with short timeout, draining everything before it
        let sync_start = Instant::now();
        // Bug #3 fix: removed unused _sync_found variable
        loop {
            if sync_start.elapsed() > Duration::from_secs(5) {
                // Sync timeout - send a second sync to force shell ready state
                eprintln!("  WARN: Sync timeout, sending secondary sync...");

                // Aggressive drain - multiple passes
                self.drain_output(Duration::from_millis(500));

                // Send a secondary sync with a different marker
                let sync2_marker = format!("___SYNC2_{}___", sync_id);
                let _ = self
                    .stdin
                    .write_all(format!("echo '{}'\n", sync2_marker).as_bytes());
                let _ = self.stdin.flush();

                // Wait for secondary sync marker (shorter timeout)
                let sync2_start = Instant::now();
                while sync2_start.elapsed() < Duration::from_secs(3) {
                    match self.rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(line) => {
                            self.output_buffer.push(line.clone());
                            if strip_ansi_codes(&line).contains(&sync2_marker) {
                                break;
                            }
                        }
                        Err(_) => continue,
                    }
                }

                // Final aggressive drain
                self.drain_output(Duration::from_millis(300));
                break;
            }
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());
                    let clean = strip_ansi_codes(&line);
                    if clean.contains(&sync_marker) {
                        // Found sync marker - all previous output has been flushed
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Extra drain after sync to catch any trailing output
        self.drain_output(Duration::from_millis(100));

        // Generate unique markers for this command
        let cmd_id = timestamp_micros();
        let start_marker = format!("___START_{}___", cmd_id);
        let done_marker = format!("___DONE_{}___", cmd_id);

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
                    // Check if line contains our marker (not just ends with - may have terminal codes)
                    if trimmed.contains(&start_marker) {
                        collecting = true;
                        continue;
                    }

                    // Check for completion marker (unique per command)
                    // The done marker is followed by a space and exit code (number).
                    // Using unique markers avoids matching output from previous commands.
                    if let Some(pos) = trimmed.find(&done_marker) {
                        let rest = &trimmed[pos + done_marker.len()..];
                        let rest_trimmed = rest.trim();
                        // Only match if the rest starts with a digit (the exit code)
                        if rest_trimmed
                            .chars()
                            .next()
                            .map(|c| c.is_ascii_digit())
                            .unwrap_or(false)
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

                    // Filter out:
                    // 1. Shell prompts (root@host:~#, [root@host ~]#, etc)
                    // 2. ANY marker lines (___START___, ___DONE___, ___SYNC___)
                    //    This includes markers from previous commands that arrive late
                    let is_prompt = line.contains("root@") || line.contains("# ");
                    let is_any_marker = trimmed.contains("___START_")
                        || trimmed.contains("___DONE_")
                        || trimmed.contains("___SYNC_");

                    if !is_prompt && !is_any_marker {
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
        let cmd_id = timestamp_micros();
        let start_marker = format!("___START_{}___", cmd_id);
        let done_marker = format!("___DONE_{}___", cmd_id);

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
                            .map(|c| c.is_ascii_digit())
                            .unwrap_or(false)
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
