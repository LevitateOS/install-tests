//! Shared QEMU session helpers for spawning live and installed system VMs.
//!
//! Eliminates duplicated QEMU setup code across scenario runners and install-tests binaries.

use crate::boot_injection::boot_injection_from_env;
use crate::distro::DistroContext;
use crate::qemu::Console;
use recqemu::QemuBuilder;
use anyhow::{Context, Result};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

/// Set up OVMF firmware and writable vars copy at a caller-provided path.
/// Returns (ovmf_code, ovmf_vars_copy).
pub fn setup_ovmf_vars_at(ovmf_vars_path: &Path) -> Result<(PathBuf, PathBuf)> {
    let ovmf = recqemu::find_ovmf()?;
    let ovmf_vars_template = recqemu::find_ovmf_vars()?;
    if let Some(parent) = ovmf_vars_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating scenario runtime dir '{}'", parent.display()))?;
    }
    if ovmf_vars_path.exists() {
        std::fs::remove_file(ovmf_vars_path)?;
    }
    std::fs::copy(&ovmf_vars_template, ovmf_vars_path)?;
    Ok((ovmf, ovmf_vars_path.to_path_buf()))
}

/// Spawn a QEMU VM booting from a live ISO (no disk attached).
pub fn spawn_live(_ctx: &dyn DistroContext, iso_path: &Path) -> Result<(Child, Console)> {
    let (child, console, _) = spawn_live_with_ssh(_ctx, iso_path)?;
    Ok((child, console))
}

/// Spawn a live ISO VM with host-side SSH forwarding enabled.
///
/// Returns the forwarded host port mapped to guest tcp/22.
pub fn spawn_live_with_ssh(
    _ctx: &dyn DistroContext,
    iso_path: &Path,
) -> Result<(Child, Console, u16)> {
    let ovmf = recqemu::find_ovmf()?;
    let ssh_host_port = allocate_local_port()?;

    let builder = QemuBuilder::new()
        .nodefaults()
        .cdrom(iso_path)
        .cdrom_as_disk()
        .uefi(ovmf)
        .user_network_with_hostfwd(ssh_host_port, GUEST_SSH_PORT)
        .nographic()
        .serial_stdio()
        .no_reboot();
    let mut cmd = with_boot_injection(builder)?.build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console, ssh_host_port))
}

/// Spawn a QEMU VM booting from a live ISO with a disk attached (for installation).
pub fn spawn_live_with_disk(
    iso_path: &Path,
    disk_path: &Path,
    ovmf: &Path,
    ovmf_vars: &Path,
) -> Result<(Child, Console)> {
    let builder = QemuBuilder::new()
        .nodefaults()
        .cdrom(iso_path)
        .cdrom_as_disk()
        .disk(disk_path.to_path_buf())
        .uefi(ovmf.to_path_buf())
        .uefi_vars(ovmf_vars.to_path_buf())
        .boot_order("dc")
        .user_network()
        .nographic()
        .serial_stdio()
        .no_reboot();
    let mut cmd = with_boot_injection(builder)?.build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console))
}

/// Spawn a QEMU VM booting from a live ISO with a disk attached and SSH forwarding.
///
/// Returns the forwarded host port mapped to guest tcp/22.
pub fn spawn_live_with_disk_with_ssh(
    iso_path: &Path,
    disk_path: &Path,
    ovmf: &Path,
    ovmf_vars: &Path,
) -> Result<(Child, Console, u16)> {
    let ssh_host_port = allocate_local_port()?;

    let builder = QemuBuilder::new()
        .nodefaults()
        .cdrom(iso_path)
        .cdrom_as_disk()
        .disk(disk_path.to_path_buf())
        .uefi(ovmf.to_path_buf())
        .uefi_vars(ovmf_vars.to_path_buf())
        .boot_order("dc")
        .user_network_with_hostfwd(ssh_host_port, GUEST_SSH_PORT)
        .nographic()
        .serial_stdio()
        .no_reboot();
    let mut cmd = with_boot_injection(builder)?.build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console, ssh_host_port))
}

/// Spawn a QEMU VM booting from an installed disk (no ISO).
pub fn spawn_installed(
    disk_path: &Path,
    ovmf: &Path,
    ovmf_vars: &Path,
) -> Result<(Child, Console)> {
    let builder = QemuBuilder::new()
        .nodefaults()
        .disk(disk_path)
        .uefi(ovmf.to_path_buf())
        .uefi_vars(ovmf_vars.to_path_buf())
        .boot_order("c")
        .user_network()
        .nographic()
        .serial_stdio()
        .no_reboot();
    let mut cmd = with_boot_injection(builder)?.build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console))
}

fn with_boot_injection(builder: QemuBuilder) -> Result<QemuBuilder> {
    let Some(injection) = boot_injection_from_env()? else {
        return Ok(builder);
    };
    let mut configured = builder.fw_cfg_file(&injection.fw_cfg_name, injection.payload_file);
    if let Some(media_iso_file) = injection.media_iso_file {
        configured = configured.extra_cdrom(media_iso_file);
    }
    Ok(configured)
}

/// The guest's sshd port.
const GUEST_SSH_PORT: u16 = 22;

fn allocate_local_port() -> Result<u16> {
    let listener = TcpListener::bind((recqemu::HOSTFWD_ADDRESS, 0)).context("allocating local tcp port for SSH forward")?;
    let port = listener
        .local_addr()
        .context("reading allocated local tcp port")?
        .port();
    drop(listener);
    Ok(port)
}
