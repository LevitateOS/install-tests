//! Installation steps and their verification.
//!
//! Each phase of the installation has corresponding step implementations.
//!
//! # STOP. READ. THEN ACT.
//!
//! Before adding or modifying steps:
//! 1. Read all existing phase files (phase1_boot.rs through phase6_verify.rs)
//! 2. Understand the Step trait and how steps are structured
//! 3. Check if similar functionality already exists
//!
//! ## Anti-Reward-Hacking Design
//!
//! Each step follows principles from Anthropic's emergent misalignment research:
//!
//! 1. **Verify actual outcomes, not just exit codes** - We check that expected
//!    state changes actually occurred (files exist, content correct, etc.)
//!
//! 2. **Fail on unexpected state** - If verification shows wrong state,
//!    we report expected vs actual to make debugging clear
//!
//! 3. **Chain dependent operations** - Steps execute AND verify in sequence,
//!    preventing false positives where individual commands pass but flow fails
//!
//! 4. **Each step has an "ensures" statement** - Documents what the step
//!    guarantees for the user when it passes

mod phase1_boot;
mod phase2_disk;
mod phase3_base;
mod phase4_config;
mod phase5_boot;
mod phase6_verify;

pub use phase1_boot::*;
pub use phase2_disk::*;
pub use phase3_base::*;
pub use phase4_config::*;
pub use phase5_boot::*;
pub use phase6_verify::*;

use crate::qemu::Console;
use anyhow::Result;
use std::time::Duration;

/// Result of a verification check
#[derive(Debug, Clone)]
pub enum CheckResult {
    Pass(String),
    Fail { expected: String, actual: String },
}

impl CheckResult {
    pub fn passed(&self) -> bool {
        matches!(self, CheckResult::Pass(_))
    }
}

/// Result of running a step
#[derive(Debug)]
pub struct StepResult {
    pub step_num: usize,
    pub name: String,
    pub passed: bool,
    pub duration: Duration,
    pub checks: Vec<(String, CheckResult)>,
    pub fix_suggestion: Option<String>,
}

impl StepResult {
    pub fn new(step_num: usize, name: &str) -> Self {
        Self {
            step_num,
            name: name.to_string(),
            passed: true,
            duration: Duration::ZERO,
            checks: Vec::new(),
            fix_suggestion: None,
        }
    }

    pub fn add_check(&mut self, name: &str, result: CheckResult) {
        if !result.passed() {
            self.passed = false;
        }
        self.checks.push((name.to_string(), result));
    }

    pub fn fail(&mut self, suggestion: &str) {
        self.passed = false;
        self.fix_suggestion = Some(suggestion.to_string());
    }
}

/// A single installation step
pub trait Step {
    /// Step number (1-24)
    fn num(&self) -> usize;

    /// Step name for display
    fn name(&self) -> &str;

    /// What this step ensures for the end user when it passes.
    /// This is displayed in test output and helps document what each step guarantees.
    fn ensures(&self) -> &str;

    /// Execute the step
    fn execute(&self, console: &mut Console) -> Result<StepResult>;

    /// Phase this step belongs to
    fn phase(&self) -> usize {
        match self.num() {
            1..=2 => 1,   // Boot verification
            3..=6 => 2,   // Disk setup (partition, format, mount)
            7..=10 => 3,  // Base system (mount media, extract, fstab, chroot)
            11..=15 => 4, // Configuration (timezone, locale, hostname, passwords, users)
            16..=18 => 5, // Bootloader (initramfs, bootloader, services)
            19..=24 => 6, // Post-reboot verification (systemd, user, network, sudo)
            _ => 0,
        }
    }
}

/// Get all steps in order
pub fn all_steps() -> Vec<Box<dyn Step>> {
    vec![
        // Phase 1: Boot
        Box::new(phase1_boot::VerifyUefi),
        Box::new(phase1_boot::SyncClock),
        // Phase 2: Disk
        Box::new(phase2_disk::IdentifyDisk),
        Box::new(phase2_disk::PartitionDisk),
        Box::new(phase2_disk::FormatPartitions),
        Box::new(phase2_disk::MountPartitions),
        // Phase 3: Base system
        Box::new(phase3_base::MountInstallMedia),
        Box::new(phase3_base::ExtractTarball),
        Box::new(phase3_base::GenerateFstab),
        Box::new(phase3_base::SetupChroot),
        // Phase 4: Configuration
        Box::new(phase4_config::SetTimezone),
        Box::new(phase4_config::ConfigureLocale),
        Box::new(phase4_config::SetHostname),
        Box::new(phase4_config::SetRootPassword),
        Box::new(phase4_config::CreateUser),
        // Phase 5: Boot setup (initramfs, bootloader, services)
        Box::new(phase5_boot::GenerateInitramfs),
        Box::new(phase5_boot::InstallBootloader),
        Box::new(phase5_boot::EnableServices),
        // Phase 6: Post-reboot verification (runs AFTER booting installed system)
        Box::new(phase6_verify::VerifySystemdBoot),
        Box::new(phase6_verify::VerifyHostname),
        Box::new(phase6_verify::VerifyUserLogin),
        Box::new(phase6_verify::VerifyNetworking),
        Box::new(phase6_verify::VerifySudo),
        Box::new(phase6_verify::VerifyEssentialCommands),
    ]
}

/// Get steps for a specific phase
pub fn steps_for_phase(phase: usize) -> Vec<Box<dyn Step>> {
    all_steps()
        .into_iter()
        .filter(|s| s.phase() == phase)
        .collect()
}
