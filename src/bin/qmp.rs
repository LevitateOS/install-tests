//! E2E Installation Test Runner (QMP Backend) for LevitateOS and AcornOS.
//!
//! Runs installation steps in QEMU using QMP (QEMU Machine Protocol) for I/O.
//! This backend emulates real user experience: keystrokes and screenshots.
//!
//! # When to Use QMP vs Serial
//!
//! Use this backend when:
//! - Testing graphical installers
//! - Validating user experience (exact key inputs)
//! - Visual regression testing
//! - Debugging display/input issues
//!
//! Use the serial backend (`--bin serial`) for:
//! - CI/CD pipelines (faster)
//! - Quick iteration
//! - Text-based verification
//!
//! # Limitations
//!
//! QMP cannot easily capture command output without OCR. Exit codes are
//! assumed to be successful. For reliable command verification, use serial.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use install_tests::{
    acquire_test_lock, all_steps, context_for_distro, create_disk, find_ovmf, find_ovmf_vars,
    kill_stale_qemu_processes, require_preflight, steps_for_phase, DistroContext, Executor,
    QemuBuilder, AVAILABLE_DISTROS,
};
use install_tests::qemu::qmp::QmpClient;

#[derive(Parser)]
#[command(name = "install-tests-qmp")]
#[command(about = "E2E installation test runner for LevitateOS (QMP backend)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run installation tests using QMP
    Run {
        /// Run only a specific step (1-24)
        #[arg(long)]
        step: Option<usize>,

        /// Run only steps in a specific phase (1-6)
        #[arg(long)]
        phase: Option<usize>,

        /// Distro to test (levitate or acorn)
        #[arg(long, default_value = "levitate")]
        distro: String,

        /// Path to ISO file (default: distro-specific path)
        #[arg(long)]
        iso: Option<PathBuf>,

        /// Disk size for virtual disk (20G matches production requirements)
        #[arg(long, default_value = "20G")]
        disk_size: String,

        /// VNC display number for optional live viewing (default: none)
        #[arg(long)]
        vnc: Option<u16>,

        /// Directory to save screenshots
        #[arg(long, default_value = "/tmp/qmp-screenshots")]
        screenshot_dir: PathBuf,
    },

    /// Smoke test: boot ISO and type a command
    Smoke {
        /// Path to ISO file
        #[arg(long)]
        iso: PathBuf,

        /// VNC display number for live viewing
        #[arg(long, default_value = "0")]
        vnc: u16,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            step,
            phase,
            distro,
            iso,
            disk_size,
            vnc,
            screenshot_dir,
        } => {
            let ctx = context_for_distro(&distro).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown distro '{}'. Available: {}",
                    distro,
                    AVAILABLE_DISTROS.join(", ")
                )
            })?;
            run_tests_qmp(
                step,
                phase,
                Arc::from(ctx),
                iso,
                &disk_size,
                vnc,
                &screenshot_dir,
            )
        }
        Commands::Smoke { iso, vnc } => smoke_test(&iso, vnc),
    }
}

/// Smoke test: boot ISO, type a command, capture screenshot
fn smoke_test(iso_path: &PathBuf, vnc_display: u16) -> Result<()> {
    println!("{}", "QMP Smoke Test".bold());
    println!();

    if !iso_path.exists() {
        bail!("ISO not found at {}", iso_path.display());
    }

    // Find OVMF for UEFI boot
    let ovmf =
        find_ovmf().context("OVMF not found - UEFI boot required for installation tests")?;
    println!("  OVMF: {}", ovmf.display());

    // Find OVMF_VARS template
    let ovmf_vars_template =
        find_ovmf_vars().context("OVMF_VARS not found - needed for EFI variable storage")?;
    let ovmf_vars_path = std::env::temp_dir().join("leviso-qmp-smoke-vars.fd");
    if ovmf_vars_path.exists() {
        std::fs::remove_file(&ovmf_vars_path)?;
    }
    std::fs::copy(&ovmf_vars_template, &ovmf_vars_path)?;

    // Create test disk
    let disk_path = std::env::temp_dir().join("leviso-qmp-smoke.qcow2");
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    create_disk(&disk_path, "10G")?;

    // QMP socket path
    let qmp_socket = std::env::temp_dir().join("leviso-qmp-smoke.sock");
    if qmp_socket.exists() {
        std::fs::remove_file(&qmp_socket)?;
    }

    println!("  ISO: {}", iso_path.display());
    println!("  QMP socket: {}", qmp_socket.display());
    println!("  VNC: :{} (port {})", vnc_display, 5900 + vnc_display);
    println!();

    // Build QEMU command for QMP mode
    println!("{}", "Starting QEMU with QMP...".cyan());
    let mut cmd = QemuBuilder::new()
        .cdrom(iso_path.clone())
        .disk(disk_path.clone())
        .uefi(ovmf)
        .uefi_vars(ovmf_vars_path.clone())
        .boot_order("dc")
        .qmp_socket(qmp_socket.clone())
        .vnc_display(vnc_display)
        .no_reboot()
        .build_qmp();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    println!("{}", "QEMU started!".green());

    // Wait for QMP socket to be ready
    println!("{}", "Waiting for QMP socket...".cyan());
    for _ in 0..50 {
        if qmp_socket.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if !qmp_socket.exists() {
        let _ = child.kill();
        let _ = child.wait();
        bail!("QMP socket not created after 5 seconds");
    }

    // Connect QMP client
    println!("{}", "Connecting to QMP...".cyan());
    let mut qmp = QmpClient::connect(&qmp_socket)?;
    println!("{}", "QMP connected!".green());
    println!();

    // Wait for system to boot (rough timing since we can't read screen)
    println!("{}", "Waiting for boot (30 seconds)...".cyan());
    println!(
        "{}",
        format!(
            "Connect VNC viewer to localhost:{} to watch",
            5900 + vnc_display
        )
        .yellow()
    );
    std::thread::sleep(Duration::from_secs(30));

    // Take a screenshot
    let screenshot_path = "/tmp/qmp-smoke-boot.ppm";
    println!("{}", "Taking screenshot...".cyan());
    qmp.screendump(screenshot_path)?;
    println!("  Screenshot saved to: {}", screenshot_path);

    // Type a test command
    println!();
    println!("{}", "Typing 'echo hello' via QMP...".cyan());
    qmp.send_text("echo hello\n")?;
    std::thread::sleep(Duration::from_secs(2));

    // Take another screenshot
    let screenshot_path2 = "/tmp/qmp-smoke-after-echo.ppm";
    println!("{}", "Taking screenshot after command...".cyan());
    qmp.screendump(screenshot_path2)?;
    println!("  Screenshot saved to: {}", screenshot_path2);

    // Cleanup
    println!();
    println!("{}", "Shutting down...".cyan());
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&disk_path);
    let _ = std::fs::remove_file(&ovmf_vars_path);
    let _ = std::fs::remove_file(&qmp_socket);

    println!();
    println!("{}", "=".repeat(60));
    println!("{}", "Smoke test complete!".green().bold());
    println!("{}", "=".repeat(60));
    println!();
    println!("Check screenshots:");
    println!("  - {}", screenshot_path);
    println!("  - {}", screenshot_path2);
    println!();
    println!("Convert PPM to PNG: convert {} screenshot.png", screenshot_path);

    Ok(())
}

/// Run full installation tests using QMP backend
fn run_tests_qmp(
    step_num: Option<usize>,
    phase_num: Option<usize>,
    ctx: Arc<dyn DistroContext>,
    iso_path: Option<PathBuf>,
    disk_size: &str,
    vnc_display: Option<u16>,
    screenshot_dir: &PathBuf,
) -> Result<()> {
    println!(
        "{}",
        format!("{} E2E Installation Tests (QMP Backend)", ctx.name()).bold()
    );
    println!();
    println!(
        "{}",
        "WARNING: QMP backend cannot capture command output without OCR.".yellow()
    );
    println!(
        "{}",
        "For reliable verification, use the serial backend.".yellow()
    );
    println!();

    // Create screenshot directory
    std::fs::create_dir_all(screenshot_dir)?;

    // CRITICAL: Acquire exclusive lock and kill any stale QEMU processes
    println!(
        "{}",
        "Acquiring test lock and cleaning up stale processes...".cyan()
    );
    kill_stale_qemu_processes();
    let _lock = acquire_test_lock()?;
    println!("{}", "Lock acquired, no other tests running.".green());
    println!();

    // Validate ISO path
    let iso_path = iso_path.unwrap_or_else(|| {
        let default = ctx.default_iso_path();
        if default.is_relative() {
            std::env::current_dir().unwrap_or_default().join(default)
        } else {
            default
        }
    });

    if !iso_path.exists() {
        bail!(
            "ISO not found at {}. Build the {} ISO first.",
            iso_path.display(),
            ctx.name()
        );
    }

    // Run preflight verification to catch artifact issues BEFORE starting QEMU
    let iso_dir = iso_path.parent().unwrap_or(std::path::Path::new("."));
    require_preflight(iso_dir)?;

    // Find OVMF for UEFI boot
    let ovmf =
        find_ovmf().context("OVMF not found - UEFI boot required for installation tests")?;
    let ovmf_vars_template =
        find_ovmf_vars().context("OVMF_VARS not found - needed for EFI variable storage")?;
    let ovmf_vars_path = std::env::temp_dir().join("leviso-install-test-qmp-vars.fd");
    if ovmf_vars_path.exists() {
        std::fs::remove_file(&ovmf_vars_path)?;
    }
    std::fs::copy(&ovmf_vars_template, &ovmf_vars_path)?;

    // Create test disk
    let disk_path = std::env::temp_dir().join("leviso-install-test-qmp.qcow2");
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    create_disk(&disk_path, disk_size)?;

    // QMP socket
    let qmp_socket = std::env::temp_dir().join("leviso-install-test.qmp");
    if qmp_socket.exists() {
        std::fs::remove_file(&qmp_socket)?;
    }

    println!("  ISO:        {}", iso_path.display());
    println!("  OVMF:       {}", ovmf.display());
    println!("  Disk:       {} ({})", disk_path.display(), disk_size);
    println!("  QMP socket: {}", qmp_socket.display());
    if let Some(vnc) = vnc_display {
        println!("  VNC:        :{} (port {})", vnc, 5900 + vnc);
    }
    println!("  Screenshots: {}", screenshot_dir.display());
    println!();

    // Determine which steps to run
    let all_requested = match (step_num, phase_num) {
        (Some(n), _) => all_steps().into_iter().filter(|s| s.num() == n).collect(),
        (_, Some(p)) => steps_for_phase(p),
        (None, None) => all_steps(),
    };

    if all_requested.is_empty() {
        bail!("No steps match the specified criteria");
    }

    // Build QEMU command for QMP mode
    println!("{}", "Starting QEMU with QMP...".cyan());
    let mut builder = QemuBuilder::new()
        .cdrom(iso_path)
        .disk(disk_path.clone())
        .uefi(ovmf)
        .uefi_vars(ovmf_vars_path.clone())
        .boot_order("dc")
        .with_user_network()
        .qmp_socket(qmp_socket.clone())
        .no_reboot();

    if let Some(vnc) = vnc_display {
        builder = builder.vnc_display(vnc);
    }

    let mut child = builder.build_qmp().spawn().context("Failed to spawn QEMU")?;
    println!("{}", "QEMU started!".green());

    // Wait for QMP socket
    println!("{}", "Waiting for QMP socket...".cyan());
    for _ in 0..50 {
        if qmp_socket.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if !qmp_socket.exists() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&disk_path);
        let _ = std::fs::remove_file(&ovmf_vars_path);
        bail!("QMP socket not created after 5 seconds");
    }

    // Connect QMP client
    let mut qmp = QmpClient::connect(&qmp_socket)?;
    println!("{}", "QMP connected!".green());

    // Wait for boot
    println!("{}", "Waiting for boot...".cyan());
    qmp.wait_for_live_boot(Duration::from_secs(30))?;
    println!("{}", format!("{} live ISO booted!", ctx.name()).green());

    // Take initial screenshot
    let boot_screenshot = screenshot_dir.join("01-boot.ppm");
    qmp.screendump(boot_screenshot.to_str().unwrap())?;
    println!("  Screenshot: {}", boot_screenshot.display());
    println!();

    // Run steps
    println!(
        "{}",
        "Running installation steps (QMP mode - limited verification)...".cyan()
    );
    println!();

    for (i, step) in all_requested.iter().enumerate() {
        print!(
            "{} Step {:2}: {}... ",
            ">>".cyan(),
            step.num(),
            step.name()
        );

        // Execute step
        match step.execute(&mut qmp, &*ctx) {
            Ok(result) => {
                if result.passed {
                    println!("{}", "OK".green());
                } else {
                    println!("{}", "FAIL".red());
                    // Take screenshot on failure
                    let fail_screenshot = screenshot_dir.join(format!("fail-step-{:02}.ppm", step.num()));
                    let _ = qmp.screendump(fail_screenshot.to_str().unwrap());
                }
            }
            Err(e) => {
                println!("{}: {}", "ERROR".red(), e);
            }
        }

        // Periodic screenshots
        if (i + 1) % 5 == 0 {
            let periodic_screenshot = screenshot_dir.join(format!("progress-{:02}.ppm", i + 1));
            let _ = qmp.screendump(periodic_screenshot.to_str().unwrap());
        }
    }

    // Final screenshot
    let final_screenshot = screenshot_dir.join("final.ppm");
    qmp.screendump(final_screenshot.to_str().unwrap())?;
    println!();
    println!("  Final screenshot: {}", final_screenshot.display());

    // Cleanup
    println!();
    println!("{}", "Shutting down...".cyan());
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&disk_path);
    let _ = std::fs::remove_file(&ovmf_vars_path);
    let _ = std::fs::remove_file(&qmp_socket);

    println!();
    println!("{}", "=".repeat(60));
    println!("{}", "QMP test run complete".green().bold());
    println!("{}", "=".repeat(60));
    println!();
    println!("Screenshots saved to: {}", screenshot_dir.display());
    println!();
    println!(
        "{}",
        "Note: QMP cannot verify command output. Use serial backend for CI/CD.".yellow()
    );

    Ok(())
}
