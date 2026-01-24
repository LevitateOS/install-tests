//! Phase 1: Boot verification steps.
//!
//! Steps 1-2: Verify UEFI mode, sync clock, identify target disk.
//!
//! # Anti-Cheat
//!
//! Step 1 (UEFI verification) is CRITICAL. If we're not booting through real UEFI
//! firmware, we're not testing the actual boot chain users will experience.
//! This was the source of the TEAM_062 architectural cheat where -kernel bypass
//! was used while appearing to test UEFI.

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use leviso_cheat_guard::cheat_ensure;
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
        // ANTI-CHEAT: This MUST pass. If it fails, we're using -kernel bypass
        // instead of booting through real UEFI firmware.
        let cmd_result = console.exec(
            "ls /sys/firmware/efi/efivars 2>/dev/null && echo UEFI_OK || echo UEFI_FAIL",
            Duration::from_secs(5),
        )?;

        // CRITICAL: Real UEFI boot is required - no skip, no bypass
        cheat_ensure!(
            cmd_result.output.contains("UEFI_OK"),
            protects = "System booted via real UEFI firmware",
            severity = "CRITICAL",
            cheats = [
                "Use -kernel direct boot to bypass UEFI",
                "Skip UEFI check and mark as optional",
                "Convert failure to Skip instead of Fail"
            ],
            consequence = "UEFI boot path not tested, bootloader issues won't be caught until users hit them on real hardware",
            "UEFI mode not detected - /sys/firmware/efi/efivars missing. \
             This means QEMU is using -kernel bypass instead of booting through OVMF firmware."
        );

        result.add_check("UEFI mode detected", CheckResult::pass("/sys/firmware/efi/efivars exists"));

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

        // NTP check REMOVED - network-dependent, tests run offline
        // The purpose is to verify system time is reasonable for file timestamps,
        // not to test NTP infrastructure.

        // Verify time looks reasonable (year >= 2024)
        // QEMU typically inherits host time, so this should pass
        let date_result = console.exec("date +%Y", Duration::from_secs(5))?;
        let year: i32 = date_result.output.trim().parse().unwrap_or(0);

        // Time must be reasonable for certificates and file timestamps
        cheat_ensure!(
            year >= 2024,
            protects = "System time is reasonable for file operations and certificates",
            severity = "HIGH",
            cheats = [
                "Accept any year value",
                "Skip time check entirely",
                "Convert to warning"
            ],
            consequence = "Wrong system time can cause certificate validation failures and confusing file timestamps",
            "System year is {} - expected >= 2024. RTC not set correctly.", year
        );

        result.add_check("System time reasonable", CheckResult::pass(format!("year={}", year)));

        // Add a small delay to let any async output settle
        // This prevents cross-contamination with the next step
        let _ = console.exec("sleep 0.5", Duration::from_secs(2))?;

        result.duration = start.elapsed();
        Ok(result)
    }
}
