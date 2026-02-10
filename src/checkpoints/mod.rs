//! Checkpoint-based development loop.
//!
//! Lightweight, incremental checkpoints that gate progression and give fast
//! feedback during development. Each checkpoint validates one thing.
//!
//! # Checkpoints
//!
//! 1. **Live Boot** — ISO boots in QEMU (login prompt or `___SHELL_READY___`)
//! 2. **Live Tools** — Expected binaries present in live environment
//! 3. **Installation** — Scripted install to disk succeeds
//! 4. **Installed Boot** — System boots from disk after install
//! 5. **Automated Login** — Harness can login and run commands
//! 6. **Daily Driver Tools** — All expected tools present on installed system

pub mod state;

use crate::distro::{context_for_distro, DistroContext};
use crate::qemu::{Console, QemuBuilder, SerialExecutorExt};
use anyhow::{bail, Context, Result};
use colored::Colorize;
use state::CheckpointState;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Run a single checkpoint for a distro.
pub fn run_checkpoint(distro_id: &str, checkpoint: u32) -> Result<bool> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;
    let iso_path = resolve_iso_path(&*ctx)?;

    let mut state = CheckpointState::load(distro_id);
    if !state.is_valid_for_iso(&iso_path) {
        println!(
            "{}",
            "ISO rebuilt since last run — resetting checkpoints.".yellow()
        );
        state.reset_for_iso(&iso_path);
        state.save(distro_id)?;
    }

    // Gating: checkpoint N requires N-1 to have passed
    if checkpoint > 1 && !state.has_passed(checkpoint - 1) {
        bail!(
            "Checkpoint {} is blocked: checkpoint {} has not passed yet.\n\
             Run: cargo run --bin checkpoints -- --distro {} --checkpoint {}",
            checkpoint,
            checkpoint - 1,
            distro_id,
            checkpoint - 1
        );
    }

    // Already passed?
    if state.has_passed(checkpoint) {
        println!(
            "{} Checkpoint {} already passed (use --reset to clear).",
            "[SKIP]".green(),
            checkpoint
        );
        return Ok(true);
    }

    println!(
        "{} Checkpoint {}: {}",
        ">>".cyan(),
        checkpoint,
        checkpoint_name(checkpoint)
    );

    let result = match checkpoint {
        1 => run_live_boot(&*ctx, &iso_path),
        2 => run_live_tools(&*ctx, &iso_path),
        3 => run_installation(&*ctx, &iso_path),
        4 => run_installed_boot(&*ctx, &iso_path),
        5 => run_automated_login(&*ctx, &iso_path),
        6 => run_daily_driver_tools(&*ctx, &iso_path),
        _ => bail!("Invalid checkpoint number: {} (valid: 1-6)", checkpoint),
    };

    match &result {
        Ok(evidence) => {
            state.record(checkpoint, true, evidence);
            state.save(distro_id)?;
            println!(
                "{} Checkpoint {} passed: {}",
                "[PASS]".green().bold(),
                checkpoint,
                evidence
            );
            Ok(true)
        }
        Err(e) => {
            state.record(checkpoint, false, &format!("{:#}", e));
            state.save(distro_id)?;
            print_failure(checkpoint, e);
            Ok(false)
        }
    }
}

/// Run all checkpoints up to `target` (inclusive).
pub fn run_up_to(distro_id: &str, target: u32) -> Result<bool> {
    for cp in 1..=target {
        if !run_checkpoint(distro_id, cp)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Print checkpoint status for a distro.
pub fn print_status(distro_id: &str) -> Result<()> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;
    let iso_path = resolve_iso_path(&*ctx);

    let state = CheckpointState::load(distro_id);
    let valid = iso_path
        .as_ref()
        .map(|p| state.is_valid_for_iso(p))
        .unwrap_or(false);

    println!("{} Checkpoint Status", ctx.name().bold());
    if !valid {
        println!(
            "{}",
            "  (stale — ISO rebuilt or missing, checkpoints will reset on next run)".yellow()
        );
    }
    println!();

    for cp in 1..=6u32 {
        let status = if state.has_passed(cp) {
            "[PASS]".green()
        } else if state.results.contains_key(&cp) {
            "[FAIL]".red()
        } else {
            "[    ]".dimmed()
        };
        println!("  {} {}: {}", status, cp, checkpoint_name(cp));
    }
    println!();
    println!(
        "  Highest passed: {}",
        state.highest_passed().to_string().bold()
    );
    Ok(())
}

/// Reset all checkpoint state for a distro.
pub fn reset_state(distro_id: &str) -> Result<()> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.checkpoints")
        .join(format!("{}.json", distro_id));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    println!("Checkpoints reset for {}.", distro_id);
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Checkpoint implementations
// ═══════════════════════════════════════════════════════════════════════════

/// Checkpoint 1: Live Boot — ISO boots in QEMU.
fn run_live_boot(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let (mut child, mut console) = spawn_live_qemu(ctx, iso_path)?;

    let result = console.wait_for_live_boot_with_context(Duration::from_secs(60), ctx);

    let evidence = match &result {
        Ok(()) => "Boot markers detected".to_string(),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!("{:#}", e));
        }
    };

    let _ = child.kill();
    let _ = child.wait();
    Ok(evidence)
}

/// Checkpoint 2: Live Tools — Expected binaries in live environment.
fn run_live_tools(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let (mut child, mut console) = spawn_live_qemu(ctx, iso_path)?;
    console.wait_for_live_boot_with_context(Duration::from_secs(60), ctx)?;

    // Check for key tools expected in the live environment
    let tools = live_tools_for(ctx.id());
    let mut missing = Vec::new();
    let mut found = Vec::new();

    for tool in &tools {
        let result = console.exec(
            &format!("which {} 2>/dev/null && echo FOUND", tool),
            Duration::from_secs(5),
        )?;
        if result.output.contains("FOUND") {
            found.push(*tool);
        } else {
            missing.push(*tool);
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    if !missing.is_empty() {
        bail!(
            "Missing tools in live environment: {}\nFound: {}",
            missing.join(", "),
            found.join(", ")
        );
    }

    Ok(format!(
        "All {} tools present: {}",
        found.len(),
        found.join(", ")
    ))
}

/// Checkpoint 3: Installation — Scripted install to disk.
fn run_installation(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    recqemu::create_disk(&disk_path, "20G")?;

    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars_template = recqemu::find_ovmf_vars().context("OVMF_VARS not found")?;
    let ovmf_vars = temp_ovmf_vars_path(ctx.id());
    if ovmf_vars.exists() {
        std::fs::remove_file(&ovmf_vars)?;
    }
    std::fs::copy(&ovmf_vars_template, &ovmf_vars)?;

    let mut cmd = QemuBuilder::new()
        .cdrom(iso_path.to_path_buf())
        .disk(disk_path.clone())
        .uefi(ovmf.clone())
        .uefi_vars(ovmf_vars.clone())
        .boot_order("dc")
        .with_user_network()
        .nographic()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let mut console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));

    console.wait_for_live_boot_with_context(Duration::from_secs(60), ctx)?;

    // Run the installation script
    let install_cmds = install_commands_for(ctx);
    let mut step_count = 0;
    for (desc, cmd_str) in &install_cmds {
        println!("    {} {}", "->".cyan(), desc);
        let result = console.exec(cmd_str, Duration::from_secs(120))?;
        if !result.success() {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "Installation step '{}' failed (exit {}): {}",
                desc,
                result.exit_code,
                result.output.trim()
            );
        }
        step_count += 1;
    }

    // Verify key artifacts exist
    let verify_cmds = [
        ("Root filesystem", "ls /mnt/sysroot/bin/busybox"),
        ("Boot partition", "ls /mnt/sysroot/boot/EFI"),
        ("fstab", "cat /mnt/sysroot/etc/fstab"),
    ];
    for (desc, cmd_str) in &verify_cmds {
        let result = console.exec(cmd_str, Duration::from_secs(10))?;
        if !result.success() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Verification '{}' failed: {}", desc, result.output.trim());
        }
    }

    let _ = console.exec("poweroff -f", Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();

    Ok(format!("{} install steps completed + verified", step_count))
}

/// Checkpoint 4: Installed Boot — Boot from disk after install.
fn run_installed_boot(ctx: &dyn DistroContext, _iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    if !disk_path.exists() {
        bail!(
            "No disk image found at {}. Checkpoint 3 (Installation) must pass first.",
            disk_path.display()
        );
    }

    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars = temp_ovmf_vars_path(ctx.id());
    if !ovmf_vars.exists() {
        bail!("No OVMF vars found. Checkpoint 3 (Installation) must pass first.");
    }

    // Boot from disk only — no ISO
    let mut cmd = QemuBuilder::new()
        .disk(disk_path.clone())
        .uefi(ovmf)
        .uefi_vars(ovmf_vars)
        .boot_order("c")
        .with_user_network()
        .nographic()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let mut console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));

    let result = console.wait_for_installed_boot_with_context(Duration::from_secs(90), ctx);

    match &result {
        Ok(()) => {
            let _ = child.kill();
            let _ = child.wait();
            Ok("Installed system boot markers detected".to_string())
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(anyhow::anyhow!("{:#}", e))
        }
    }
}

/// Checkpoint 5: Automated Login — Harness can login and execute commands.
fn run_automated_login(ctx: &dyn DistroContext, _iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars = temp_ovmf_vars_path(ctx.id());

    let mut cmd = QemuBuilder::new()
        .disk(disk_path)
        .uefi(ovmf)
        .uefi_vars(ovmf_vars)
        .boot_order("c")
        .with_user_network()
        .nographic()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let mut console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));

    console.wait_for_installed_boot_with_context(Duration::from_secs(90), ctx)?;

    // Attempt login
    console.login("root", ctx.default_password(), Duration::from_secs(15))?;

    // Verify shell works
    let result = console.exec("echo CHECKPOINT_LOGIN_OK", Duration::from_secs(5))?;
    let _ = child.kill();
    let _ = child.wait();

    if result.output.contains("CHECKPOINT_LOGIN_OK") {
        Ok("Login succeeded, shell functional".to_string())
    } else {
        bail!(
            "Login succeeded but shell not functional. Got: {}",
            result.output.trim()
        );
    }
}

/// Checkpoint 6: Daily Driver Tools — All expected tools present.
fn run_daily_driver_tools(ctx: &dyn DistroContext, _iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars = temp_ovmf_vars_path(ctx.id());

    let mut cmd = QemuBuilder::new()
        .disk(disk_path)
        .uefi(ovmf)
        .uefi_vars(ovmf_vars)
        .boot_order("c")
        .with_user_network()
        .nographic()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let mut console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));

    console.wait_for_installed_boot_with_context(Duration::from_secs(90), ctx)?;
    console.login("root", ctx.default_password(), Duration::from_secs(15))?;

    let tools = installed_tools_for(ctx.id());
    let mut missing = Vec::new();
    let mut found = Vec::new();

    for tool in &tools {
        let result = console.exec(
            &format!("which {} 2>/dev/null && echo FOUND", tool),
            Duration::from_secs(5),
        )?;
        if result.output.contains("FOUND") {
            found.push(*tool);
        } else {
            missing.push(*tool);
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    if !missing.is_empty() {
        bail!(
            "Missing daily driver tools: {}\nFound: {}",
            missing.join(", "),
            found.join(", ")
        );
    }

    Ok(format!("All {} daily driver tools present", found.len()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

fn checkpoint_name(n: u32) -> &'static str {
    match n {
        1 => "Live Boot",
        2 => "Live Tools",
        3 => "Installation",
        4 => "Installed Boot",
        5 => "Automated Login",
        6 => "Daily Driver Tools",
        _ => "Unknown",
    }
}

fn resolve_iso_path(ctx: &dyn DistroContext) -> Result<PathBuf> {
    let default = ctx.default_iso_path();
    let iso_path = if default.is_relative() {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(default)
    } else {
        default
    };
    if !iso_path.exists() {
        bail!(
            "ISO not found at {}. Build {} first: cd {} && cargo run -- build",
            iso_path.display(),
            ctx.name(),
            ctx.id()
        );
    }
    Ok(iso_path)
}

fn spawn_live_qemu(
    _ctx: &dyn DistroContext,
    iso_path: &Path,
) -> Result<(std::process::Child, Console)> {
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;

    let mut cmd = QemuBuilder::new()
        .cdrom(iso_path.to_path_buf())
        .uefi(ovmf)
        .nographic()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console))
}

fn temp_disk_path(distro_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("checkpoint-{}-disk.qcow2", distro_id))
}

fn temp_ovmf_vars_path(distro_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("checkpoint-{}-vars.fd", distro_id))
}

/// Tools expected in the live ISO environment.
fn live_tools_for(distro_id: &str) -> Vec<&'static str> {
    match distro_id {
        "acorn" | "acornos" => vec![
            "recstrap",
            "recfstab",
            "recchroot",
            "sfdisk",
            "mkfs.ext4",
            "mount",
        ],
        "iuppiter" | "iuppiteros" => vec![
            "recstrap",
            "recfstab",
            "recchroot",
            "sfdisk",
            "mkfs.ext4",
            "mount",
            "smartctl",
            "hdparm",
        ],
        _ => vec!["recstrap", "recfstab", "recchroot", "sfdisk", "mkfs.ext4"],
    }
}

/// Tools expected on the installed system.
fn installed_tools_for(distro_id: &str) -> Vec<&'static str> {
    match distro_id {
        "acorn" | "acornos" => vec![
            "sudo", "ip", "ssh", "ash", "mount", "umount", "dmesg", "ps", "ls", "cat",
        ],
        "iuppiter" | "iuppiteros" => vec![
            "sudo", "ip", "ssh", "ash", "smartctl", "hdparm", "sg_inq", "mount", "umount", "dmesg",
        ],
        _ => vec!["sudo", "ip", "ssh", "mount", "umount", "dmesg"],
    }
}

/// Installation commands for a distro. Returns (description, command) pairs.
fn install_commands_for(ctx: &dyn DistroContext) -> Vec<(&'static str, String)> {
    // Common install flow for Alpine-based distros
    vec![
        (
            "Partition disk",
            "echo 'label: gpt\nsize=512M, type=uefi\ntype=linux' | sfdisk /dev/sda".to_string(),
        ),
        (
            "Format EFI partition",
            "mkfs.fat -F32 /dev/sda1".to_string(),
        ),
        (
            "Format root partition",
            "mkfs.ext4 -F /dev/sda2".to_string(),
        ),
        (
            "Mount root",
            "mkdir -p /mnt/sysroot && mount /dev/sda2 /mnt/sysroot".to_string(),
        ),
        (
            "Mount boot",
            "mkdir -p /mnt/sysroot/boot && mount /dev/sda1 /mnt/sysroot/boot".to_string(),
        ),
        ("Extract rootfs", "recstrap /mnt/sysroot".to_string()),
        ("Generate fstab", "recfstab /mnt/sysroot".to_string()),
        (
            "Install bootloader",
            format!("recchroot /mnt/sysroot -- {}", ctx.install_bootloader_cmd()),
        ),
        (
            "Set hostname",
            format!(
                "echo {} > /mnt/sysroot/etc/hostname",
                ctx.default_hostname()
            ),
        ),
        (
            "Set root password",
            format!(
                "recchroot /mnt/sysroot -- sh -c 'echo root:{} | chpasswd'",
                ctx.default_password()
            ),
        ),
        (
            "Enable serial getty",
            format!(
                "recchroot /mnt/sysroot -- sh -c '{}'",
                ctx.enable_serial_getty_cmd()
            ),
        ),
        ("Unmount", "umount -R /mnt/sysroot".to_string()),
    ]
}

fn print_failure(checkpoint: u32, err: &anyhow::Error) {
    eprintln!();
    eprintln!(
        "{} Checkpoint {} FAILED: {}",
        "[FAIL]".red().bold(),
        checkpoint,
        checkpoint_name(checkpoint)
    );
    eprintln!();
    eprintln!("  Error: {:#}", err);
    eprintln!();

    match checkpoint {
        1 => {
            eprintln!("  Common causes:");
            eprintln!("    - ISO not built or corrupted");
            eprintln!("    - Kernel panic during boot");
            eprintln!("    - initramfs missing /init");
            eprintln!("    - UEFI firmware not finding boot entry");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Manual boot: cd <DistroDir> && cargo run -- run --serial");
            eprintln!("    - Rebuild: cd <DistroDir> && cargo run -- build");
        }
        2 => {
            eprintln!("  Common causes:");
            eprintln!("    - Tools not included in initramfs or rootfs");
            eprintln!("    - PATH not set correctly");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Check package list in distro-spec");
        }
        3 => {
            eprintln!("  Common causes:");
            eprintln!("    - recstrap/recfstab/recchroot broken");
            eprintln!("    - Disk too small or partition failure");
            eprintln!("    - Bootloader install failure");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Manual install: boot ISO, run install commands by hand");
        }
        4 => {
            eprintln!("  Common causes:");
            eprintln!("    - Bootloader not installed correctly");
            eprintln!("    - UKI not found by systemd-boot");
            eprintln!("    - Missing kernel modules");
        }
        5 => {
            eprintln!("  Common causes:");
            eprintln!("    - No serial getty enabled");
            eprintln!("    - Password not set correctly");
            eprintln!("    - Shell not functional");
        }
        6 => {
            eprintln!("  Common causes:");
            eprintln!("    - Packages not installed in rootfs");
            eprintln!("    - PATH misconfigured");
        }
        _ => {}
    }
    eprintln!();
}
