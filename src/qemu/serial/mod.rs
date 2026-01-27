//! Serial console backend for QEMU tests.
//!
//! This module provides serial I/O for command execution with exit code capture.
//!
//! # STOP. READ. THEN ACT.
//!
//! This module already has:
//! - `Console` - Serial I/O with command execution and exit code capture
//! - `exec()` / `exec_ok()` - Run commands with exit code capture
//! - `exec_chroot()` - Run commands in chroot via recchroot
//! - `wait_for_boot()` - Wait for systemd startup
//! - `write_file()` - Write files via serial console
//! - `login()` - Authentication subsystem (login, shell markers)
//!
//! Read all methods before adding new ones. Don't duplicate functionality.

mod ansi;
mod auth;
mod boot;
mod chroot;
mod console;
mod exec;
mod sync;
mod utils;

pub use console::{CommandResult, Console};
pub use sync::{generate_command_markers, is_marker_line};

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
        Console::wait_for_live_boot_with_context(self, stall_timeout, ctx)
    }

    fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        ctx: &dyn DistroContext,
    ) -> Result<()> {
        Console::wait_for_installed_boot_with_context(self, stall_timeout, ctx)
    }
}
