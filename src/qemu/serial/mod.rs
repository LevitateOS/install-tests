//! Serial console boot waits that take their patterns from the variant's
//! scenario contract.

pub use recqemu::serial::Console;

use anyhow::Result;
use distro_contract::{BootScenario, ScenarioContract};
use recinit::live::ROOTFS_MISSING_MESSAGE;
use recqemu::serial::BootPatterns;
use std::time::Duration;

pub trait SerialExecutorExt {
    /// Wait for the live ISO's serial shell.
    fn wait_for_live_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        scenarios: &ScenarioContract,
    ) -> Result<()>;

    /// Wait for the live system when its serial console may run the install
    /// UX instead of a shell (`live_tools.serial_ready_patterns` also count).
    fn wait_for_live_tools_with_context(
        &mut self,
        stall_timeout: Duration,
        scenarios: &ScenarioContract,
    ) -> Result<()>;

    /// Wait for the installed system; returns the failed-service lines seen.
    fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        scenarios: &ScenarioContract,
    ) -> Result<Vec<String>>;
}

/// The live boot's fatal patterns plus the live init's rootfs-missing message.
pub fn live_fatal(live_boot: &BootScenario) -> Vec<String> {
    let mut fatal = live_boot.fatal_patterns.clone();
    fatal.push(ROOTFS_MISSING_MESSAGE.to_string());
    fatal
}

impl SerialExecutorExt for Console {
    fn wait_for_live_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        scenarios: &ScenarioContract,
    ) -> Result<()> {
        let live_boot = &scenarios.live_boot;
        self.wait_for_boot(
            stall_timeout,
            &BootPatterns {
                success: &live_boot.success_patterns,
                fatal: &live_fatal(live_boot),
                service_failures: &live_boot.service_failure_patterns,
            },
        )?;
        Ok(())
    }

    fn wait_for_live_tools_with_context(
        &mut self,
        stall_timeout: Duration,
        scenarios: &ScenarioContract,
    ) -> Result<()> {
        let mut success = scenarios.live_boot.success_patterns.clone();
        success.extend(scenarios.live_tools.serial_ready_patterns.iter().cloned());
        // The live issue text the live overlay writes also means the console is up.
        success.push(distro_builder::install_ux::LIVE_LOGIN_HINT.to_string());
        self.wait_for_boot(
            stall_timeout,
            &BootPatterns {
                success: &success,
                fatal: &live_fatal(&scenarios.live_boot),
                service_failures: &scenarios.live_boot.service_failure_patterns,
            },
        )?;
        Ok(())
    }

    fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        scenarios: &ScenarioContract,
    ) -> Result<Vec<String>> {
        let installed = &scenarios.installed_boot;
        self.wait_for_boot(
            stall_timeout,
            &BootPatterns {
                success: &installed.success_patterns,
                fatal: &installed.fatal_patterns,
                service_failures: &installed.service_failure_patterns,
            },
        )
    }
}
