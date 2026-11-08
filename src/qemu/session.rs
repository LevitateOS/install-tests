//! The one launcher of scenario VMs, for `cargo xtask test` and `cargo xtask
//! run` alike: libvirt domains (see `recqemu::libvirt`) named
//! `<distro>-<scenario>`, on the host's OVMF, with every input explicit. A
//! [`Vm`] destroys its domain when dropped (so no error path leaks a VM)
//! unless it is handed over with [`Vm::keep_running`]. While it runs, the VM
//! is visible in Cockpit's Virtual Machines page (the user session) and in
//! `virsh -c qemu:///session list`.

use crate::boot_injection::BootInjection;
use crate::qemu::Console;
use anyhow::{Context, Result};
use distro_contract::{ScenarioId, Variant};
use recqemu::libvirt::{runtime_dir, Domain, DomainSpec, SerialTarget, UefiVars};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the serial socket may take to appear after the domain is created.
const SERIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// The UEFI variable store a VM boots with.
#[derive(Default)]
pub enum Vars<'a> {
    /// A fresh store in the runtime directory, for a boot whose boot entries
    /// do not matter (a live ISO).
    #[default]
    Temporary,
    /// A fresh store at this path, which stays after the VM (an install's).
    FreshAt(&'a Path),
    /// An existing store the guest keeps writing (an installed system's).
    Existing(&'a Path),
}

/// The forward of a host port to the guest's sshd.
#[derive(Default, Clone, Copy)]
pub enum Ssh {
    #[default]
    None,
    /// A free port.
    Free,
    Port(u16),
}

/// Where the serial port goes.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Serial {
    /// A socket this process reads ([`Vm::console`]).
    #[default]
    Harness,
    /// A pty, for `virsh console` on the user's terminal.
    Terminal,
}

/// What to boot.
pub struct VmSpec<'a> {
    pub variant: &'a Variant,
    /// The domain is named `<distro>-<scenario>`.
    pub scenario: ScenarioId,
    /// Live ISO to boot from (first in the boot order).
    pub iso: Option<&'a Path>,
    /// Disk to install to or boot from.
    pub disk: Option<&'a Path>,
    pub vars: Vars<'a>,
    pub ssh: Ssh,
    /// An image attached as a CD the guest can read but does not boot.
    pub media: Option<&'a Path>,
    /// Settings for the live init; only live boots read them.
    pub injection: Option<&'a BootInjection>,
    pub serial: Serial,
}

impl<'a> VmSpec<'a> {
    pub fn new(variant: &'a Variant, scenario: ScenarioId) -> Self {
        Self {
            variant,
            scenario,
            iso: None,
            disk: None,
            vars: Vars::Temporary,
            ssh: Ssh::None,
            media: None,
            injection: None,
            serial: Serial::Harness,
        }
    }
}

/// A running VM; dropping it destroys the domain.
pub struct Vm {
    domain: Domain,
    /// The serial console, when this process reads it ([`Serial::Harness`]).
    pub console: Option<Console>,
    ssh_port: Option<u16>,
}

impl Vm {
    /// The serial console; the spec must have given it to the harness.
    pub fn console(&mut self) -> Result<&mut Console> {
        self.console
            .as_mut()
            .context("this VM's serial port goes to a terminal, not the harness")
    }

    /// The forwarded SSH port; the spec must have asked for one.
    pub fn ssh_port(&self) -> Result<u16> {
        self.ssh_port.context("this VM was started without SSH forwarding")
    }

    pub fn domain(&self) -> &Domain {
        &self.domain
    }

    /// Wait for the domain to end on its own, as it does when the guest
    /// reboots or powers off.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<()> {
        self.domain.wait_for_shutdown(timeout)
    }

    /// Type `command` (`poweroff`, `reboot`) into the guest's serial shell and
    /// wait for the domain to end; it fails unless it ends within `timeout`.
    /// The shell's connection drops while the guest goes down, so there is no
    /// command result to read.
    pub fn shutdown_from_guest(&mut self, command: &str, timeout: Duration) -> Result<()> {
        self.console()?
            .send(&format!("{command}\n"))
            .with_context(|| format!("typing '{command}' into the guest"))?;
        self.wait_for_exit(timeout)
            .with_context(|| format!("the guest did not end after '{command}'"))
    }

    /// Leave the VM running after this process lets go of it (Cockpit and
    /// `virsh` still manage it).
    pub fn keep_running(self) {
        self.domain.keep_running();
    }
}

/// Boot a VM: UEFI on the host's OVMF, the serial port as the spec says, a
/// VNC screen for Cockpit, and the injection medium when one is given.
pub fn spawn(spec: VmSpec<'_>) -> Result<Vm> {
    let name = format!("{}-{}", spec.variant.id, spec.scenario.key());
    let dir = runtime_dir();
    let socket = dir.join(format!("{name}.serial"));
    let serial_log = dir.join(format!("{name}.serial.log"));
    // Keep the previous boot's transcript (a scenario boots one VM name many times).
    match fs::rename(&serial_log, serial_log.with_extension("log.prev")) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            return Err(err).with_context(|| format!("keeping the previous transcript '{}'", serial_log.display()));
        }
        _ => {}
    }
    let absolute = |path: &Path| -> Result<PathBuf> {
        fs::canonicalize(path).with_context(|| format!("resolving '{}'", path.display()))
    };
    let template = recqemu::find_ovmf_vars()?;
    let uefi_vars = match spec.vars {
        Vars::Temporary => UefiVars::Fresh {
            template,
            path: dir.join(format!("{name}.VARS.fd")),
        },
        Vars::FreshAt(path) => UefiVars::Fresh {
            template,
            path: path.to_path_buf(),
        },
        Vars::Existing(path) => UefiVars::Existing(absolute(path)?),
    };
    let ssh_port = match spec.ssh {
        Ssh::None => None,
        Ssh::Free => Some(allocate_local_port()?),
        Ssh::Port(port) => Some(port),
    };
    let mut extra_cdroms = Vec::new();
    if let Some(media) = spec.media {
        extra_cdroms.push(absolute(media)?);
    }
    if let Some(injection) = spec.injection {
        extra_cdroms.push(absolute(&injection.iso())?);
    }
    let domain_spec = DomainSpec {
        name: name.clone(),
        title: format!("Scenario VM {name}"),
        memory_mib: DomainSpec::DEFAULT_MEMORY_MIB,
        vcpus: DomainSpec::DEFAULT_VCPUS,
        uefi_code: recqemu::find_ovmf()?,
        uefi_vars,
        disk: spec.disk.map(absolute).transpose()?,
        boot_iso: spec.iso.map(absolute).transpose()?,
        extra_cdroms,
        ssh_forward: ssh_port,
        serial: match spec.serial {
            Serial::Harness => SerialTarget::Socket(socket.clone()),
            Serial::Terminal => SerialTarget::Pty,
        },
        serial_log,
    };

    let domain = Domain::create_paused(&domain_spec)?;
    let console = match spec.serial {
        Serial::Harness => Some(Console::connect(&socket, SERIAL_CONNECT_TIMEOUT)?),
        Serial::Terminal => None,
    };
    domain.resume()?;
    Ok(Vm {
        domain,
        console,
        ssh_port,
    })
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
