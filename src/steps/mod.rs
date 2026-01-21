//! Installation steps and their verification.
//!
//! Each phase of the installation has corresponding step implementations.

mod phase1_boot;
mod phase2_disk;
mod phase3_base;
mod phase4_config;
mod phase5_boot;

pub use phase1_boot::*;
pub use phase2_disk::*;
pub use phase3_base::*;
pub use phase4_config::*;
pub use phase5_boot::*;

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
    /// Step number (1-16)
    fn num(&self) -> usize;

    /// Step name for display
    fn name(&self) -> &str;

    /// Execute the step
    fn execute(&self, console: &mut Console) -> Result<StepResult>;

    /// Phase this step belongs to
    fn phase(&self) -> usize {
        match self.num() {
            1..=2 => 1,   // Boot verification
            3..=6 => 2,   // Disk setup (partition, format, mount)
            7..=10 => 3,  // Base system (mount media, extract, fstab, chroot)
            11..=15 => 4, // Configuration (timezone, locale, hostname, passwords, users)
            16..=17 => 5, // Bootloader
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
        // Phase 5: Bootloader
        Box::new(phase5_boot::InstallBootloader),
        Box::new(phase5_boot::EnableServices),
    ]
}

/// Get steps for a specific phase
pub fn steps_for_phase(phase: usize) -> Vec<Box<dyn Step>> {
    all_steps()
        .into_iter()
        .filter(|s| s.phase() == phase)
        .collect()
}
