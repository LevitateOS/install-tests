//! The VMs the scenarios boot, as libvirt domains (see `recqemu::libvirt`):
//! one spawn function, and a session that destroys its domain when dropped
//! (so no error path leaks a VM). While a scenario runs, its VM is visible
//! in Cockpit's Virtual Machines page (the user session) and in
//! `virsh -c qemu:///session list`.

use crate::boot_injection::{boot_injection_from_env, BootInjection};
use crate::qemu::Console;
use anyhow::{Context, Result};
use recqemu::libvirt::{runtime_dir, Domain, DomainSpec, SerialTarget, UefiVars};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the serial socket may take to appear after the domain is created.
const SERIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// What to boot.
#[derive(Default)]
pub struct VmSpec<'a> {
    /// The domain's name, `<distro>-<scenario>` (e.g. `levitate-install`).
    pub name: &'a str,
    /// Live ISO to boot from (first in the boot order).
    pub iso: Option<&'a Path>,
    /// Disk to install to or boot from.
    pub disk: Option<&'a Path>,
    /// Writable UEFI variable store (persists boot entries); without one the
    /// VM gets a fresh store.
    pub ovmf_vars: Option<&'a Path>,
    /// Forward a host port to the guest's sshd.
    pub ssh: bool,
    /// An image attached as a CD the guest can read but does not boot.
    pub media: Option<&'a Path>,
}

/// A running VM; dropping it destroys the domain (and then removes the
/// injection's temporary files, which QEMU reads while it runs).
pub struct Vm {
    domain: Domain,
    _injection: Option<BootInjection>,
    pub console: Console,
    /// Host port forwarded to the guest's sshd, when requested.
    pub ssh_port: Option<u16>,
    /// The serial transcript libvirt keeps.
    pub serial_log: PathBuf,
}

impl Vm {
    /// The forwarded SSH port; the spec must have asked for one.
    pub fn ssh_port(&self) -> Result<u16> {
        self.ssh_port.context("this VM was started without SSH forwarding")
    }

    /// Wait for the domain to end on its own, as it does when the guest
    /// reboots or powers off.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<()> {
        self.domain.wait_for_shutdown(timeout)
    }
}

/// Boot a VM: UEFI, the serial port on a socket this process reads, a VNC
/// screen for Cockpit, and the boot injection from the environment, if any.
pub fn spawn(spec: VmSpec<'_>) -> Result<Vm> {
    anyhow::ensure!(!spec.name.is_empty(), "a scenario VM needs a name");
    let dir = runtime_dir();
    let socket = dir.join(format!("{}.serial", spec.name));
    let serial_log = dir.join(format!("{}.serial.log", spec.name));
    // Keep the previous boot's transcript (a scenario boots one VM name many times).
    if serial_log.exists() {
        let _ = fs::rename(&serial_log, serial_log.with_extension("log.prev"));
    }
    let uefi_vars = match spec.ovmf_vars {
        Some(vars) => UefiVars::Existing(fs::canonicalize(vars)?),
        None => UefiVars::Fresh {
            template: recqemu::find_ovmf_vars()?,
            path: dir.join(format!("{}.VARS.fd", spec.name)),
        },
    };
    let ssh_port = if spec.ssh { Some(allocate_local_port()?) } else { None };
    let absolute = |path: Option<&Path>| path.map(fs::canonicalize).transpose();
    let mut domain_spec = DomainSpec {
        name: spec.name.to_string(),
        title: format!("Scenario VM {}", spec.name),
        uefi_code: recqemu::find_ovmf()?,
        uefi_vars,
        disk: absolute(spec.disk)?,
        boot_iso: absolute(spec.iso)?,
        extra_cdroms: absolute(spec.media)?.into_iter().collect(),
        fw_cfg: Vec::new(),
        ssh_forward: ssh_port,
        serial: SerialTarget::Socket(socket.clone()),
        serial_log: serial_log.clone(),
    };
    let injection = boot_injection_from_env()?;
    if let Some(injection) = &injection {
        injection.attach(&mut domain_spec);
    }

    let domain = Domain::create_paused(&domain_spec)?;
    let console = Console::connect(&socket, SERIAL_CONNECT_TIMEOUT)?;
    domain.resume()?;
    Ok(Vm {
        domain,
        _injection: injection,
        console,
        ssh_port,
        serial_log,
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

/// A free local port for the SSH forward. passt binds it right after, so
/// another process could take it in between; the domain then fails to start.
fn allocate_local_port() -> Result<u16> {
    let listener = TcpListener::bind((recqemu::HOSTFWD_ADDRESS, 0))
        .context("allocating local tcp port for SSH forward")?;
    Ok(listener
        .local_addr()
        .context("reading allocated local tcp port")?
        .port())
}
