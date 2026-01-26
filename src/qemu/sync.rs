//! Console synchronization and marker utilities.
//!
//! This module provides marker generation for reliable command execution over
//! serial console. Commands are wrapped with unique markers to reliably
//! capture their output.

/// Get a timestamp in microseconds for unique marker generation.
/// Falls back to 0 if system time is unavailable.
pub fn timestamp_micros() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0)
}

/// Generate unique command markers for output capture.
///
/// Returns (start_marker, done_marker) that can be used to wrap a command
/// and reliably capture only its output.
///
/// Uses 6-digit IDs for uniqueness. Command echo is disabled via `stty -echo`
/// in the shell instrumentation scripts, so marker length doesn't cause wrapping.
pub fn generate_command_markers() -> (String, String) {
    // Use last 6 digits of timestamp - unique enough for sequential commands
    let cmd_id = (timestamp_micros() % 1_000_000) as u32;
    let start_marker = format!("___START_{}___", cmd_id);
    let done_marker = format!("___DONE_{}___", cmd_id);
    (start_marker, done_marker)
}

/// Check if a line contains any sync/command marker.
///
/// Used to filter out marker lines from command output.
pub fn is_marker_line(line: &str) -> bool {
    let trimmed = line.trim();
    // Exec markers
    trimmed.contains("___START_")
        || trimmed.contains("___DONE_")
        || trimmed.contains("___SYNC_")
        || trimmed.contains("___SYNC2_")
        // Shell instrumentation markers (from test profile scripts)
        || trimmed.contains("___SHELL_READY___")
        || trimmed.contains("___PROMPT___")
        || trimmed.contains("___CMD_START_")
        || trimmed.contains("___CMD_END_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_marker_line() {
        // Exec markers
        assert!(is_marker_line("___START_123456___"));
        assert!(is_marker_line("___DONE_789012___"));
        assert!(is_marker_line("___SYNC_789___"));
        assert!(is_marker_line("  ___SYNC2_123___  "));
        // Shell markers
        assert!(is_marker_line("___SHELL_READY___"));
        assert!(is_marker_line("___PROMPT___"));
        assert!(is_marker_line("___CMD_START_123_echo___"));
        assert!(is_marker_line("___CMD_END_123_0___"));
        // Non-markers (these should NOT match)
        assert!(!is_marker_line("hello world"));
        assert!(!is_marker_line("START something"));
        assert!(!is_marker_line("2026"));
        assert!(!is_marker_line("DONE"));
        assert!(!is_marker_line("___SOMETHING_ELSE___"));
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
        std::thread::sleep(std::time::Duration::from_micros(10));
        let t2 = timestamp_micros();
        assert_ne!(t1, t2);
    }
}
