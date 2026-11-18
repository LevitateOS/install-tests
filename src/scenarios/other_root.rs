//! other-root: the recab contract on an installed A/B system
//! (`tools/recab/REQUIREMENTS.md`), over the install run's disk and UEFI
//! variables (a disposable overlay, kept for all boots of the run):
//!
//! 1. Root A boots as the default: it is locked, root B is unlocked at
//!    `/run/recab/other`. A file is written into B and `recab boot-other`
//!    picks B for the next boot only.
//! 2. Root B boots: the file is there, B is locked and A unlocked, and
//!    `recab-started` makes B the boot default. `recab copy` makes A a copy
//!    of B, and `recab compare` finds them equal.
//! 3. A is then made unable to start (its init removed) and picked once with
//!    `recab boot-other`. That boot stops in the installed initramfs, before
//!    any init (so before `recab-started`, which restarts a root that ends in
//!    the emergency state by itself); the scenario cuts the power, as a
//!    person at the machine would.
//! 4. The next boot starts B again, the root that last finished starting,
//!    with no one-time pick left.
//!
//! Removing the init is the one way to keep a root from starting that is the
//! same on systemd and OpenRC.

use anyhow::{bail, ensure, Context, Result};
use recab::Slot;
use recqemu::{shell_quote, Guest, Outcome};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::{overlay_install, Harness, InstallRun, ScenarioId};
use crate::budget::Budget;
use crate::qemu::session::{self, Vars, VmSpec};
use crate::qemu::wait_installed_boot;

/// The file written into the other root.
const PROBE: &str = "etc/other-root-probe";

/// `recab status --json`, the fields this scenario reads.
#[derive(Debug, Deserialize)]
struct Status {
    booted: Slot,
    other: Slot,
    other_mount: Option<String>,
    /// The other root has its boot entry (false while a copy is unfinished).
    other_bootable: bool,
    boot_default: Option<Slot>,
    next_boot_once: Option<Slot>,
}

/// The run's disk and UEFI variables, booted again and again.
struct Machine<'a> {
    h: &'a Harness,
    flavour: distro_contract::profile::Flavour,
    disk: PathBuf,
    vars: PathBuf,
    run_dir: &'a Path,
    boots: usize,
}

/// A booted root with root logged in on the serial console.
struct Booted {
    vm: session::Vm,
}

impl Machine<'_> {
    fn spawn(&mut self) -> Result<session::Vm> {
        self.boots += 1;
        let log = self
            .run_dir
            .join(format!("boot-{:02}.serial.log", self.boots));
        session::spawn(VmSpec {
            disk: Some(&self.disk),
            flavour: self.flavour,
            vars: Vars::Existing(&self.vars),
            serial_log: Some(&log),
            ..VmSpec::new(&self.h.variant, ScenarioId::OtherRoot)
        })
    }

    /// Boot to the login prompt and log in as root.
    fn boot(&mut self) -> Result<Booted> {
        let variant = &self.h.variant;
        let mut vm = self.spawn()?;
        wait_installed_boot(vm.console()?, variant)?;
        crate::qemu::login(vm.console()?, "root", crate::expect::root_password(variant))?;
        Ok(Booted { vm })
    }

    /// Boot a root that cannot start: the installed initramfs stops at the
    /// missing init and the login prompt never comes. Then cut the power.
    fn boot_without_init(&mut self) -> Result<()> {
        let variant = &self.h.variant;
        let mut vm = self.spawn()?;
        let stopped = vec![format!(
            "{} in the root filesystem does not resolve to an executable",
            recinit::live::SWITCH_ROOT_INIT
        )];
        let login = crate::expect::installed_boot(variant).success;
        vm.console()?
            .wait_for_boot(
                Budget::BootSilence.get(),
                &recqemu::serial::BootPatterns {
                    success: &stopped,
                    fatal: &login,
                },
            )
            .context("the root without an init was expected to stop in the initramfs")?;
        // Dropping the VM destroys the domain: the power is cut.
        drop(vm);
        Ok(())
    }
}

impl Booted {
    fn run(&mut self, command: &str, budget: Budget) -> Result<String> {
        match self.vm.console()?.exec(command, budget.get())? {
            Outcome::Exited {
                code: 0, output, ..
            } => Ok(output.trim().to_owned()),
            outcome => bail!("'{command}' {}:\n{}", outcome.describe(), outcome.output()),
        }
    }

    fn sh(&mut self, command: &str) -> Result<String> {
        self.run(command, Budget::Command)
    }

    fn status(&mut self) -> Result<Status> {
        let out = self.sh("recab --json status")?;
        let text = out
            .find('{')
            .map(|i| &out[i..])
            .context("recab status printed no JSON")?;
        serde_json::from_str(text).with_context(|| format!("parsing recab status: {out}"))
    }

    /// The booted root is `booted`, locked; the other root is unlocked and
    /// mounted read-write at recab's mount point.
    fn expect_locked(&mut self, booted: Slot) -> Result<Status> {
        let status = self.status()?;
        ensure!(
            status.booted == booted && status.other == booted.other(),
            "expected root {booted} booted: {status:?}"
        );
        ensure!(
            status.other_mount.as_deref() == Some(recab::OTHER_MOUNT),
            "the other root is not mounted at {}: {status:?}",
            recab::OTHER_MOUNT
        );
        self.sh("findmnt -n -o OPTIONS / | tr ',' '\\n' | grep -qx ro")
            .context("the booted root is not mounted read-only")?;
        self.sh(r#"[ "$(blockdev --getro "$(findmnt -n -o SOURCE /)")" = 1 ]"#)
            .context("the booted root's device is not read-only")?;
        self.sh("if (printf x > /etc/.other-root-refused) 2>/dev/null; then false; else true; fi")
            .context("the booted root took a write")?;
        self.sh(&format!(
            "findmnt -n -o OPTIONS {} | tr ',' '\\n' | grep -qx rw",
            recab::OTHER_MOUNT
        ))
        .context("the other root is not mounted read-write")?;
        Ok(status)
    }

    /// Wait for `recab-started` to make `slot` the boot default.
    fn expect_default(&mut self, slot: Slot) -> Result<()> {
        // recab prints pretty JSON, so the colon is followed by a space.
        let wanted = format!("\"boot_default\": *\"{slot}\"");
        // The loop runs in a subshell so its `exit` ends only the subshell; a bare
        // `exit` would log out the guest's interactive login shell.
        self.run(
            &format!(
                "(i=0; while [ $i -lt 100 ]; do recab --json status | grep -Eq {} && exit 0; i=$((i+1)); sleep 2; done; exit 1)",
                shell_quote(&wanted)
            ),
            Budget::Started,
        )
        .with_context(|| format!("root {slot} did not become the boot default after it started"))?;
        Ok(())
    }

    fn reboot(mut self) -> Result<()> {
        self.vm.shutdown_from_guest("reboot")
    }

    fn poweroff(mut self) -> Result<()> {
        self.vm.shutdown_from_guest("poweroff")
    }
}

pub(super) fn run(h: &Harness, install: &InstallRun, run_dir: &Path) -> Result<String> {
    let (disk, vars) = overlay_install(install, run_dir)?;
    let mut machine = Machine {
        h,
        flavour: install.flavour,
        disk,
        vars,
        run_dir,
        boots: 0,
    };
    let other = recab::OTHER_MOUNT;
    let probe = format!(
        "written into the other root by other-root ({})",
        h.variant.id
    );

    // 1. Root A, the default after installation.
    let mut sys = machine.boot()?;
    let status = sys.expect_locked(Slot::A)?;
    ensure!(
        status.boot_default == Some(Slot::A)
            && status.next_boot_once.is_none()
            && status.other_bootable,
        "after installation root A must be the default with nothing picked and B bootable: {status:?}"
    );
    sys.sh(&format!(
        "printf '%s' {} > {other}/{PROBE} && sync",
        shell_quote(&probe)
    ))
    .context("writing into the unlocked other root")?;
    sys.sh("recab boot-other")?;
    let status = sys.status()?;
    ensure!(
        status.next_boot_once == Some(Slot::B) && status.boot_default == Some(Slot::A),
        "boot-other must pick B once and leave A the default: {status:?}"
    );
    sys.reboot()?;

    // 2. Root B, picked once: locked, carrying the change, then the default.
    let mut sys = machine.boot()?;
    sys.expect_locked(Slot::B)?;
    sys.sh(&format!(
        "test \"$(cat /{PROBE})\" = {}",
        shell_quote(&probe)
    ))
    .context("the file written into the other root is not in the booted root")?;
    sys.sh(&format!("test ! -e {other}/{PROBE}"))
        .context("root A, now the other root, already has the file")?;
    sys.expect_default(Slot::B)?;
    sys.run("recab copy", Budget::Stage)?;
    let compared = sys.run("recab --json compare", Budget::Stage)?;
    // recab prints pretty JSON, so parse it rather than match its spacing.
    let parsed = serde_json::from_str::<serde_json::Value>(&compared);
    let equal = parsed.as_ref().is_ok_and(|value| {
        value["differences"].as_array().is_some_and(Vec::is_empty)
            && value["more"].as_u64() == Some(0)
    });
    let parse_note = match &parsed {
        Ok(_) => String::new(),
        Err(error) => format!(" (compare output is not JSON: {error})"),
    };
    ensure!(
        equal,
        "after recab copy the roots still differ{parse_note}:\n{compared}"
    );
    sys.sh(&format!(
        "test \"$(cat {other}/{PROBE})\" = {}",
        shell_quote(&probe)
    ))
    .context("recab copy did not bring the file into root A")?;
    ensure!(
        sys.status()?.other_bootable,
        "after recab copy root A has no boot entry"
    );

    // 3. Root A made unable to start, picked once.
    sys.sh(&format!(
        "rm -f {other}{init} && test ! -e {other}{init} && sync",
        init = recinit::live::SWITCH_ROOT_INIT
    ))
    .context("removing the other root's init")?;
    sys.sh("recab boot-other")?;
    let status = sys.status()?;
    ensure!(
        status.next_boot_once == Some(Slot::A) && status.boot_default == Some(Slot::B),
        "boot-other must pick A once and leave B the default: {status:?}"
    );
    sys.reboot()?;
    machine.boot_without_init()?;

    // 4. The next boot starts B, the root that last finished starting.
    let mut sys = machine.boot()?;
    let status = sys.expect_locked(Slot::B)?;
    ensure!(
        status.boot_default == Some(Slot::B) && status.next_boot_once.is_none(),
        "after the failed boot of A, B must be the default with nothing picked: {status:?}"
    );
    sys.poweroff()?;

    Ok(format!(
        "{} boots: a change written into the unlocked other root booted with boot-other, the booted root was locked and became the default once started; copy made the roots equal; a root that could not start fell back to the previous root",
        machine.boots
    ))
}
