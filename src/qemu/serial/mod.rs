//! Serial console boot waits, with the patterns `crate::expect` derives for
//! the variant.

pub use recqemu::serial::Console;

use crate::expect::{self, BootExpect};
use anyhow::Result;
use distro_contract::Variant;
use recqemu::serial::BootPatterns;
use std::time::Duration;

pub trait SerialExecutorExt {
    /// Wait for the live ISO's serial shell.
    fn wait_for_live_boot_with_context(&mut self, stall_timeout: Duration, variant: &Variant) -> Result<()>;

    /// Wait for the live system when its serial console may run the install
    /// UX instead of a shell.
    fn wait_for_live_tools_with_context(&mut self, stall_timeout: Duration, variant: &Variant) -> Result<()>;

    /// Wait for the installed system; returns the failed-service lines seen.
    fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        variant: &Variant,
    ) -> Result<Vec<String>>;
}

fn wait(console: &mut Console, stall_timeout: Duration, expect: &BootExpect) -> Result<Vec<String>> {
    console.wait_for_boot(
        stall_timeout,
        &BootPatterns {
            success: &expect.success,
            fatal: &expect.fatal,
            service_failures: &expect.service_failures,
        },
    )
}

impl SerialExecutorExt for Console {
    fn wait_for_live_boot_with_context(&mut self, stall_timeout: Duration, variant: &Variant) -> Result<()> {
        wait(self, stall_timeout, &expect::live_boot(variant)).map(|_| ())
    }

    fn wait_for_live_tools_with_context(&mut self, stall_timeout: Duration, variant: &Variant) -> Result<()> {
        wait(self, stall_timeout, &expect::live_tools(variant)).map(|_| ())
    }

    fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        variant: &Variant,
    ) -> Result<Vec<String>> {
        wait(self, stall_timeout, &expect::installed_boot(variant))
    }
}
