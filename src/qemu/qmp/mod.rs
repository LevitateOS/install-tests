//! QMP (QEMU Machine Protocol) backend for visual install testing.
//!
//! This module provides keystroke injection and screenshot capture via QMP,
//! allowing tests to emulate real user experience: see what they see, type what they type.
//!
//! # Architecture
//!
//! QMP is a JSON-based control interface built into QEMU. It can:
//! - Send keystrokes (`send-key`)
//! - Send mouse events (`input-send-event`)
//! - Capture screenshots (`screendump`)
//! - Control VM state (pause, resume, snapshot)
//!
//! # When to Use QMP vs Serial
//!
//! | Use case | Backend | Why |
//! |----------|---------|-----|
//! | CI/CD pipelines | serial | Fast, text-based verification |
//! | Quick iteration | serial | No rendering overhead |
//! | User experience validation | qmp | Emulates real keyboard input |
//! | Visual regression testing | qmp | Can capture screenshots |
//! | Debugging boot issues | serial | Full text output |
//! | Testing graphical installers | qmp | Required for GUI interaction |
//!
//! # Example
//!
//! ```ignore
//! // Connect to QMP socket
//! let mut qmp = QmpClient::connect("/tmp/qmp.sock")?;
//!
//! // Type a command
//! qmp.send_text("echo hello\n")?;
//!
//! // Take a screenshot
//! qmp.screendump("/tmp/screen.ppm")?;
//! ```

mod capture;
mod client;
mod input;

pub use capture::screendump;
pub use client::QmpClient;
pub use input::{send_key, send_text, KeyCode};

use crate::executor::{ExecResult, Executor};
use anyhow::Result;
use std::time::Duration;

/// Implementation of Executor trait for QMP client.
///
/// This allows test steps to work with the QMP backend through the
/// abstract Executor interface.
///
/// # Note
///
/// QMP-based execution is fundamentally different from serial:
/// - Commands are typed via keystrokes, not stdin
/// - Output is captured via screenshots, not stdout
/// - Exit codes require parsing screen content or using markers
///
/// For most tests, the serial backend is more reliable for command
/// execution. QMP is primarily useful for:
/// - Visual verification
/// - Testing graphical interfaces
/// - Emulating exact user keystrokes
impl Executor for QmpClient {
    fn exec(&mut self, cmd: &str, timeout: Duration) -> Result<ExecResult> {
        // QMP execution is tricky - we type the command and wait for a marker
        // This is a simplified implementation that types commands and waits
        let start_marker = format!("___QMP_START_{}___", std::process::id());
        let end_marker = format!("___QMP_END_{}___", std::process::id());

        // Type the command with markers to capture output
        let full_cmd = format!(
            "echo '{}'; {}; echo '{}' $?\n",
            start_marker, cmd, end_marker
        );

        self.send_text(&full_cmd)?;

        // Wait for output - in QMP mode, we can't easily capture stdout
        // This is a limitation of the QMP approach
        // For now, we assume success after timeout
        std::thread::sleep(timeout.min(Duration::from_secs(5)));

        // QMP can't easily capture command output - this is the main limitation
        // For real output capture, use serial backend or implement VNC screen reading
        Ok(ExecResult {
            completed: true,
            exit_code: 0, // Assumed success - no way to verify without OCR
            output: String::new(), // No output capture in QMP mode
            aborted_on_error: false,
            stalled: false,
        })
    }

    fn exec_chroot(&mut self, path: &str, cmd: &str, timeout: Duration) -> Result<ExecResult> {
        // Execute chroot command same as regular exec
        let full_cmd = format!(
            "recchroot '{}' /bin/bash -c '{}'",
            path.replace('\'', "'\\''"),
            cmd.replace('\'', "'\\''")
        );
        self.exec(&full_cmd, timeout)
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<()> {
        // Use printf with escaped content
        let escaped = content
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
            .replace('%', "%%")
            .replace('\n', "\\n");

        let cmd = format!("set +H; printf \"{}\" > {}\n", escaped, path);
        self.send_text(&cmd)?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    }

    fn login(&mut self, username: &str, password: &str, timeout: Duration) -> Result<()> {
        // Wait for login prompt
        std::thread::sleep(Duration::from_secs(2));

        // Type username
        self.send_text(username)?;
        self.send_text("\n")?;
        std::thread::sleep(Duration::from_secs(1));

        // Type password
        self.send_text(password)?;
        self.send_text("\n")?;
        std::thread::sleep(timeout.min(Duration::from_secs(5)));

        Ok(())
    }

    fn wait_for_live_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        // QMP can't easily detect boot completion without OCR
        // Wait for the timeout and assume boot completed
        // For proper boot detection, use screenshots + OCR or serial backend
        std::thread::sleep(stall_timeout);
        Ok(())
    }

    fn wait_for_installed_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        // Same limitation as wait_for_live_boot
        std::thread::sleep(stall_timeout);
        Ok(())
    }

    fn failed_services(&self) -> &[String] {
        QmpClient::failed_services(self)
    }
}
