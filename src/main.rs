//! E2E Installation Test Runner for LevitateOS.
//!
//! Runs installation steps in QEMU and verifies each step completes correctly.

mod qemu;
mod steps;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use qemu::{find_ovmf, create_disk, QemuBuilder};
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
        /// Run only a specific step (1-16)
        #[arg(long)]
        step: Option<usize>,

        /// Run only steps in a specific phase (1-5)
        #[arg(long)]
        phase: Option<usize>,

        /// Path to leviso directory (default: ../leviso)
        #[arg(long, default_value = "../leviso")]
        leviso_dir: PathBuf,

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
        Commands::Run { step, phase, leviso_dir, disk_size, keep_vm } => {
            run_tests(step, phase, &leviso_dir, &disk_size, keep_vm)
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

    let steps = all_steps();
    let mut current_phase = 0;

    for step in steps {
        if step.phase() != current_phase {
            current_phase = step.phase();
            println!();
            println!("{}", format!("Phase {}", current_phase).blue().bold());
        }
        println!("  {:2}. {}", step.num(), step.name());
    }
    println!();
}

fn run_tests(
    step_num: Option<usize>,
    phase_num: Option<usize>,
    leviso_dir: &PathBuf,
    disk_size: &str,
    _keep_vm: bool,
) -> Result<()> {
    println!("{}", "LevitateOS E2E Installation Tests".bold());
    println!();

    // Validate leviso directory
    let kernel_path = leviso_dir.join("downloads/iso-contents/images/pxeboot/vmlinuz");
    let initramfs_path = leviso_dir.join("output/initramfs.cpio.gz");

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

    println!("  Kernel:    {}", kernel_path.display());
    println!("  Initramfs: {}", initramfs_path.display());

    // Find OVMF for UEFI boot
    let ovmf = find_ovmf().context("OVMF not found - UEFI boot required for installation tests")?;
    println!("  OVMF:      {}", ovmf.display());

    // Create test disk
    let disk_path = std::env::temp_dir().join("leviso-install-test.qcow2");
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    create_disk(&disk_path, disk_size)?;
    println!("  Disk:      {} ({})", disk_path.display(), disk_size);
    println!();

    // Build QEMU command
    let mut cmd = QemuBuilder::new()
        .kernel(kernel_path)
        .initrd(initramfs_path)
        .append("console=tty0 console=ttyS0,115200n8 rdinit=/init panic=30")
        .disk(disk_path.clone())
        .uefi(ovmf)
        .nographic()
        .no_reboot()
        .build_piped();

    // Spawn QEMU
    println!("{}", "Starting QEMU...".cyan());
    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;

    // Create console controller
    let mut console = qemu::Console::new(&mut child)?;

    // Wait for boot
    println!("{}", "Waiting for boot...".cyan());
    console.wait_for_boot(Duration::from_secs(120))?;
    println!("{}", "System booted!".green());
    println!();

    // Determine which steps to run
    let steps_to_run: Vec<Box<dyn Step>> = match (step_num, phase_num) {
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

    if steps_to_run.is_empty() {
        bail!("No steps match the specified criteria");
    }

    // Run steps
    let mut results: Vec<StepResult> = Vec::new();
    let mut all_passed = true;

    for step in steps_to_run {
        print!("{} Step {:2}: {}... ",
            "▶".cyan(),
            step.num(),
            step.name()
        );

        let start = Instant::now();
        match step.execute(&mut console) {
            Ok(result) => {
                let duration = start.elapsed();
                if result.passed {
                    println!("{} ({:.1}s)", "PASS".green().bold(), duration.as_secs_f64());
                } else {
                    println!("{} ({:.1}s)", "FAIL".red().bold(), duration.as_secs_f64());
                    all_passed = false;

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

                    // Stop on first failure
                    results.push(result);
                    break;
                }
                results.push(result);
            }
            Err(e) => {
                println!("{}", "ERROR".red().bold());
                println!("    {}", e);
                all_passed = false;
                break;
            }
        }
    }

    // Print summary
    println!();
    println!("{}", "━".repeat(60));
    println!();

    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();

    if all_passed {
        println!("{} All {} steps passed!", "✓".green().bold(), passed);
    } else {
        println!("{} {}/{} steps passed", "✗".red().bold(), passed, total);
    }

    // Cleanup
    drop(console);
    let _ = child.kill();
    let _ = child.wait();

    // Remove test disk
    let _ = std::fs::remove_file(&disk_path);

    if all_passed {
        Ok(())
    } else {
        bail!("Installation tests failed")
    }
}
