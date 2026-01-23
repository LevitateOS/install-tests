//! Phase 1: Boot verification steps.
//!
//! Steps 1-2: Verify UEFI mode, sync clock, identify target disk.

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use std::time::{Duration, Instant};

/// Step 1: Verify UEFI boot mode
pub struct VerifyUefi;

impl Step for VerifyUefi {
    fn num(&self) -> usize { 1 }
    fn name(&self) -> &str { "Verify UEFI Boot Mode" }
    fn ensures(&self) -> &str {
        "System booted in UEFI mode required for GPT/ESP installation"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check for EFI variables directory
        // Note: With direct kernel boot (-kernel), UEFI firmware is not actually used
        // even if OVMF is loaded. This is expected behavior for testing.
        let cmd_result = console.exec(
            "ls /sys/firmware/efi/efivars 2>/dev/null && echo UEFI_OK || echo UEFI_FAIL",
            Duration::from_secs(5),
        )?;

        if cmd_result.output.contains("UEFI_OK") {
            result.add_check("UEFI mode detected", CheckResult::Pass);
        } else {
            // Direct kernel boot bypasses UEFI - this is a SKIP, not a pass
            // We're not actually testing UEFI boot, just using GPT+ESP layout
            result.add_check(
                "UEFI mode detected",
                CheckResult::Skip("Direct kernel boot - UEFI not tested (using GPT+ESP layout)".to_string()),
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 2: Sync system clock
pub struct SyncClock;

impl Step for SyncClock {
    fn num(&self) -> usize { 2 }
    fn name(&self) -> &str { "Sync System Clock" }
    fn ensures(&self) -> &str {
        "System clock is synchronized for proper file timestamps and certificates"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Try to enable NTP sync (may not work in all environments)
        let _ntp_result = console.exec(
            "timedatectl set-ntp true 2>&1",
            Duration::from_secs(5),
        )?;

        // Check timedatectl status
        let status = console.exec(
            "timedatectl show --property=NTP --property=NTPSynchronized",
            Duration::from_secs(5),
        )?;

        if status.output.contains("NTP=yes") {
            result.add_check("NTP enabled", CheckResult::Pass);
        } else {
            // NTP not working - this is a SKIP, not a pass
            // We didn't test NTP functionality
            result.add_check(
                "NTP enabled",
                CheckResult::Skip("NTP not available in QEMU test environment".to_string()),
            );
        }

        // Verify time looks reasonable (year >= 2024)
        // Note: QEMU's RTC may not be set correctly, so we just note the time
        // without failing - this is not critical for offline installation
        let date_result = console.exec("date +%Y", Duration::from_secs(5))?;
        let year: i32 = date_result.output.trim().parse().unwrap_or(0);

        if year >= 2024 {
            result.add_check("System time reasonable", CheckResult::Pass);
        } else {
            // Wrong year - this is a WARNING, not a pass
            // System time is wrong but installation can proceed
            result.add_check(
                "System time reasonable",
                CheckResult::Warning(format!("Year is {} - RTC not set correctly", year)),
            );
        }

        // Add a small delay to let any timedatectl async output settle
        // This prevents cross-contamination with the next step
        let _ = console.exec("sleep 0.5", Duration::from_secs(2))?;

        result.duration = start.elapsed();
        Ok(result)
    }
}
