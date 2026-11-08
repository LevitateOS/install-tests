//! The boot waits: `recqemu::Console::wait_for_boot` with the oracle
//! `crate::expect` derives for the variant, under the one boot-silence budget.

pub use recqemu::serial::Console;

use crate::budget::Budget;
use crate::expect::{self, BootExpect};
use anyhow::Result;
use distro_contract::Variant;
use recqemu::serial::BootPatterns;

fn wait(console: &mut Console, expect: &BootExpect) -> Result<()> {
    console.wait_for_boot(
        Budget::BootSilence.get(),
        &BootPatterns {
            success: &expect.success,
            fatal: &expect.fatal,
        },
    )
}

/// Wait for a live ISO's serial shell.
pub fn wait_live_boot(console: &mut Console, variant: &Variant) -> Result<()> {
    wait(console, &expect::live_boot(variant))
}

/// Wait for the installed system's login prompt.
pub fn wait_installed_boot(console: &mut Console, variant: &Variant) -> Result<()> {
    wait(console, &expect::installed_boot(variant))
}
