//! Shared QEMU session helpers for spawning live and installed system VMs.
//!
//! Eliminates duplicated QEMU setup code across stages and install-tests binaries.

use crate::distro::DistroContext;
use crate::qemu::{Console, QemuBuilder};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

/// Resolve the ISO path for a distro context.
pub fn resolve_iso(ctx: &dyn DistroContext) -> Result<PathBuf> {
    let default = ctx.default_iso_path();
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let iso_path = if default.is_relative() {
        workspace_root.join(default)
    } else {
        default
    };
    if !iso_path.exists() {
        bail!(
            "ISO not found at {}. Build {} Stage 00 first: \
             cargo run -p distro-builder --bin distro-builder -- iso build {} 00Build",
            iso_path.display(),
            ctx.name(),
            ctx.id()
        );
    }
    Ok(iso_path)
}

/// Set up OVMF firmware and writable vars copy. Returns (ovmf_code, ovmf_vars).
pub fn setup_ovmf_vars(distro_id: &str) -> Result<(PathBuf, PathBuf)> {
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars_template = recqemu::find_ovmf_vars().context("OVMF_VARS not found")?;
    let ovmf_vars = temp_ovmf_vars_path(distro_id);
    if ovmf_vars.exists() {
        std::fs::remove_file(&ovmf_vars)?;
    }
    std::fs::copy(&ovmf_vars_template, &ovmf_vars)?;
    Ok((ovmf, ovmf_vars))
}

/// Temp disk path for a distro's stage testing.
pub fn temp_disk_path(distro_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("stage-{}-disk.qcow2", distro_id))
}

/// Temp OVMF vars path for a distro's stage testing.
pub fn temp_ovmf_vars_path(distro_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("stage-{}-vars.fd", distro_id))
}

/// Spawn a QEMU VM booting from a live ISO (no disk attached).
pub fn spawn_live(_ctx: &dyn DistroContext, iso_path: &Path) -> Result<(Child, Console)> {
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;

    let mut cmd = QemuBuilder::new()
        .cdrom(iso_path.to_path_buf())
        .uefi(ovmf)
        .nographic()
        .serial_stdio()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console))
}

/// Spawn a QEMU VM booting from a live ISO with a disk attached (for installation).
pub fn spawn_live_with_disk(
    iso_path: &Path,
    disk_path: &Path,
    ovmf: &Path,
    ovmf_vars: &Path,
) -> Result<(Child, Console)> {
    let mut cmd = QemuBuilder::new()
        .cdrom(iso_path.to_path_buf())
        .disk(disk_path.to_path_buf())
        .uefi(ovmf.to_path_buf())
        .uefi_vars(ovmf_vars.to_path_buf())
        .boot_order("dc")
        .with_user_network()
        .nographic()
        .serial_stdio()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console))
}

/// Spawn a QEMU VM booting from an installed disk (no ISO).
pub fn spawn_installed(
    disk_path: &Path,
    ovmf: &Path,
    ovmf_vars: &Path,
) -> Result<(Child, Console)> {
    let mut cmd = QemuBuilder::new()
        .disk(disk_path.to_path_buf())
        .uefi(ovmf.to_path_buf())
        .uefi_vars(ovmf_vars.to_path_buf())
        .boot_order("c")
        .with_user_network()
        .nographic()
        .serial_stdio()
        .no_reboot()
        .build_piped();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let console = Console::new(&mut child)?;
    std::thread::sleep(Duration::from_secs(2));
    Ok((child, console))
}
