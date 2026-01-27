//! Serial console backend for QEMU tests.
//!
//! This module provides the Executor trait implementation for serial Console.
//! The actual serial I/O is provided by recqemu::Console.
//!
//! # Re-exports from recqemu
//!
//! - `Console` - Serial I/O with command execution and exit code capture
//! - `CommandResult` - Result of command execution
//! - `generate_command_markers`, `is_marker_line` - Marker utilities
//!
//! # Extensions
//!
//! - `impl Executor for Console` - Adapts Console to the test Executor trait
//! - `SerialExecutorExt` - Context-aware methods for multi-distro support

// Re-export from recqemu
pub use recqemu::serial::{generate_command_markers, is_marker_line, CommandResult, Console};

use crate::distro::DistroContext;
use crate::executor::{ExecResult, Executor};
use anyhow::Result;
use std::time::Duration;

/// Implementation of Executor trait for serial Console.
///
/// This allows test steps to work with the serial backend through the
/// abstract Executor interface.
impl Executor for Console {
    fn exec(&mut self, cmd: &str, timeout: Duration) -> Result<ExecResult> {
        let result = Console::exec(self, cmd, timeout)?;
        Ok(ExecResult {
            completed: result.completed,
            exit_code: result.exit_code,
            output: result.output,
            aborted_on_error: result.aborted_on_error,
            stalled: result.stalled,
        })
    }

    fn exec_chroot(&mut self, path: &str, cmd: &str, timeout: Duration) -> Result<ExecResult> {
        let result = Console::exec_chroot(self, path, cmd, timeout)?;
        Ok(ExecResult {
            completed: result.completed,
            exit_code: result.exit_code,
            output: result.output,
            aborted_on_error: result.aborted_on_error,
            stalled: result.stalled,
        })
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<()> {
        Console::write_file(self, path, content)
    }

    fn login(&mut self, username: &str, password: &str, timeout: Duration) -> Result<()> {
        Console::login(self, username, password, timeout)
    }

    fn wait_for_live_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        Console::wait_for_boot(self, stall_timeout)
    }

    fn wait_for_installed_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        Console::wait_for_installed_boot(self, stall_timeout)
    }

    fn failed_services(&self) -> &[String] {
        Console::failed_services(self)
    }
}

/// Wrapper trait extension for Console to work with DistroContext.
///
/// The Executor trait is generic and doesn't know about DistroContext.
/// This extension adds context-aware methods for the serial backend.
pub trait SerialExecutorExt {
    fn wait_for_live_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        ctx: &dyn DistroContext,
    ) -> Result<()>;

    fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        ctx: &dyn DistroContext,
    ) -> Result<()>;
}

impl SerialExecutorExt for Console {
    fn wait_for_live_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        ctx: &dyn DistroContext,
    ) -> Result<()> {
        Console::wait_for_boot_with_patterns(
            self,
            stall_timeout,
            ctx.live_boot_success_patterns(),
            ctx.boot_error_patterns(),
            false, // Don't track service failures, fail immediately
        )
    }

    fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        ctx: &dyn DistroContext,
    ) -> Result<()> {
        Console::wait_for_boot_with_patterns(
            self,
            stall_timeout,
            ctx.installed_boot_success_patterns(),
            ctx.critical_boot_errors(),
            true, // Track service failures for later diagnostic capture
        )
    }
}
