//! E2E Installation Test Runner (QMP Backend) for LevitateOS and AcornOS.
//!
//! QMP is for visual-only testing: smoke tests and screenshots.
//! For step-based verification, use `cargo run --bin install-tests`.
//!
//! # Limitations
//!
//! QMP cannot capture command output without OCR. It cannot honestly
//! implement the Executor trait. Use this for visual verification only.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::Path;
use std::time::Duration;

use install_tests::qemu::qmp::QmpClient;
use install_tests::{
    create_disk, find_ovmf, find_ovmf_vars, kill_stale_qemu_processes, QemuBuilder,
};

#[derive(Parser)]
#[command(name = "install-tests-qmp")]
#[command(about = "Visual smoke testing for LevitateOS (QMP backend — screenshots only)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Smoke test: boot ISO, type a command, capture screenshot
    Smoke {
        /// Path to ISO file
        #[arg(long)]
        iso: std::path::PathBuf,

        /// VNC display number for live viewing
        #[arg(long, default_value = "0")]
        vnc: u16,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    install_tests::enforce_policy_guard("install-tests qmp")?;

    match cli.command {
        Commands::Smoke { iso, vnc } => smoke_test(&iso, vnc),
    }
}

/// Smoke test: boot ISO, type a command, capture screenshot
fn smoke_test(iso_path: &Path, vnc_display: u16) -> Result<()> {
    println!("{}", "QMP Smoke Test".bold());
    println!();

    if !iso_path.exists() {
        bail!("ISO not found at {}", iso_path.display());
    }

    kill_stale_qemu_processes();

    let ovmf = find_ovmf().context("OVMF not found - UEFI boot required for installation tests")?;
    let ovmf_vars_template =
        find_ovmf_vars().context("OVMF_VARS not found - needed for EFI variable storage")?;
    let ovmf_vars_path = std::env::temp_dir().join("leviso-qmp-smoke-vars.fd");
    if ovmf_vars_path.exists() {
        std::fs::remove_file(&ovmf_vars_path)?;
    }
    std::fs::copy(&ovmf_vars_template, &ovmf_vars_path)?;

    let disk_path = std::env::temp_dir().join("leviso-qmp-smoke.qcow2");
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    create_disk(&disk_path, "10G")?;

    let qmp_socket = std::env::temp_dir().join("leviso-qmp-smoke.sock");
    if qmp_socket.exists() {
        std::fs::remove_file(&qmp_socket)?;
    }

    println!("  ISO: {}", iso_path.display());
    println!("  QMP socket: {}", qmp_socket.display());
    println!("  VNC: :{} (port {})", vnc_display, 5900 + vnc_display);
    println!();

    println!("{}", "Starting QEMU with QMP...".cyan());
    let mut cmd = QemuBuilder::new()
        .cdrom(iso_path.to_path_buf())
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

    println!("{}", "Connecting to QMP...".cyan());
    let mut qmp = QmpClient::connect(&qmp_socket)?;
    println!("{}", "QMP connected!".green());
    println!();

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

    let screenshot_path = "/tmp/qmp-smoke-boot.ppm";
    println!("{}", "Taking screenshot...".cyan());
    qmp.screendump(screenshot_path)?;
    println!("  Screenshot saved to: {}", screenshot_path);

    println!();
    println!("{}", "Typing 'echo hello' via QMP...".cyan());
    qmp.send_text("echo hello\n")?;
    std::thread::sleep(Duration::from_secs(2));

    let screenshot_path2 = "/tmp/qmp-smoke-after-echo.ppm";
    println!("{}", "Taking screenshot after command...".cyan());
    qmp.screendump(screenshot_path2)?;
    println!("  Screenshot saved to: {}", screenshot_path2);

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

    Ok(())
}
