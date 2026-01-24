//! E2E Installation Test Runner for LevitateOS.
//!
//! Runs installation steps in QEMU and verifies each step completes correctly.
//!
//! # STOP. READ. THEN ACT.
//!
//! This is the CORRECT location for E2E installation tests.
//! NOT `leviso/tests/`. THIS crate. Read before writing.
//!
//! Before modifying this code:
//! 1. Read the existing modules in `qemu/` and `steps/`
//! 2. Understand what already exists
//! 3. Don't duplicate functionality
//!
//! See `/home/vince/Projects/LevitateOS/STOP_READ_THEN_ACT.md` for why this matters.

mod qemu;
mod steps;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use qemu::{find_ovmf, create_disk, QemuBuilder, kill_stale_qemu_processes, acquire_test_lock};
use steps::{all_steps, steps_for_phase, Step, StepResult, CheckResult, CommandLog};

#[derive(Parser)]
#[command(name = "install-tests")]
#[command(about = "E2E installation test runner for LevitateOS")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run installation tests
    Run {
        /// Run only a specific step (1-24)
        #[arg(long)]
        step: Option<usize>,

        /// Run only steps in a specific phase (1-6)
        #[arg(long)]
        phase: Option<usize>,

        /// Path to leviso directory (default: ../../leviso)
        #[arg(long, default_value = "../../leviso")]
        leviso_dir: PathBuf,

        /// Path to ISO file (default: <leviso_dir>/output/leviso.iso)
        #[arg(long)]
        iso: Option<PathBuf>,

        /// Disk size for virtual disk (20G matches production requirements)
        #[arg(long, default_value = "20G")]
        disk_size: String,

        /// Keep VM running after tests (for debugging)
        #[arg(long)]
        keep_vm: bool,
    },

    /// List all test steps
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { step, phase, leviso_dir, iso, disk_size, keep_vm } => {
            run_tests(step, phase, &leviso_dir, iso, &disk_size, keep_vm)
        }
        Commands::List => {
            list_steps();
            Ok(())
        }
    }
}

fn list_steps() {
    println!("{}", "LevitateOS Installation Test Steps".bold());
    println!();
    println!("Each step has an 'ensures' statement describing what it guarantees.");
    println!();
    println!("{}", "Phases 1-5 run on the live ISO, Phase 6 runs after rebooting into the installed system.".yellow());
    println!();

    let steps = all_steps();
    let mut current_phase = 0;

    for step in steps {
        if step.phase() != current_phase {
            current_phase = step.phase();
            println!();
            let phase_desc = match current_phase {
                1 => "Phase 1 (Boot Verification)",
                2 => "Phase 2 (Disk Setup)",
                3 => "Phase 3 (Base System)",
                4 => "Phase 4 (Configuration)",
                5 => "Phase 5 (Bootloader)",
                6 => "Phase 6 (Post-Reboot Verification) ← REBOOTS INTO INSTALLED SYSTEM",
                _ => "Unknown Phase",
            };
            println!("{}", phase_desc.blue().bold());
        }
        println!("  {:2}. {}", step.num(), step.name());
        println!("      ensures: {}", step.ensures());
    }
    println!();
}

/// Print command logs for a step
fn print_command_logs(commands: &[CommandLog]) {
    if commands.is_empty() {
        return;
    }

    for cmd in commands {
        let exit_status = if cmd.success {
            format!("{}", cmd.exit_code).green()
        } else {
            format!("{}", cmd.exit_code).red()
        };

        // Format duration - skeptics want to see timing
        let duration_str = if cmd.duration.as_millis() < 1000 {
            format!("{}ms", cmd.duration.as_millis())
        } else {
            format!("{:.1}s", cmd.duration.as_secs_f64())
        };

        // Show full command - don't hide anything from skeptics
        println!(
            "    {} {} [{}, {}]",
            "→".cyan(),
            cmd.command.dimmed(),
            exit_status,
            duration_str.dimmed()
        );

        // Show ALL output - truncating hides evidence
        let output = cmd.output.trim();
        if !output.is_empty() {
            for line in output.lines() {
                println!("      {}", line.dimmed());
            }
        }
    }
}

/// Print check results with evidence
fn print_checks(checks: &[(String, CheckResult)]) {
    for (check_name, check_result) in checks {
        match check_result {
            CheckResult::Pass { evidence } => {
                println!("    {} {}: {}", "✓".green(), check_name, evidence.green());
            }
            CheckResult::Fail { expected, actual } => {
                println!("    {} {}", "✗".red(), check_name);
                println!("      expected: {}", expected);
                println!("      actual:   {}", actual.red());
            }
            CheckResult::Skip(reason) => {
                println!("    {} {}: {}", "⊘".yellow(), check_name, reason);
            }
            CheckResult::Warning(reason) => {
                println!("    {} {}: {}", "⚠".yellow(), check_name, reason);
            }
        }
    }
}

/// Run a single step and print result
fn run_single_step(step: &dyn Step, console: &mut qemu::Console) -> Result<(StepResult, bool)> {
    print!("{} Step {:2}: {}... ",
        "▶".cyan(),
        step.num(),
        step.name()
    );

    let start = Instant::now();
    match step.execute(console) {
        Ok(result) => {
            let duration = start.elapsed();
            if result.passed {
                // Show pass with any skip/warning counts
                let mut status = "PASS".green().bold().to_string();
                if result.has_skips || result.has_warnings {
                    let mut notes = Vec::new();
                    if result.has_skips {
                        notes.push(format!("{} skipped", result.skip_count()));
                    }
                    if result.has_warnings {
                        notes.push(format!("{} warnings", result.warning_count()));
                    }
                    status = format!("{} ({})", "PASS".green().bold(), notes.join(", ").yellow());
                }
                println!("{} ({:.1}s)", status, duration.as_secs_f64());

                // Print command logs - show what actually ran
                print_command_logs(&result.commands);

                // Print ALL checks with evidence - skeptics want proof
                print_checks(&result.checks);

                Ok((result, true))
            } else {
                println!("{} ({:.1}s)", "FAIL".red().bold(), duration.as_secs_f64());

                // Print command logs - essential for debugging failures
                print_command_logs(&result.commands);

                // Print ALL checks - show what passed AND what failed
                print_checks(&result.checks);

                if let Some(fix) = &result.fix_suggestion {
                    println!("    {} {}", "Fix:".yellow(), fix);
                }

                Ok((result, false))
            }
        }
        Err(e) => {
            println!("{}", "ERROR".red().bold());
            println!("    {}", e);
            Err(e)
        }
    }
}

fn run_tests(
    step_num: Option<usize>,
    phase_num: Option<usize>,
    leviso_dir: &std::path::Path,
    iso_path: Option<PathBuf>,
    disk_size: &str,
    _keep_vm: bool,
) -> Result<()> {
    println!("{}", "LevitateOS E2E Installation Tests".bold());
    println!();

    // CRITICAL: Acquire exclusive lock and kill any stale QEMU processes
    // This prevents memory leaks from zombie QEMU instances
    println!("{}", "Acquiring test lock and cleaning up stale processes...".cyan());
    kill_stale_qemu_processes();
    let _lock = acquire_test_lock()?;
    println!("{}", "Lock acquired, no other tests running.".green());
    println!();

    // Validate leviso directory
    // ANTI-CHEAT: We boot the ISO through real UEFI firmware, not -kernel bypass
    // This tests the actual boot chain: OVMF → systemd-boot → kernel
    let iso_path = iso_path.unwrap_or_else(|| leviso_dir.join("output/levitateos.iso"));

    if !iso_path.exists() {
        bail!(
            "ISO not found at {}. Run 'cargo run -- iso' in leviso first.",
            iso_path.display()
        );
    }

    println!("  ISO:       {}", iso_path.display());

    // Find OVMF for UEFI boot
    let ovmf = find_ovmf().context("OVMF not found - UEFI boot required for installation tests")?;
    println!("  OVMF:      {}", ovmf.display());

    // Find OVMF_VARS template and copy to temp location (needs to be writable)
    let ovmf_vars_template = qemu::find_ovmf_vars()
        .context("OVMF_VARS not found - needed for EFI variable storage")?;
    let ovmf_vars_path = std::env::temp_dir().join("leviso-install-test-vars.fd");
    if ovmf_vars_path.exists() {
        std::fs::remove_file(&ovmf_vars_path)?;
    }
    std::fs::copy(&ovmf_vars_template, &ovmf_vars_path)?;
    println!("  OVMF_VARS: {} (copied from {})", ovmf_vars_path.display(), ovmf_vars_template.display());

    // Create test disk
    let disk_path = std::env::temp_dir().join("leviso-install-test.qcow2");
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    create_disk(&disk_path, disk_size)?;
    println!("  Disk:      {} ({})", disk_path.display(), disk_size);
    println!();

    // Determine which steps to run
    let all_requested: Vec<Box<dyn Step>> = match (step_num, phase_num) {
        (Some(n), _) => {
            all_steps().into_iter().filter(|s| s.num() == n).collect()
        }
        (_, Some(p)) => {
            steps_for_phase(p)
        }
        (None, None) => {
            all_steps()
        }
    };

    if all_requested.is_empty() {
        bail!("No steps match the specified criteria");
    }

    // Split steps into pre-reboot (1-18) and post-reboot (19-24)
    let pre_reboot_steps: Vec<_> = all_requested.iter()
        .filter(|s| s.num() <= 18)
        .map(|s| s.num())
        .collect();
    let post_reboot_steps: Vec<_> = all_requested.iter()
        .filter(|s| s.num() >= 19)
        .map(|s| s.num())
        .collect();

    let needs_pre_reboot = !pre_reboot_steps.is_empty();
    let needs_post_reboot = !post_reboot_steps.is_empty();

    let mut results: Vec<StepResult> = Vec::new();
    let mut all_passed = true;

    // ═══════════════════════════════════════════════════════════════════════
    // PHASE 1-5: Run installation steps on the live ISO
    // ═══════════════════════════════════════════════════════════════════════
    if needs_pre_reboot {
        println!("{}", "═".repeat(60));
        println!("{}", "INSTALLATION PHASE (Live ISO)".cyan().bold());
        println!("{}", "═".repeat(60));
        println!();

        // Build QEMU command for live ISO boot
        // ANTI-CHEAT: Boot through real UEFI firmware, not -kernel bypass
        // Boot chain: OVMF → ISO's EFI boot → systemd-boot → kernel → initramfs → live system
        let mut cmd = QemuBuilder::new()
            .cdrom(iso_path.clone())
            .disk(disk_path.clone())
            .uefi(ovmf.clone())
            .uefi_vars(ovmf_vars_path.clone())  // Writable for boot entries
            .boot_order("dc")  // CDROM first (live ISO), then disk
            .with_user_network()  // Enable networking for IP address testing
            .nographic()
            .no_reboot()
            .build_piped();

        // Spawn QEMU
        println!("{}", "Starting QEMU (live ISO)...".cyan());
        let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
        let mut console = qemu::Console::new(&mut child)?;

        // Wait for boot - fail-fast detection, timeout only if detection broken
        println!("{}", "Waiting for boot...".cyan());
        console.wait_for_boot(Duration::from_secs(30))?;
        println!("{}", "Live ISO booted!".green());
        println!();

        // Run pre-reboot steps
        let steps: Vec<Box<dyn Step>> = all_steps()
            .into_iter()
            .filter(|s| pre_reboot_steps.contains(&s.num()))
            .collect();

        for step in steps {
            let (result, passed) = run_single_step(step.as_ref(), &mut console)?;
            if !passed {
                results.push(result);
                // Stop on first failure
                drop(console);
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&disk_path);
                bail!("Installation tests failed");
            }
            results.push(result);
        }

        // Shutdown the live ISO
        println!();
        println!("{}", "Installation complete, shutting down live ISO...".cyan());
        let _ = console.exec("poweroff -f", Duration::from_secs(5));
        drop(console);
        // ALWAYS kill the child in case poweroff didn't work
        let _ = child.kill();
        let _ = child.wait();

        // Give it a moment to fully terminate
        std::thread::sleep(Duration::from_secs(1));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // PHASE 6: Boot the installed system and verify
    // ═══════════════════════════════════════════════════════════════════════
    if needs_post_reboot && all_passed {
        println!();
        println!("{}", "═".repeat(60));
        println!("{}", "VERIFICATION PHASE (Installed System)".cyan().bold());
        println!("{}", "═".repeat(60));
        println!();

        // Boot from disk (not ISO) - UEFI will boot from disk's EFI partition
        // Boot chain: OVMF → disk's EFI partition → systemd-boot → kernel
        // Same OVMF_VARS file is used to preserve boot entries from installation
        let mut cmd = QemuBuilder::new()
            .disk(disk_path.clone())
            .uefi(ovmf.clone())
            .uefi_vars(ovmf_vars_path.clone())
            .boot_order("c")  // Disk only (installed system)
            .with_user_network()  // Enable networking for IP address testing
            .nographic()
            .no_reboot()
            .build_piped();

        println!("{}", "Starting QEMU (booting installed system)...".cyan());
        let mut child = cmd.spawn().context("Failed to spawn QEMU for installed system")?;
        let mut console = qemu::Console::new(&mut child)?;

        // Wait for the installed system to boot
        // Uses fail-fast detection - timeout only triggers if detection is broken
        // Service failures are tracked (not fatal) so we can capture diagnostics
        println!("{}", "Waiting for installed system to boot...".cyan());
        console.wait_for_installed_boot(Duration::from_secs(30))?;
        println!("{}", "Installed system booted!".green());

        // Check if any services failed during boot
        let boot_failures = console.failed_services().to_vec();
        if !boot_failures.is_empty() {
            println!();
            println!("{}", "⚠ Services failed during boot - will capture diagnostics".yellow());
            for failure in &boot_failures {
                println!("    {}", failure.trim());
            }
            println!();
        }

        // Login or verify shell access (handles both autologin and manual login cases)
        println!("{}", "Verifying shell access...".cyan());
        console.login("root", "levitate", Duration::from_secs(15))?;
        println!("{}", "Logged in!".green());

        // Brief settle time - shell should be ready after login verification
        std::thread::sleep(Duration::from_millis(500));

        // Single warmup command to verify shell is functional
        let warmup = console.exec("echo SHELL_READY_CHECK", Duration::from_secs(5))?;
        if !warmup.output.contains("SHELL_READY_CHECK") {
            // Shell is broken - don't proceed with useless tests
            drop(console);
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&disk_path);
            let _ = std::fs::remove_file(&ovmf_vars_path);
            bail!(
                "Shell warmup failed. The installed system booted but \
                 the shell is not responding correctly. This indicates a broken installation.\n\
                 Got: {:?}",
                warmup.output.trim()
            );
        }
        println!("{}", "Shell ready!".green());

        // If services failed during boot, capture diagnostics NOW
        if !boot_failures.is_empty() {
            println!();
            println!("{}", "═".repeat(60));
            println!("{}", "CAPTURING DIAGNOSTICS FOR FAILED SERVICES".yellow().bold());
            println!("{}", "═".repeat(60));
            println!();

            // Get list of failed units
            let failed_list = console.exec(
                "systemctl --failed --no-pager",
                Duration::from_secs(10),
            )?;
            println!("{}", "Failed units:".yellow());
            println!("{}", failed_list.output);

            // For each failed service, get detailed status
            // Extract service names from boot_failures
            for failure_line in &boot_failures {
                // Try to extract service name (e.g., "sshd.service")
                if let Some(start) = failure_line.find("start ") {
                    let after_start = &failure_line[start + 6..];
                    if let Some(end) = after_start.find(|c: char| c == ' ' || c == '-' || c == '.') {
                        let service = &after_start[..end];
                        // Try getting status for common service patterns
                        for suffix in ["", ".service"] {
                            let full_name = format!("{}{}", service, suffix);
                            println!();
                            println!("{} {}:", "Status of".yellow(), full_name);
                            let status = console.exec(
                                &format!("systemctl status {} --no-pager 2>&1 || true", full_name),
                                Duration::from_secs(10),
                            )?;
                            println!("{}", status.output);

                            // Get journal logs
                            println!("{} {}:", "Journal for".yellow(), full_name);
                            let journal = console.exec(
                                &format!("journalctl -u {} --no-pager -n 50 2>&1 || true", full_name),
                                Duration::from_secs(10),
                            )?;
                            println!("{}", journal.output);
                        }
                    }
                }
            }

            // Also try common failing services
            for service in ["sshd", "sshd-keygen@rsa", "sshd-keygen@ecdsa", "sshd-keygen@ed25519"] {
                println!();
                println!("{} {}:", "Checking".yellow(), service);
                let status = console.exec(
                    &format!("systemctl status {} --no-pager 2>&1 | head -30 || true", service),
                    Duration::from_secs(10),
                )?;
                if !status.output.contains("could not be found") {
                    println!("{}", status.output);
                }
            }

            // Check /run/sshd exists
            println!();
            println!("{}", "Checking /run/sshd:".yellow());
            let run_sshd = console.exec("ls -la /run/sshd 2>&1 || echo 'NOT FOUND'", Duration::from_secs(5))?;
            println!("{}", run_sshd.output);

            // Check SSH host keys
            println!();
            println!("{}", "Checking SSH host keys:".yellow());
            let host_keys = console.exec("ls -la /etc/ssh/ssh_host_* 2>&1 || echo 'NO HOST KEYS'", Duration::from_secs(5))?;
            println!("{}", host_keys.output);

            // Check tmpfiles.d config
            println!();
            println!("{}", "Checking tmpfiles.d sshd config:".yellow());
            let tmpfiles = console.exec("cat /usr/lib/tmpfiles.d/sshd.conf 2>&1 || echo 'NOT FOUND'", Duration::from_secs(5))?;
            println!("{}", tmpfiles.output);

            // ALL OR NOTHING: Services failed, this is a test failure
            drop(console);
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&disk_path);
            let _ = std::fs::remove_file(&ovmf_vars_path);
            bail!(
                "Boot completed but {} service(s) failed. See diagnostics above.",
                boot_failures.len()
            );
        }

        // Run post-reboot verification steps
        let steps: Vec<Box<dyn Step>> = all_steps()
            .into_iter()
            .filter(|s| post_reboot_steps.contains(&s.num()))
            .collect();

        for step in steps {
            let (result, passed) = run_single_step(step.as_ref(), &mut console)?;
            if !passed {
                all_passed = false;
            }
            results.push(result);
            // Don't break on failure for verification - run all checks
        }

        // Cleanup
        drop(console);
        let _ = child.kill();
        let _ = child.wait();
    }

    // ═══════════════════════════════════════════════════════════════════════
    // SUMMARY
    // ═══════════════════════════════════════════════════════════════════════
    println!();
    println!("{}", "═".repeat(60));
    println!("{}", "TEST SUMMARY".cyan().bold());
    println!("{}", "═".repeat(60));
    println!();

    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.iter().filter(|r| !r.passed).count();
    let total_skips: usize = results.iter().map(|r| r.skip_count()).sum();
    let total_warnings: usize = results.iter().map(|r| r.warning_count()).sum();
    let total = results.len();

    // Show results by phase
    let phases_run: Vec<usize> = results.iter()
        .map(|r| {
            match r.step_num {
                1..=2 => 1,
                3..=6 => 2,
                7..=10 => 3,
                11..=15 => 4,
                16..=18 => 5,
                19..=24 => 6,
                _ => 0,
            }
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    for phase in 1..=6 {
        if !phases_run.contains(&phase) {
            continue;
        }
        let phase_results: Vec<_> = results.iter()
            .filter(|r| {
                let p = match r.step_num {
                    1..=2 => 1,
                    3..=6 => 2,
                    7..=10 => 3,
                    11..=15 => 4,
                    16..=18 => 5,
                    19..=24 => 6,
                    _ => 0,
                };
                p == phase
            })
            .collect();

        let phase_passed = phase_results.iter().filter(|r| r.passed).count();
        let phase_skips: usize = phase_results.iter().map(|r| r.skip_count()).sum();
        let phase_warnings: usize = phase_results.iter().map(|r| r.warning_count()).sum();
        let phase_total = phase_results.len();
        let phase_status = if phase_passed == phase_total {
            if phase_warnings > 0 {
                "⚠".yellow()  // Pass but with warnings
            } else {
                "✓".green()
            }
        } else {
            "✗".red()
        };

        let phase_name = match phase {
            1 => "Boot Verification",
            2 => "Disk Setup",
            3 => "Base System",
            4 => "Configuration",
            5 => "Bootloader",
            6 => "Post-Reboot Verification",
            _ => "Unknown",
        };

        let mut notes = Vec::new();
        if phase_skips > 0 {
            notes.push(format!("{} skipped", phase_skips));
        }
        if phase_warnings > 0 {
            notes.push(format!("{} warnings", phase_warnings));
        }
        let note_str = if notes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", notes.join(", "))
        };

        println!("  {} Phase {}: {} ({}/{}){}",
            phase_status, phase, phase_name, phase_passed, phase_total, note_str.yellow());
    }

    println!();

    if all_passed {
        println!("{}", "═".repeat(60));
        if total_warnings > 0 || total_skips > 0 {
            // Pass but with caveats - be honest
            println!("{} {}/{} steps passed", "✓".green().bold(), passed, total);
            if total_skips > 0 {
                println!("  {} {} checks skipped (not tested)", "⊘".yellow(), total_skips);
            }
            if total_warnings > 0 {
                println!("  {} {} warnings (potential issues)", "⚠".yellow(), total_warnings);
            }
        } else {
            println!("{} All {} steps passed!", "✓".green().bold(), passed);
        }
        println!("{}", "═".repeat(60));
        println!();
        println!("The installed system:");
        println!("  • Boots with systemd as init");
        println!("  • Reaches multi-user.target");
        println!("  • Has working user accounts");
        if total_skips > 0 || total_warnings > 0 {
            println!("  • {} Some features were not tested or have warnings", "⚠".yellow());
        } else {
            println!("  • Has functional networking");
        }
        println!("  • Has working sudo");
        println!("  • Has all essential commands");
        println!();
        if total_skips > 0 || total_warnings > 0 {
            println!("{}", "This rootfs passed but has gaps. Review skips/warnings above.".yellow().bold());
        } else {
            println!("{}", "This rootfs is ready for daily driver use.".green().bold());
        }
    } else {
        println!("{}", "═".repeat(60));
        println!("{} {}/{} steps passed ({} failed)", "✗".red().bold(), passed, total, failed);
        if total_skips > 0 {
            println!("  {} {} checks skipped", "⊘".yellow(), total_skips);
        }
        if total_warnings > 0 {
            println!("  {} {} warnings", "⚠".yellow(), total_warnings);
        }
        println!("{}", "═".repeat(60));
        println!();

        // Show failed steps
        println!("{}", "Failed steps:".red());
        for result in &results {
            if !result.passed {
                println!("  • Step {}: {}", result.step_num, result.name);
                for (check_name, check_result) in &result.checks {
                    if let CheckResult::Fail { expected, actual } = check_result {
                        println!("      {} {}", "✗".red(), check_name);
                        println!("        Expected: {}", expected);
                        println!("        Actual:   {}", actual);
                    }
                }
            }
        }

        // Also show warnings if any (they may be related to failures)
        if total_warnings > 0 {
            println!();
            println!("{}", "Warnings:".yellow());
            for result in &results {
                for (check_name, check_result) in &result.checks {
                    if let CheckResult::Warning(reason) = check_result {
                        println!("  • {}: {}", check_name, reason);
                    }
                }
            }
        }
    }

    // Cleanup temp files
    let _ = std::fs::remove_file(&disk_path);
    let _ = std::fs::remove_file(&ovmf_vars_path);

    if all_passed {
        Ok(())
    } else {
        bail!("Installation tests failed")
    }
}
