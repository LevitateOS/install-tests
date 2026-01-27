//! Executor trait for abstracting QEMU I/O backends.
//!
//! This trait allows steps to work with either serial console or QMP backends.
//! Each backend implements command execution, text input, and output waiting.

use anyhow::Result;
use std::time::Duration;

/// Result of executing a command through an executor.
#[derive(Debug, Clone)]
pub struct ExecResult {
    /// Whether the command completed successfully.
    pub completed: bool,
    /// Exit code (0 = success).
    pub exit_code: i32,
    /// Output from the command.
    pub output: String,
    /// Whether execution was aborted due to fatal error pattern.
    pub aborted_on_error: bool,
    /// Whether execution was aborted due to stall (no output).
    pub stalled: bool,
}

impl ExecResult {
    /// Check if the command succeeded.
    pub fn success(&self) -> bool {
        self.completed && self.exit_code == 0 && !self.aborted_on_error && !self.stalled
    }
}

/// Trait for executing commands in QEMU (serial or QMP backend).
///
/// Both serial console and QMP implement this trait, allowing test steps
/// to be written once and run on either backend.
pub trait Executor {
    /// Execute a command and capture output + exit code.
    ///
    /// # Arguments
    /// * `cmd` - The command to run
    /// * `timeout` - Maximum time to wait for completion
    ///
    /// # Returns
    /// ExecResult containing completion status, exit code, and output.
    fn exec(&mut self, cmd: &str, timeout: Duration) -> Result<ExecResult>;

    /// Execute a command that's expected to succeed.
    ///
    /// Returns the output on success, or an error if the command fails.
    fn exec_ok(&mut self, cmd: &str, timeout: Duration) -> Result<String> {
        let result = self.exec(cmd, timeout)?;
        if !result.success() {
            anyhow::bail!(
                "Command failed (exit {}): {}\nOutput: {}",
                result.exit_code,
                cmd,
                result.output
            );
        }
        Ok(result.output)
    }

    /// Execute a command in a chroot environment.
    ///
    /// Uses recchroot (like arch-chroot) to handle bind mounts automatically.
    fn exec_chroot(&mut self, path: &str, cmd: &str, timeout: Duration) -> Result<ExecResult>;

    /// Write a file to the guest system.
    ///
    /// Used for writing configuration files.
    fn write_file(&mut self, path: &str, content: &str) -> Result<()>;

    /// Login to the system with username and password.
    ///
    /// Handles the serial console login flow (waiting for prompts, etc).
    fn login(&mut self, username: &str, password: &str, timeout: Duration) -> Result<()>;

    /// Wait for the live ISO to boot.
    ///
    /// Returns when boot is complete or errors if boot fails.
    fn wait_for_live_boot(&mut self, stall_timeout: Duration) -> Result<()>;

    /// Wait for the installed system to boot.
    ///
    /// Tracks service failures instead of failing immediately.
    fn wait_for_installed_boot(&mut self, stall_timeout: Duration) -> Result<()>;

    /// Get any services that failed during boot.
    fn failed_services(&self) -> &[String];
}
