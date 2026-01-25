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

use crate::distro::DistroContext;
use crate::qemu::Console;
use anyhow::Result;
use std::time::Duration;

/// Log entry for a command execution
#[derive(Debug, Clone)]
pub struct CommandLog {
    /// The command that was run
    pub command: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Command output (stdout + stderr)
    pub output: String,
    /// Whether the command succeeded
    pub success: bool,
    /// How long the command took
    pub duration: Duration,
}

impl CommandLog {
    pub fn new(
        command: impl Into<String>,
        exit_code: i32,
        output: impl Into<String>,
        duration: Duration,
    ) -> Self {
        Self {
            command: command.into(),
            exit_code,
            output: output.into(),
            success: exit_code == 0,
            duration,
        }
    }
}

/// Result of a verification check
#[derive(Debug, Clone)]
pub enum CheckResult {
    /// Check passed with evidence proving it worked
    /// The evidence string should contain ACTUAL VALUES, not just "ok"
    /// Good: "45MB initramfs at /boot/initramfs.img"
    /// Bad:  "file exists" (skeptic asks: "but is it empty?")
    Pass { evidence: String },
    /// Check failed - the feature is broken
    Fail { expected: String, actual: String },
    /// Check skipped - feature not available (e.g., missing from tarball)
    /// This is NOT a pass - it means the feature wasn't tested
    Skip(String),
    /// Check warning - feature works but with concerns
    /// This is NOT a pass - it indicates a potential issue
    Warning(String),
}

impl CheckResult {
    /// Create a passing check with evidence
    /// Evidence should be ACTUAL VALUES proving it worked, not just "ok"
    pub fn pass(evidence: impl Into<String>) -> Self {
        CheckResult::Pass { evidence: evidence.into() }
    }

    /// Returns true for Skip
    pub fn skipped(&self) -> bool {
        matches!(self, CheckResult::Skip(_))
    }

    /// Returns true for Warning
    pub fn warned(&self) -> bool {
        matches!(self, CheckResult::Warning(_))
    }
}

/// Result of running a step
#[derive(Debug)]
pub struct StepResult {
    pub step_num: usize,
    pub name: String,
    /// True only if all checks passed (no fails, skips don't count as pass)
    pub passed: bool,
    /// True if any check was skipped (indicates incomplete testing)
    pub has_skips: bool,
    /// True if any check has warnings
    pub has_warnings: bool,
    pub duration: Duration,
    pub checks: Vec<(String, CheckResult)>,
    pub fix_suggestion: Option<String>,
    /// Commands executed during this step with their results
    pub commands: Vec<CommandLog>,
}

impl StepResult {
    pub fn new(step_num: usize, name: &str) -> Self {
        Self {
            step_num,
            name: name.to_string(),
            passed: true,
            has_skips: false,
            has_warnings: false,
            duration: Duration::ZERO,
            checks: Vec::new(),
            fix_suggestion: None,
            commands: Vec::new(),
        }
    }

    /// Log a command execution with its result and duration
    pub fn log_command(
        &mut self,
        command: impl Into<String>,
        exit_code: i32,
        output: impl Into<String>,
        duration: Duration,
    ) {
        self.commands.push(CommandLog::new(command, exit_code, output, duration));
    }

    /// Add a passing check with evidence
    /// Evidence should be ACTUAL VALUES that prove the check passed
    pub fn pass(&mut self, name: &str, evidence: impl Into<String>) {
        self.checks.push((
            name.to_string(),
            CheckResult::Pass { evidence: evidence.into() },
        ));
    }

    /// Add a failing check
    pub fn fail(&mut self, name: &str, expected: impl Into<String>, actual: impl Into<String>) {
        self.passed = false;
        self.checks.push((
            name.to_string(),
            CheckResult::Fail {
                expected: expected.into(),
                actual: actual.into(),
            },
        ));
    }

    pub fn add_check(&mut self, name: &str, result: CheckResult) {
        match &result {
            CheckResult::Pass { .. } => {
                // Pass is good, no state change needed
            }
            CheckResult::Fail { .. } => {
                self.passed = false;
            }
            CheckResult::Skip(_) => {
                self.has_skips = true;
                // Skip does NOT set passed=false, but it's tracked separately
            }
            CheckResult::Warning(_) => {
                self.has_warnings = true;
                // Warning does NOT set passed=false, but it's tracked separately
            }
        }
        self.checks.push((name.to_string(), result));
    }

    /// Count of skipped checks
    pub fn skip_count(&self) -> usize {
        self.checks.iter().filter(|(_, r)| r.skipped()).count()
    }

    /// Count of warning checks
    pub fn warning_count(&self) -> usize {
        self.checks.iter().filter(|(_, r)| r.warned()).count()
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

    /// Execute the step with distro context.
    fn execute(&self, console: &mut Console, ctx: &dyn DistroContext) -> Result<StepResult>;

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
        Box::new(phase3_base::ExtractSquashfs),
        Box::new(phase3_base::GenerateFstab),
        Box::new(phase3_base::VerifyChroot),
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
