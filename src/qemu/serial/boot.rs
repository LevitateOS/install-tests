//! Boot waiting and detection for QEMU console.
//!
//! Provides wait_for_boot() and wait_for_installed_boot() with stall detection
//! and fail-fast error pattern matching.
//!
//! Methods come in two variants:
//! - Default methods use hardcoded LevitateOS patterns for backward compatibility
//! - `_with_context` methods accept a DistroContext for multi-distro support

use anyhow::{bail, Result};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::console::Console;
use crate::distro::DistroContext;
use crate::qemu::patterns::{BOOT_ERROR_PATTERNS, CRITICAL_BOOT_ERRORS, SERVICE_FAILURE_PATTERNS};

impl Console {
    /// Wait for the system to boot.
    ///
    /// FAIL FAST DESIGN with STALL DETECTION:
    /// - Detect failure patterns IMMEDIATELY and bail
    /// - Detect success patterns IMMEDIATELY and return
    /// - Stall timeout only triggers if NO OUTPUT for N seconds
    /// - Boot can take as long as it needs as long as progress is being made
    #[allow(dead_code)]
    pub fn wait_for_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        self.wait_for_boot_with_patterns(
            stall_timeout,
            // Success pattern for live ISO boot
            // FAIL FAST: Only accept ___SHELL_READY___ from 00-levitate-test.sh
            // No fallbacks - if instrumentation is broken, test must fail immediately
            &["___SHELL_READY___"],
            // Error patterns (shared)
            BOOT_ERROR_PATTERNS,
            false, // Don't track service failures, fail immediately
        )
    }

    /// Wait for installed system to boot (different success patterns).
    ///
    /// Unlike live ISO boot, this tracks service failures instead of immediately
    /// bailing, allowing us to login and capture diagnostic information.
    #[allow(dead_code)]
    pub fn wait_for_installed_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        self.wait_for_boot_with_patterns(
            stall_timeout,
            // Success patterns for installed system boot
            // Unlike live ISO which has autologin, installed system requires login
            // Use "levitateos login:" to avoid matching "Login Prompts" in systemd output
            // After login, shell emits ___SHELL_READY___ for command execution
            // Also accept multi-user.target - proves system booted successfully even if
            // serial console login prompt has issues (VT emulation quirks in QEMU)
            &["___SHELL_READY___", "levitateos login:", "multi-user.target"],
            // Only critical errors - service failures are tracked separately
            CRITICAL_BOOT_ERRORS,
            true, // Track service failures for later diagnostic capture
        )
    }

    /// Get any failed services that were observed during boot.
    pub fn failed_services(&self) -> &[String] {
        &self.failed_services
    }

    /// Wait for live ISO boot using distro-specific patterns.
    pub fn wait_for_live_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        ctx: &dyn DistroContext,
    ) -> Result<()> {
        self.wait_for_boot_with_patterns(
            stall_timeout,
            ctx.live_boot_success_patterns(),
            ctx.boot_error_patterns(),
            false, // Don't track service failures, fail immediately
        )
    }

    /// Wait for installed system boot using distro-specific patterns.
    pub fn wait_for_installed_boot_with_context(
        &mut self,
        stall_timeout: Duration,
        ctx: &dyn DistroContext,
    ) -> Result<()> {
        self.wait_for_boot_with_patterns(
            stall_timeout,
            ctx.installed_boot_success_patterns(),
            ctx.critical_boot_errors(),
            true, // Track service failures for later diagnostic capture
        )
    }

    /// Core boot waiting logic with configurable patterns.
    ///
    /// Uses STALL DETECTION: only fails if no output for `stall_timeout`.
    /// Boot can take as long as it needs as long as it's making progress.
    ///
    /// If `track_service_failures` is true, service failures are tracked in
    /// `self.failed_services` instead of causing immediate failure. This allows
    /// capturing diagnostics after boot completes.
    fn wait_for_boot_with_patterns(
        &mut self,
        stall_timeout: Duration,
        success_patterns: &[&str],
        error_patterns: &[&str],
        track_service_failures: bool,
    ) -> Result<()> {
        let mut last_output_time = Instant::now();

        // Track what stage we're in for better error messages
        let mut saw_uefi = false;
        let mut saw_bootloader = false;
        let mut saw_kernel = false;

        // Clear any previously tracked service failures
        self.failed_services.clear();

        loop {
            // STALL DETECTION: Only fail if no output for stall_timeout
            // This allows boot to take as long as needed while making progress
            if last_output_time.elapsed() > stall_timeout {
                let stage = if saw_kernel {
                    "Kernel started but init STALLED (no output)"
                } else if saw_bootloader {
                    "Bootloader ran but kernel STALLED (no output)"
                } else if saw_uefi {
                    "UEFI ran but then STALLED (no output)"
                } else {
                    "No output received - QEMU or serial broken"
                };

                let last_lines: Vec<_> = self.output_buffer.iter().rev().take(30).collect();
                let context = last_lines
                    .into_iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!(
                    "BOOT STALLED: {}\n\
                     No output for {} seconds - system appears hung.\n\n\
                     Last output:\n{}",
                    stage,
                    stall_timeout.as_secs(),
                    context
                );
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    // Got output - reset stall timer
                    last_output_time = Instant::now();
                    self.output_buffer.push(line.clone());

                    // Track boot stage for better diagnostics
                    if line.contains("UEFI") || line.contains("BdsDxe") || line.contains("EFI") {
                        saw_uefi = true;
                    }
                    if line.contains("systemd-boot")
                        || line.contains("Loading Linux")
                        || line.contains("loader")
                    {
                        saw_bootloader = true;
                    }
                    if line.contains("Linux version")
                        || line.contains("Booting Linux")
                        || line.contains("KASLR")
                    {
                        saw_kernel = true;
                    }

                    // Track service failures if enabled
                    if track_service_failures {
                        for pattern in SERVICE_FAILURE_PATTERNS {
                            if line.contains(pattern) {
                                // Extract the service name from lines like:
                                // "[FAILED] Failed to start sshd.service - OpenSSH server daemon."
                                // "Starting sshd.service..." followed by failure
                                self.failed_services.push(line.clone());
                                eprintln!("  {} Service failure observed: {}", "WARN:".to_string(), line.trim());
                                break;
                            }
                        }
                    }

                    // FAIL FAST: Check error patterns FIRST
                    for pattern in error_patterns {
                        if line.contains(pattern) {
                            let last_lines: Vec<_> =
                                self.output_buffer.iter().rev().take(30).collect();
                            let context = last_lines
                                .into_iter()
                                .rev()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("\n");
                            bail!("Boot failed: {}\n\nContext:\n{}", pattern, context);
                        }
                    }

                    // Check success patterns
                    for pattern in success_patterns {
                        if line.contains(pattern) {
                            // Small settle time for system to be ready
                            std::thread::sleep(Duration::from_millis(500));
                            return Ok(());
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let last_lines: Vec<_> = self.output_buffer.iter().rev().take(20).collect();
                    let context = last_lines
                        .into_iter()
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n");
                    bail!("QEMU process died\n\nLast output:\n{}", context);
                }
            }
        }
    }
}
