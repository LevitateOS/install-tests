//! Utility functions for QEMU console.
//!
//! Provides write_file() for config creation.
//!
//! Note: Login functionality has been moved to the `auth` module.
//! See `auth.rs` for authentication-related operations.

use anyhow::Result;
use std::time::Duration;

use super::console::Console;

impl Console {
    /// Write a file directly (useful for configs).
    pub fn write_file(&mut self, path: &str, content: &str) -> Result<()> {
        // Use printf with escaped content (heredocs don't work well with serial console)
        // Escape special characters for shell
        // Bug #1 fix: Add % escape for printf format specifiers
        let escaped = content
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
            .replace('%', "%%") // Bug #1 fix: escape % for printf
            .replace('\n', "\\n");

        // Disable history expansion (set +H) to prevent ! from being interpreted
        // as a history command (e.g., #!/bin/bash would fail with "event not found")
        let cmd = format!("set +H; printf \"{}\" > {}", escaped, path);
        self.exec_ok(&cmd, Duration::from_secs(10))?;
        Ok(())
    }
}
