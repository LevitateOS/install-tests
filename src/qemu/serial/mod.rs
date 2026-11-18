//! The boot waits: `recqemu::Console::wait_for_boot` with the oracle
//! `crate::expect` derives for the variant, under the one boot-silence budget.

pub use recqemu::serial::Console;

use crate::budget::Budget;
use crate::expect::{self, BootExpect};
use anyhow::{bail, Result};
use distro_contract::Variant;
use recqemu::serial::BootPatterns;
use recqemu::Guest;

fn wait(console: &mut Console, expect: &BootExpect) -> Result<()> {
    console.wait_for_boot(
        Budget::BootSilence.get(),
        &BootPatterns {
            success: &expect.success,
            fatal: &expect.fatal,
        },
    )
}

/// Stop the kernel's messages from reaching the console. The scenarios read
/// command output from the serial console, so a kernel line (a clocksource
/// watchdog report, say) would land in the middle of an output. `dmesg -n 1`
/// is util-linux's and busybox's; the sysctl file is the fallback.
fn silence_kernel(console: &mut Console) -> Result<()> {
    let outcome = console.exec(
        "dmesg -n 1 2>/dev/null || echo 1 > /proc/sys/kernel/printk",
        Budget::Command.get(),
    )?;
    if !outcome.success() {
        bail!(
            "could not silence the kernel's console messages ({}): {}",
            outcome.describe(),
            outcome.output().trim()
        );
    }
    Ok(())
}

/// Log in on the serial getty, then silence the kernel's console messages:
/// every serial-shell session of the scenarios starts here.
pub fn login(console: &mut Console, username: &str, password: &str) -> Result<()> {
    console.login(username, password, Budget::Login.get())?;
    silence_kernel(console)
}

/// Wait for a live ISO's serial shell.
pub fn wait_live_boot(console: &mut Console, variant: &Variant) -> Result<()> {
    wait(console, &expect::live_boot(variant))?;
    silence_kernel(console)
}

/// Wait for the installed system's login prompt.
pub fn wait_installed_boot(console: &mut Console, variant: &Variant) -> Result<()> {
    wait(console, &expect::installed_boot(variant))
}
