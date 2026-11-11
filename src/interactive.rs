//! `cargo xtask run`: boot a scenario's input for a person, through the same
//! launcher, injection, boot judge and SSH login as the scenarios.

use crate::qemu::session::{self, Serial, Ssh, Vars, VmSpec};
use crate::qemu::wait_live_boot;
use crate::scenarios::{self, Harness, RunOptions, ScenarioId};
use anyhow::{Context, Result, bail};
use recqemu::libvirt::runtime_dir;
use std::path::Path;
use std::process::Command;

/// How the person gets the VM.
#[derive(Debug, Clone, Copy)]
pub enum Mode {
    /// The serial console on this terminal (`virsh console`); the live
    /// system runs its install screen there.
    Console,
    /// Wait for the boot and an SSH login, then an SSH session on this
    /// terminal (`shell`) or stop there. `port` is the host port forwarded
    /// to the guest's sshd (default: a free one).
    Ssh { port: Option<u16>, shell: bool },
    /// Wait for the boot, then stop: a check that needs no terminal.
    Check,
}

/// Boot `scenario`'s input (its ISO, or an overlay of the latest install
/// run's disk, so the install run stays as it is) for distro `distro_id`.
pub fn open(
    repo: &Path,
    distro_id: &str,
    scenario: ScenarioId,
    mode: Mode,
    options: &RunOptions,
) -> Result<()> {
    if !scenario.is_runnable() {
        bail!("{scenario} boots nothing of its own to open");
    }
    let installed = scenario.input_for(options.flavour).iso.is_none();
    if installed && !matches!(mode, Mode::Console) {
        bail!("{scenario} opens the installed system's serial console only");
    }
    let h = Harness::new(repo, distro_id, options, !installed)?;
    let variant = &h.variant;
    let serial = match mode {
        Mode::Console => Serial::Terminal,
        Mode::Ssh { .. } | Mode::Check => Serial::Harness,
    };

    let mut vm = if let Some(product) = scenario.input_for(options.flavour).iso {
        let iso = scenarios::resolve_release_selected(
            repo,
            distro_id,
            product,
            options.release_run.as_deref(),
        )?
        .iso;
        // On an interactive serial console the live system runs the install screen.
        let extra = match mode {
            Mode::Console => vec![format!("{}=1", distro_builder::install_ux::SERIAL_KEY)],
            _ => Vec::new(),
        };
        let injection = h.live_injection(&extra)?;
        eprintln!(
            "Booting {} {scenario} from {}...",
            variant.contract.identity.os_name,
            iso.display()
        );
        session::spawn(VmSpec {
            iso: Some(&iso),
            flavour: product.flavour(),
            injection: Some(&injection),
            ssh: match mode {
                Mode::Ssh {
                    port: Some(port), ..
                } => Ssh::Port(port),
                Mode::Ssh { port: None, .. } => Ssh::Free,
                _ => Ssh::None,
            },
            serial,
            ..VmSpec::new(variant, scenario)
        })?
    } else {
        let install = scenarios::resolve_install_selected(repo, distro_id, options)?;
        let dir = runtime_dir().join(format!("{distro_id}-{scenario}"));
        std::fs::create_dir_all(&dir).with_context(|| format!("creating '{}'", dir.display()))?;
        let (disk, vars) = scenarios::overlay_install(&install, &dir)?;
        eprintln!(
            "Booting the installed {} of install run {} (through an overlay in {})...",
            variant.contract.identity.os_name,
            install.run.run_id,
            dir.display()
        );
        session::spawn(VmSpec {
            disk: Some(&disk),
            flavour: install.flavour,
            vars: Vars::Existing(&vars),
            serial,
            ..VmSpec::new(variant, scenario)
        })?
    };
    let name = vm.domain().name().to_string();
    let uri = vm.domain().uri().to_string();
    eprintln!(
        "VM '{name}' runs on {uri}: watch it in Cockpit (Virtual Machines) or `virsh -c {uri} console {name}`."
    );

    match mode {
        Mode::Console => {
            eprintln!("Attaching the serial console (Ctrl+] to leave)...");
            let status = Command::new("virsh")
                .args(["-c", &uri, "console", "--force", &name])
                .status()
                .context("running virsh console")?;
            if vm.domain().is_running()? {
                eprintln!(
                    "Left the console; '{name}' keeps running. Stop it in Cockpit or with: virsh -c {uri} destroy {name}"
                );
                vm.keep_running();
            }
            if !status.success() {
                bail!("virsh console exited with {status}");
            }
            Ok(())
        }
        Mode::Check => {
            wait_live_boot(vm.console()?, variant)?;
            eprintln!("{name} booted.");
            Ok(())
        }
        Mode::Ssh { shell, .. } => {
            let port = vm.ssh_port()?;
            let mut remote = h.remote(port)?;
            wait_live_boot(vm.console()?, variant)?;
            scenarios::verify_live_boot_ssh_login(&h, &mut vm, &mut remote)?;
            eprintln!(
                "{name} takes SSH logins on {}:{port}.",
                recqemu::HOSTFWD_ADDRESS
            );
            if shell {
                remote.interactive()?;
            }
            Ok(())
        }
    }
}
