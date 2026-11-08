//! The A/B update scenario. On a copy of the latest install disk, the
//! installed system writes the release's system into its inactive slot with
//! `recab stage`, then the slot state machine is checked across real reboots:
//! a degraded trial that reboots back by itself, a healthy trial that
//! recab-trial.service commits, and rollback to the previous slot.

use anyhow::{bail, ensure, Context, Result};
use distro_contract::Variant;
use recab::{Slot, Status};
use recinit::live::FAIL_REBOOT_OPTION;
use std::path::Path;
use std::time::{Duration, Instant};

use super::{Harness, InstallRun, ScenarioId};
use crate::budget::Budget;
use crate::guest;
use crate::qemu::session::{self, Vars, VmSpec};
use crate::qemu::{wait_installed_boot, Console};
use recqemu::{shell_quote, Guest, Outcome};

/// Where the scenario mounts slot B to break and repair it.
const SLOT_MOUNT: &str = "/run/ab-update-slot";
/// A service that always fails, so slot B's trial comes up degraded.
const FAILING_SERVICE: &str = "ab-update-fail";

/// `install` is the latest install run, whose disk is A/B (`INSTALL_MODE`);
/// the scenario boots an overlay of it in `run_dir`.
pub(super) fn run(h: &Harness, install: &InstallRun, iso: &Path, run_dir: &Path) -> Result<String> {
    let (disk, vars) = super::overlay_install(install, run_dir)?;
    Machine { variant: &h.variant, disk: &disk, vars: &vars, iso }.exercise()
}

struct Machine<'a> {
    variant: &'a Variant,
    disk: &'a Path,
    vars: &'a Path,
    iso: &'a Path,
}

/// A booted, logged-in installed system.
struct Booted<'a> {
    vm: session::Vm,
    variant: &'a Variant,
}

impl Machine<'_> {
    fn exercise(&self) -> Result<String> {
        let mut steps = Vec::new();
        let contract = &self.variant.contract;
        let init = contract.profile.init;
        let entry_b = format!(
            "/boot/{}/{}",
            recab::ENTRIES_DIR,
            recab::entry_name(&contract.identity.os_id, Slot::B)
        );

        // Installed: slot A runs, is the known-good default, and supervises trials.
        let mut sys = self.boot(true)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after install")?;
        ensure!(st.next_boot == Some(Slot::A), "after install the next boot is {:?}, not A", st.next_boot);
        ensure!(sys.sh("findmnt -n -o SOURCE /var")?.contains("/dev/"), "/var is not its own partition");
        sys.sh(&format!("test -L /{}", recab::trial_service_link(init)))?;
        // Installed systems leave sshd off, so there are no host keys yet; make them, as
        // starting sshd would, so the carry into slot B is tested.
        sys.sh("ssh-keygen -A")?;
        let identity = sys.identity()?;

        // Stage the release into slot B.
        sys.stage_from_media(&contract.identity.iso_label)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after stage")?;
        ensure!(!st.trial_pending, "stage alone must not schedule a boot");
        let staged = sys.sh(&format!("cat {entry_b}"))?;
        ensure!(
            staged.contains("panic="),
            "slot B's staged entry lacks panic=:\n{staged}"
        );
        ensure!(
            staged.contains(FAIL_REBOOT_OPTION),
            "slot B's staged entry lacks the initramfs trial options:\n{staged}"
        );
        steps.push("staged slot B from the release image");

        // A trial that comes up degraded reboots back to A by itself.
        let slot_b = st
            .slots
            .iter()
            .find(|view| view.slot == Slot::B)
            .and_then(|view| view.device.clone())
            .with_context(|| format!("recab status names no device for slot B: {st:?}"))?;
        sys.on_slot(&slot_b, &format!("svc_add_failing {SLOT_MOUNT} {FAILING_SERVICE}"))?;
        sys.sh("recab set-next B")?;
        let st = sys.status()?;
        ensure!(st.trial_pending && st.next_boot == Some(Slot::B), "set-next B did not schedule a trial: {st:?}");
        sys.reboot()?;
        self.run_unattended(Budget::TrialReboot.get())
            .context("the degraded trial of slot B did not reboot by itself")?;
        let mut sys = self.boot(false)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after a degraded trial")?;
        ensure!(!st.trial_pending, "the trial of slot B was never booted: {st:?}");
        ensure!(
            st.last_trial.as_ref().is_some_and(|t| t.slot == Slot::B && !t.committed),
            "the failed trial of slot B was not recorded: {st:?}"
        );
        steps.push("a degraded trial of slot B rebooted into slot A by itself and was recorded");

        // Repair B: the healthy trial commits itself.
        // /run is empty again after the reboot.
        sys.on_slot(&slot_b, &format!("svc_remove {SLOT_MOUNT} {FAILING_SERVICE}"))?;
        sys.sh("recab set-next B")?;
        sys.reboot()?;
        let mut sys = self.boot(false)?;
        let hostname = sys.sh("cat /etc/hostname")?;
        ensure!(hostname == contract.identity.default_hostname, "slot B's hostname is '{hostname}'");
        ensure!(sys.identity()? == identity, "slot B did not keep the machine-id and SSH host keys");
        sys.wait_committed(Slot::B)?;
        let st = sys.status()?;
        expect(&st, Slot::B, true, "after the healthy trial")?;
        ensure!(st.default_slot == Some(Slot::B), "the commit did not make B the default: {st:?}");
        ensure!(st.rollback_slot == Some(Slot::A), "after the commit rollback should return to A: {st:?}");
        let committed_entry = sys.sh(&format!("cat {entry_b}"))?;
        ensure!(
            !committed_entry.contains(FAIL_REBOOT_OPTION),
            "slot B kept the trial options after its commit:\n{committed_entry}"
        );
        steps.push("a healthy trial of slot B, with the machine's identity, committed itself");
        sys.reboot()?;

        // A second generation: from the committed slot B, stage A again.
        let mut sys = self.boot(true)?;
        expect(&sys.status()?, Slot::B, true, "on the boot after the commit")?;
        sys.stage_from_media(&contract.identity.iso_label)?;
        sys.sh("recab set-next A")?;
        sys.reboot()?;
        let mut sys = self.boot(false)?;
        ensure!(sys.identity()? == identity, "slot A, staged from B, did not keep the machine's identity");
        sys.wait_committed(Slot::A)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after the second-generation trial")?;
        ensure!(st.rollback_slot == Some(Slot::B), "after the commit rollback should return to B: {st:?}");
        steps.push("slot A, staged from the committed slot B, committed itself");

        // Roll back to the previous system.
        sys.sh("recab rollback")?;
        sys.reboot()?;
        let mut sys = self.boot(false)?;
        let st = sys.status()?;
        expect(&st, Slot::B, true, "after rollback")?;
        ensure!(st.rollback_slot == Some(Slot::A), "after rollback A should remain a rollback target: {st:?}");
        steps.push("rolled back to slot B");
        sys.poweroff()?;

        Ok(format!("A/B update verified across 7 boots: {}", steps.join("; ")))
    }

    /// Boot without logging in, and wait for the guest to reboot by itself.
    fn run_unattended(&self, timeout: std::time::Duration) -> Result<()> {
        let mut vm = session::spawn(VmSpec {
            disk: Some(self.disk),
            vars: Vars::Existing(self.vars),
            ..VmSpec::new(self.variant, ScenarioId::AbUpdate)
        })?;
        vm.wait_for_exit(timeout)
    }

    fn boot(&self, with_media: bool) -> Result<Booted<'_>> {
        let mut vm = session::spawn(VmSpec {
            disk: Some(self.disk),
            vars: Vars::Existing(self.vars),
            media: with_media.then_some(self.iso),
            ..VmSpec::new(self.variant, ScenarioId::AbUpdate)
        })?;
        wait_installed_boot(vm.console()?, self.variant)?;
        vm.console()?
            .login("root", crate::expect::root_password(self.variant), Budget::Login.get())?;
        Ok(Booted { vm, variant: self.variant })
    }
}

impl Booted<'_> {
    fn console(&mut self) -> Result<&mut Console> {
        self.vm.console()
    }

    /// Mount the slot `device`, run init-backend `call` on it, and unmount it.
    fn on_slot(&mut self, device: &str, call: &str) -> Result<()> {
        self.sh(&format!("mkdir -p {SLOT_MOUNT} && mount {device} {SLOT_MOUNT}"))?;
        let variant = self.variant;
        guest::install_lib(self.console()?, variant)?;
        let outcome = guest::backend(self.console()?, variant, call, Budget::Command)?;
        if !outcome.success() {
            bail!("'{call}' on slot B failed ({}): {}", outcome.describe(), outcome.output().trim());
        }
        self.sh(&format!("umount {SLOT_MOUNT}"))?;
        Ok(())
    }

    /// The machine-id and SSH host key fingerprints, which `stage` carries.
    fn identity(&mut self) -> Result<String> {
        self.sh("cat /etc/machine-id 2>/dev/null; for k in /etc/ssh/ssh_host_*_key.pub; do ssh-keygen -lf \"$k\"; done")
    }

    /// Wait for recab-trial.service to commit the running trial of `slot`.
    fn wait_committed(&mut self, slot: Slot) -> Result<()> {
        let deadline = Instant::now() + Budget::Commit.get();
        loop {
            let st = self.status()?;
            if st.active_slot == slot && st.committed {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "the healthy trial of slot {slot} was not committed: {st:?}"
            );
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// Stage the inactive slot from the attached release ISO with
    /// `recab stage --from-media`. `iso_label` picks the release ISO out of the attached discs (the boot-injection
    /// disc is ISO 9660 too, and blkid lists the discs in no set order).
    fn stage_from_media(&mut self, iso_label: &str) -> Result<()> {
        let media = self.sh(&format!(
            "blkid -t LABEL={} -o device | head -n1",
            shell_quote(iso_label)
        ))?;
        ensure!(!media.is_empty(), "the update medium (ISO label {iso_label}) is not attached");
        self.run(&format!("recab stage --from-media {}", shell_quote(&media)), Budget::Stage)?;
        Ok(())
    }

    fn run(&mut self, command: &str, budget: Budget) -> Result<String> {
        match self.console()?.exec(command, budget.get())? {
            Outcome::Exited { code: 0, output, .. } => Ok(output.trim().to_string()),
            outcome => bail!("'{command}' {}:\n{}", outcome.describe(), outcome.output()),
        }
    }

    fn sh(&mut self, command: &str) -> Result<String> {
        self.run(command, Budget::Command)
    }

    fn status(&mut self) -> Result<Status> {
        let out = self.sh("recab status --json")?;
        let json = out
            .find('{')
            .map(|start| &out[start..])
            .with_context(|| format!("recab status printed no JSON:\n{out}"))?;
        serde_json::from_str(json).with_context(|| format!("parsing recab status:\n{out}"))
    }

    /// Reboot; a guest reboot ends the domain, and the next `boot` starts
    /// it again with the same disk and UEFI variables.
    fn reboot(mut self) -> Result<()> {
        self.vm.shutdown_from_guest("reboot")
    }

    fn poweroff(mut self) -> Result<()> {
        self.vm.shutdown_from_guest("poweroff")
    }
}

fn expect(st: &Status, active: Slot, committed: bool, when: &str) -> Result<()> {
    ensure!(
        st.active_slot == active && st.committed == committed,
        "{when}: expected slot {active} running {}, got {st:?}",
        if committed { "committed" } else { "uncommitted" }
    );
    Ok(())
}
