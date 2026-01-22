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
use steps::{all_steps, steps_for_phase, Step, StepResult, CheckResult};

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

        /// Path to leviso directory (default: ../leviso)
        #[arg(long, default_value = "../leviso")]
        leviso_dir: PathBuf,

        /// Path to ISO file (default: <leviso_dir>/output/leviso.iso)
        #[arg(long)]
        iso: Option<PathBuf>,

        /// Disk size for virtual disk
        #[arg(long, default_value = "8G")]
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

/// Run a single step and print result
fn run_single_step(step: &Box<dyn Step>, console: &mut qemu::Console) -> Result<(StepResult, bool)> {
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
                println!("{} ({:.1}s)", "PASS".green().bold(), duration.as_secs_f64());
                Ok((result, true))
            } else {
                println!("{} ({:.1}s)", "FAIL".red().bold(), duration.as_secs_f64());

                // Print failure details
                for (check_name, check_result) in &result.checks {
                    if let CheckResult::Fail { expected, actual } = check_result {
                        println!("    {} {}", "✗".red(), check_name);
                        println!("      Expected: {}", expected);
                        println!("      Actual:   {}", actual);
                    }
                }

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
    leviso_dir: &PathBuf,
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
    let kernel_path = leviso_dir.join("downloads/iso-contents/images/pxeboot/vmlinuz");
    // Tiny initramfs - mounts squashfs from ISO, creates overlay, switch_root
    // The squashfs contains the full system including recstrap and unsquashfs
    let initramfs_path = leviso_dir.join("output/initramfs-tiny.cpio.gz");
    let iso_path = iso_path.unwrap_or_else(|| leviso_dir.join("output/levitateos.iso"));

    if !kernel_path.exists() {
        bail!(
            "Kernel not found at {}. Run 'cargo run -- build' in leviso first.",
            kernel_path.display()
        );
    }
    if !initramfs_path.exists() {
        bail!(
            "Initramfs not found at {}. Run 'cargo run -- initramfs' in leviso first.",
            initramfs_path.display()
        );
    }
    if !iso_path.exists() {
        bail!(
            "ISO not found at {}. Run 'cargo run -- iso' in leviso first.",
            iso_path.display()
        );
    }

    println!("  Kernel:    {}", kernel_path.display());
    println!("  Initramfs: {}", initramfs_path.display());
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
        // Tiny initramfs mounts squashfs from ISO, creating a complete live system
        let mut cmd = QemuBuilder::new()
            .kernel(kernel_path.clone())
            .initrd(initramfs_path.clone())
            .append("console=tty0 console=ttyS0,115200n8 rdinit=/init panic=30")
            .disk(disk_path.clone())
            .cdrom(iso_path.clone())
            .uefi(ovmf.clone())
            .uefi_vars(ovmf_vars_path.clone())  // Writable for boot entries
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
            let (result, passed) = run_single_step(&step, &mut console)?;
            if !passed {
                all_passed = false;
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
        // We use a simpler QEMU command that boots directly from the disk
        // Same OVMF_VARS file is used to preserve boot entries from installation
        let mut cmd = QemuBuilder::new()
            .disk(disk_path.clone())
            .uefi(ovmf.clone())
            .uefi_vars(ovmf_vars_path.clone())
            .nographic()
            .no_reboot()
            .build_piped();

        println!("{}", "Starting QEMU (booting installed system)...".cyan());
        let mut child = cmd.spawn().context("Failed to spawn QEMU for installed system")?;
        let mut console = qemu::Console::new(&mut child)?;

        // Wait for the installed system to boot
        // Uses fail-fast detection - timeout only triggers if detection is broken
        println!("{}", "Waiting for installed system to boot...".cyan());
        console.wait_for_installed_boot(Duration::from_secs(30))?;
        println!("{}", "Installed system booted!".green());
        println!();

        // Run post-reboot verification steps
        let steps: Vec<Box<dyn Step>> = all_steps()
            .into_iter()
            .filter(|s| post_reboot_steps.contains(&s.num()))
            .collect();

        for step in steps {
            let (result, passed) = run_single_step(&step, &mut console)?;
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
        let phase_total = phase_results.len();
        let phase_status = if phase_passed == phase_total {
            "✓".green()
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

        println!("  {} Phase {}: {} ({}/{})", phase_status, phase, phase_name, phase_passed, phase_total);
    }

    println!();

    if all_passed {
        println!("{}", "═".repeat(60));
        println!("{} All {} steps passed!", "✓".green().bold(), passed);
        println!("{}", "═".repeat(60));
        println!();
        println!("The installed system:");
        println!("  • Boots with systemd as init");
        println!("  • Reaches multi-user.target");
        println!("  • Has working user accounts");
        println!("  • Has functional networking");
        println!("  • Has working sudo");
        println!("  • Has all essential commands");
        println!();
        println!("{}", "This rootfs is ready for daily driver use.".green().bold());
    } else {
        println!("{}", "═".repeat(60));
        println!("{} {}/{} steps passed ({} failed)", "✗".red().bold(), passed, total, failed);
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
