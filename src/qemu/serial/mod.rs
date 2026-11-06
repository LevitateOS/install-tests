//! Serial console waits that take their patterns from the distro context.

pub use recqemu::serial::{CommandResult, Console};

use crate::distro::{load_installed_scenario_facts, DistroContext};
use anyhow::Result;
use std::time::Duration;

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
        let facts = load_installed_scenario_facts(ctx.id())?;
        let success_patterns: Vec<&str> = facts
            .installed_boot
            .success_patterns
            .iter()
            .map(String::as_str)
            .collect();
        Console::wait_for_boot_with_patterns(
            self,
            stall_timeout,
            &success_patterns,
            ctx.critical_boot_errors(),
            true, // Track service failures for later diagnostic capture
        )
    }
}
