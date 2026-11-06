//! The QEMU VMs the scenarios boot: one spawn function, and a session that
//! stops its VM when dropped (so no error path leaks QEMU).

use crate::boot_injection::{boot_injection_from_env, BootInjection};
use crate::qemu::Console;
use anyhow::{Context, Result};
use recqemu::QemuBuilder;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Child;

/// What to boot.
#[derive(Default)]
pub struct VmSpec<'a> {
    /// Live ISO to boot from (first in the boot order).
    pub iso: Option<&'a Path>,
    /// Disk to install to or boot from.
    pub disk: Option<&'a Path>,
    /// Writable UEFI variable store (persists boot entries).
    pub ovmf_vars: Option<&'a Path>,
    /// Forward a host port to the guest's sshd.
    pub ssh: bool,
}

/// A running VM; dropping it stops QEMU (and then removes the injection's
/// temporary files, which QEMU reads while it runs).
pub struct Vm {
    child: Child,
    _injection: Option<BootInjection>,
    pub console: Console,
    /// Host port forwarded to the guest's sshd, when requested.
    pub ssh_port: Option<u16>,
}

impl Drop for Vm {
    fn drop(&mut self) {
        // The VM is being discarded; a kill/wait error leaves nothing to do.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Vm {
    /// The forwarded SSH port; the spec must have asked for one.
    pub fn ssh_port(&self) -> Result<u16> {
        self.ssh_port.context("this VM was started without SSH forwarding")
    }
}

/// Boot a VM with serial on piped stdio (UEFI, headless, no reboot), plus
/// the boot injection from the environment, if any.
pub fn spawn(spec: VmSpec<'_>) -> Result<Vm> {
    let mut builder = QemuBuilder::new()
        .nodefaults()
        .uefi(recqemu::find_ovmf()?)
        .nographic()
        .serial_stdio()
        .no_reboot();
    if let Some(iso) = spec.iso {
        builder = builder.cdrom(iso).cdrom_as_disk();
    }
    if let Some(disk) = spec.disk {
        builder = builder
            .disk(disk)
            .boot_order(if spec.iso.is_some() { "dc" } else { "c" });
    }
    if let Some(vars) = spec.ovmf_vars {
        builder = builder.uefi_vars(vars);
    }
    let ssh_port = if spec.ssh {
        let port = allocate_local_port()?;
        builder = builder.user_network_with_hostfwd(port, recqemu::GUEST_SSH_PORT);
        Some(port)
    } else {
        builder = builder.user_network();
        None
    };
    let injection = boot_injection_from_env()?;
    if let Some(injection) = &injection {
        builder = injection.attach(builder);
    }

    let mut child = builder.build_piped().spawn().context("Failed to spawn QEMU")?;
    let console = match Console::new(&mut child) {
        Ok(console) => console,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
    };
    Ok(Vm {
        child,
        _injection: injection,
        console,
        ssh_port,
    })
}

/// Copy the OVMF variable-store template to `ovmf_vars_path` (replacing it).
pub fn setup_ovmf_vars_at(ovmf_vars_path: &Path) -> Result<PathBuf> {
    let template = recqemu::find_ovmf_vars()?;
    if let Some(parent) = ovmf_vars_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating scenario runtime dir '{}'", parent.display()))?;
    }
    fs::copy(&template, ovmf_vars_path).with_context(|| {
        format!(
            "copying {} to {}",
            template.display(),
            ovmf_vars_path.display()
        )
    })?;
    Ok(ovmf_vars_path.to_path_buf())
}

/// A free local port for the SSH forward. QEMU binds it right after, so
/// another process could take it in between; QEMU then fails to start.
fn allocate_local_port() -> Result<u16> {
    let listener = TcpListener::bind((recqemu::HOSTFWD_ADDRESS, 0))
        .context("allocating local tcp port for SSH forward")?;
    Ok(listener
        .local_addr()
        .context("reading allocated local tcp port")?
        .port())
}
