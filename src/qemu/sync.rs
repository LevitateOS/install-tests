//! Console synchronization and output flushing.
//!
//! This module handles the tricky problem of reliable command execution over
//! an asynchronous serial console. Serial consoles don't have clean command
//! boundaries - output arrives whenever QEMU feels like sending it, and
//! previous commands may still be printing when you start the next one.
//!
//! # The Problem
//!
//! Without synchronization:
//! - Output from command A can contaminate command B's captured output
//! - You can't reliably know when the shell is ready for input
//! - Late-arriving output causes flaky test failures
//!
//! # The Solution
//!
//! We use a marker-based synchronization protocol:
//! 1. Drain any pending output from previous commands
//! 2. Send `echo ___SYNC_xxx___` and wait for it to appear
//! 3. This proves the shell processed everything before our sync
//! 4. Now we can reliably send commands and capture their output
//!
//! # Known Issues / Bug History
//!
//! This has been a source of many bugs. If you're debugging sync issues:
//! - Check drain timeouts (too short = missed output, too long = slow tests)
//! - Check sync marker uniqueness (timestamp collisions)
//! - Check for ANSI codes in marker detection
//! - Consider secondary sync on timeout (shell may be slow)

use std::io::Write;
use std::process::ChildStdin;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::ansi::strip_ansi_codes;

/// Configuration for sync operations.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// How long to wait when draining output.
    pub drain_wait: Duration,
    /// Timeout for primary sync marker.
    pub sync_timeout: Duration,
    /// Timeout for secondary sync (fallback).
    pub sync2_timeout: Duration,
    /// How long to drain after successful sync.
    pub post_sync_drain: Duration,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            drain_wait: Duration::from_millis(200),
            sync_timeout: Duration::from_secs(5),
            sync2_timeout: Duration::from_secs(3),
            post_sync_drain: Duration::from_millis(100),
        }
    }
}

/// Get a timestamp in microseconds for unique marker generation.
/// Falls back to 0 if system time is unavailable.
pub fn timestamp_micros() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

/// Drain all pending output from the channel.
///
/// Two-pass approach: drain, wait briefly, drain again.
/// This catches output that was in-flight during the first drain.
pub fn drain_output(
    rx: &Receiver<String>,
    output_buffer: &mut Vec<String>,
    wait_duration: Duration,
) {
    // First pass: drain everything currently available
    while let Ok(line) = rx.try_recv() {
        output_buffer.push(line);
    }

    // Brief wait for any in-flight output
    std::thread::sleep(wait_duration);

    // Second pass: drain anything that arrived
    while let Ok(line) = rx.try_recv() {
        output_buffer.push(line);
    }
}

/// Synchronize with the shell to ensure it's ready for commands.
///
/// This sends a sync marker and waits for it to appear in the output,
/// proving that the shell has processed all previous commands.
///
/// Returns Ok(()) if sync succeeded, Err if it failed completely.
pub fn sync_shell(
    stdin: &mut ChildStdin,
    rx: &Receiver<String>,
    output_buffer: &mut Vec<String>,
    config: &SyncConfig,
) -> anyhow::Result<()> {
    // First, drain any pending output
    drain_output(rx, output_buffer, config.drain_wait);

    // Generate unique sync marker
    let sync_id = timestamp_micros();
    let sync_marker = format!("___SYNC_{}___", sync_id);

    // Send sync command
    let sync_cmd = format!("echo '{}'\n", sync_marker);
    stdin.write_all(sync_cmd.as_bytes())?;
    stdin.flush()?;

    // Wait for sync marker
    let sync_start = Instant::now();
    loop {
        if sync_start.elapsed() > config.sync_timeout {
            // Primary sync timed out - try secondary sync
            eprintln!("  WARN: Sync timeout, sending secondary sync...");
            return sync_shell_secondary(stdin, rx, output_buffer, sync_id, config);
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                output_buffer.push(line.clone());
                let clean = strip_ansi_codes(&line);
                if clean.contains(&sync_marker) {
                    // Found sync marker - shell is ready
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("Console disconnected during sync");
            }
        }
    }

    // Extra drain after sync to catch any trailing output
    drain_output(rx, output_buffer, config.post_sync_drain);

    Ok(())
}

/// Secondary sync attempt after primary timeout.
///
/// Uses a different marker and more aggressive draining.
fn sync_shell_secondary(
    stdin: &mut ChildStdin,
    rx: &Receiver<String>,
    output_buffer: &mut Vec<String>,
    sync_id: u128,
    config: &SyncConfig,
) -> anyhow::Result<()> {
    // Aggressive drain
    drain_output(rx, output_buffer, Duration::from_millis(500));

    // Send secondary sync with different marker
    let sync2_marker = format!("___SYNC2_{}___", sync_id);
    let _ = stdin.write_all(format!("echo '{}'\n", sync2_marker).as_bytes());
    let _ = stdin.flush();

    // Wait for secondary sync marker
    let sync2_start = Instant::now();
    while sync2_start.elapsed() < config.sync2_timeout {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                output_buffer.push(line.clone());
                if strip_ansi_codes(&line).contains(&sync2_marker) {
                    break;
                }
            }
            Err(_) => continue,
        }
    }

    // Final aggressive drain
    drain_output(rx, output_buffer, Duration::from_millis(300));

    Ok(())
}

/// Generate unique command markers for output capture.
///
/// Returns (start_marker, done_marker) that can be used to wrap a command
/// and reliably capture only its output.
pub fn generate_command_markers() -> (String, String) {
    let cmd_id = timestamp_micros();
    let start_marker = format!("___START_{}___", cmd_id);
    let done_marker = format!("___DONE_{}___", cmd_id);
    (start_marker, done_marker)
}

/// Check if a line contains any sync/command marker.
///
/// Used to filter out marker lines from command output.
pub fn is_marker_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains("___START_")
        || trimmed.contains("___DONE_")
        || trimmed.contains("___SYNC_")
        || trimmed.contains("___SYNC2_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_marker_line() {
        assert!(is_marker_line("___START_123___"));
        assert!(is_marker_line("___DONE_456___"));
        assert!(is_marker_line("___SYNC_789___"));
        assert!(is_marker_line("  ___SYNC2_123___  "));
        assert!(!is_marker_line("hello world"));
        assert!(!is_marker_line("START something"));
    }

    #[test]
    fn test_generate_command_markers() {
        let (start, done) = generate_command_markers();
        assert!(start.starts_with("___START_"));
        assert!(done.starts_with("___DONE_"));
        assert!(start.ends_with("___"));
        assert!(done.ends_with("___"));
    }

    #[test]
    fn test_timestamp_micros_is_unique() {
        let t1 = timestamp_micros();
        std::thread::sleep(Duration::from_micros(10));
        let t2 = timestamp_micros();
        assert_ne!(t1, t2);
    }
}
