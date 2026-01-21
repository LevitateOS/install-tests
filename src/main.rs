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
        /// Run only a specific step (1-17)
        #[arg(long)]
        step: Option<usize>,

        /// Run only steps in a specific phase (1-5)
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

    let steps = all_steps();
    let mut current_phase = 0;

    for step in steps {
        if step.phase() != current_phase {
            current_phase = step.phase();
            println!();
            println!("{}", format!("Phase {}", current_phase).blue().bold());
        }
        println!("  {:2}. {}", step.num(), step.name());
        println!("      ensures: {}", step.ensures());
    }
    println!();
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

    // Validate leviso directory
    let kernel_path = leviso_dir.join("downloads/iso-contents/images/pxeboot/vmlinuz");
    let initramfs_path = leviso_dir.join("output/initramfs.cpio.gz");
    let iso_path = iso_path.unwrap_or_else(|| leviso_dir.join("output/leviso.iso"));

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
        .cdrom(iso_path)
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
        println!();

        // Show verification of installed system
        println!("{}", "Verification of Installed System".cyan().bold());
        println!("{}", "━".repeat(60));

        // Show disk layout
        println!("\n{}", "Disk Layout (lsblk):".yellow());
        if let Ok(r) = console.exec("lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT /dev/vda", Duration::from_secs(5)) {
            for line in r.output.lines().filter(|l| !l.contains("echo") && !l.contains("root@")) {
                println!("  {}", line);
            }
        }

        // Show fstab
        println!("\n{}", "/mnt/etc/fstab:".yellow());
        if let Ok(r) = console.exec("cat /mnt/etc/fstab", Duration::from_secs(5)) {
            for line in r.output.lines().filter(|l| !l.contains("echo") && !l.contains("root@") && !l.is_empty()) {
                println!("  {}", line);
            }
        }

        // Show hostname
        println!("\n{}", "/mnt/etc/hostname:".yellow());
        if let Ok(r) = console.exec("cat /mnt/etc/hostname", Duration::from_secs(5)) {
            for line in r.output.lines().filter(|l| !l.contains("echo") && !l.contains("root@")) {
                if !line.trim().is_empty() {
                    println!("  {}", line.trim());
                }
            }
        }

        // Show users
        println!("\n{}", "Users in /mnt/etc/passwd (uid >= 1000):".yellow());
        if let Ok(r) = console.exec("grep -E ':[0-9]{4,}:' /mnt/etc/passwd", Duration::from_secs(5)) {
            for line in r.output.lines().filter(|l| !l.contains("echo") && !l.contains("root@") && !l.contains("grep")) {
                if !line.trim().is_empty() {
                    println!("  {}", line.trim());
                }
            }
        }

        // Show root entry too
        if let Ok(r) = console.exec("grep '^root:' /mnt/etc/passwd", Duration::from_secs(5)) {
            for line in r.output.lines().filter(|l| l.starts_with("root:")) {
                println!("  {}", line.trim());
            }
        }

        // Show boot entry if it exists
        println!("\n{}", "Boot loader entry (/mnt/boot/loader/entries/):".yellow());
        if let Ok(r) = console.exec("cat /mnt/boot/loader/entries/*.conf 2>/dev/null || echo 'No entries (bootloader not fully installed)'", Duration::from_secs(5)) {
            for line in r.output.lines().filter(|l| !l.contains("echo") && !l.contains("root@")) {
                if !line.trim().is_empty() {
                    println!("  {}", line.trim());
                }
            }
        }

        // Show installed system size
        println!("\n{}", "Installed system size:".yellow());
        if let Ok(r) = console.exec("du -sh /mnt", Duration::from_secs(30)) {
            // Look for lines with size format (e.g., "1.2G" or "500M")
            for line in r.output.lines() {
                let trimmed = line.trim();
                if (trimmed.contains("G\t") || trimmed.contains("M\t") || trimmed.contains("K\t")) && trimmed.contains("/mnt") {
                    println!("  {}", trimmed);
                }
            }
        }

        // Show key directories exist
        println!("\n{}", "Key directories in /mnt:".yellow());
        if let Ok(r) = console.exec("ls -la /mnt/ | head -20", Duration::from_secs(5)) {
            for line in r.output.lines().filter(|l| !l.contains("echo") && !l.contains("root@") && !l.contains("ls -la")) {
                if !line.trim().is_empty() {
                    println!("  {}", line);
                }
            }
        }

        // Re-enter chroot to test binaries
        println!("\n{}", "Binary Verification (running in chroot):".yellow());
        println!("{}", "━".repeat(60));

        // Re-mount for chroot
        let _ = console.exec("mount --bind /dev /mnt/dev", Duration::from_secs(5));
        let _ = console.exec("mount --bind /proc /mnt/proc", Duration::from_secs(5));
        let _ = console.exec("mount --bind /sys /mnt/sys", Duration::from_secs(5));

        // Test various binaries
        let binaries_to_test = [
            ("bash --version | head -1", "bash"),
            ("ls --version | head -1", "coreutils (ls)"),
            ("cat --version | head -1", "coreutils (cat)"),
            ("grep --version | head -1", "grep"),
            ("sed --version | head -1", "sed"),
            ("awk --version | head -1", "awk"),
            ("tar --version | head -1", "tar"),
            ("gzip --version | head -1", "gzip"),
            ("find --version | head -1", "findutils"),
            ("systemctl --version | head -1", "systemd"),
            ("journalctl --version | head -1", "systemd (journalctl)"),
            ("useradd --version 2>&1 | head -1", "shadow-utils (useradd)"),
            ("sudo --version | head -1", "sudo"),
            ("su --version | head -1", "util-linux (su)"),
            ("mount --version | head -1", "util-linux (mount)"),
            ("fdisk --version | head -1", "util-linux (fdisk)"),
            ("ip --version 2>&1 | head -1", "iproute2"),
            ("ss --version 2>&1 | head -1", "iproute2 (ss)"),
        ];

        for (cmd, name) in binaries_to_test {
            let chroot_cmd = format!("chroot /mnt /bin/bash -c '{}'", cmd);
            if let Ok(r) = console.exec(&chroot_cmd, Duration::from_secs(5)) {
                // Extract the version line
                let version_line = r.output
                    .lines()
                    .filter(|l| !l.contains("chroot") && !l.contains("root@") && !l.contains("echo") && !l.trim().is_empty())
                    .filter(|l| !l.starts_with('>'))
                    .next()
                    .unwrap_or("");

                if version_line.contains("not found") || version_line.contains("No such file") {
                    println!("  {} {}: {}", "✗".red(), name, "NOT INSTALLED".red());
                } else if r.exit_code == 0 && !version_line.is_empty() {
                    println!("  {} {}: {}", "✓".green(), name, version_line.trim());
                } else if !version_line.is_empty() {
                    // Got output but non-zero exit (e.g., --version not supported)
                    println!("  {} {}: {}", "✓".green(), name, "(installed, version check failed)");
                } else {
                    println!("  {} {}: {}", "?".yellow(), name, "unknown");
                }
            }
        }

        // Test sudo specifically - try to run a command as root
        println!("\n{}", "Sudo functionality test:".yellow());
        let sudo_test = console.exec(
            "chroot /mnt /bin/bash -c 'echo \"levitate ALL=(ALL) NOPASSWD: ALL\" > /etc/sudoers.d/levitate && chmod 440 /etc/sudoers.d/levitate && su - levitate -c \"sudo whoami\"'",
            Duration::from_secs(10),
        );
        if let Ok(r) = sudo_test {
            let output = r.output.lines()
                .filter(|l| l.trim() == "root")
                .next();
            if output.is_some() {
                println!("  {} sudo works: 'sudo whoami' returns 'root'", "✓".green());
            } else {
                println!("  {} sudo test: {}", "?".yellow(), r.output.lines().last().unwrap_or("unknown"));
            }
        }

        // Test systemd can list units
        println!("\n{}", "Systemd functionality test:".yellow());
        if let Ok(r) = console.exec(
            "chroot /mnt /bin/bash -c 'systemctl list-unit-files --type=service 2>/dev/null | head -10'",
            Duration::from_secs(10),
        ) {
            let lines: Vec<_> = r.output.lines()
                .filter(|l| !l.contains("chroot") && !l.contains("root@") && !l.contains("echo"))
                .filter(|l| l.contains(".service"))
                .take(5)
                .collect();
            if !lines.is_empty() {
                println!("  {} systemctl list-unit-files works:", "✓".green());
                for line in lines {
                    println!("    {}", line.trim());
                }
            }
        }

        // Cleanup chroot mounts
        let _ = console.exec("umount -l /mnt/sys 2>/dev/null", Duration::from_secs(5));
        let _ = console.exec("umount -l /mnt/proc 2>/dev/null", Duration::from_secs(5));
        let _ = console.exec("umount -l /mnt/dev 2>/dev/null", Duration::from_secs(5));

        println!("\n{}", "━".repeat(60));
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
