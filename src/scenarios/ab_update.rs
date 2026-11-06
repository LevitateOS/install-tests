//! The A/B update scenario. On a copy of the latest install disk, the
//! installed system writes the release's system into its inactive slot with
//! `recab stage`, then the slot state machine is checked across real reboots:
//! a trial boot, automatic fallback from an uncommitted trial, commit, and
//! rollback to the previous slot.

use anyhow::{bail, ensure, Context, Result};
use recab::{Slot, Status};
use recinit::live::{
    INITRAMFS_LIVE_ISO_PATH, KERNEL_ISO_PATH, ROOTFS_ISO_PATH, ROOTFS_PAYLOAD_PARTITION,
};
use recpart::InstallMode;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::install::shell_quote;
use super::{
    default_password, install_mode_for_distro, resolve_latest_install_runtime, ScenarioId,
    ScenarioRun, INSTALLED_BOOT_STALL, INSTALL_DISK_FILENAME, INSTALL_OVMF_VARS_FILENAME,
    LOGIN_TIMEOUT,
};
use crate::distro::DistroContext;
use crate::qemu::session::{self, VmSpec};
use crate::qemu::{Console, SerialExecutorExt};

const MEDIA_MOUNT: &str = "/run/ab-update-media";
const STAGE_TIMEOUT: Duration = Duration::from_secs(900);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(180);
const SHELL_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn run(ctx: &dyn DistroContext, iso: &Path) -> Result<String> {
    if install_mode_for_distro(ctx.id())? != InstallMode::Ab {
        bail!(
            "{} installs in mutable mode (rootfs_mutability in its scenarios.toml); ab-update needs an A/B install",
            ctx.id()
        );
    }
    let install = resolve_latest_install_runtime(ctx.id())?;
    let scenario_run = ScenarioRun::start(ctx.id(), ScenarioId::AbUpdate, Some(install.run_id.clone()))?;

    // A copy-on-write layer over the install disk, so the install result
    // stays as the install scenario left it.
    let disk = scenario_run.output_dir.join(INSTALL_DISK_FILENAME);
    let vars = scenario_run.output_dir.join(INSTALL_OVMF_VARS_FILENAME);
    let base = fs::canonicalize(&install.disk_path)?;
    let status = Command::new("qemu-img")
        .args(["create", "-q", "-f", "qcow2", "-F", "qcow2", "-b"])
        .arg(&base)
        .arg(&disk)
        .status()
        .context("running qemu-img")?;
    ensure!(status.success(), "qemu-img could not create an overlay of '{}'", base.display());
    fs::copy(&install.ovmf_vars_path, &vars).context("copying the install's UEFI variables")?;

    let result = Machine { ctx, disk: &disk, vars: &vars, iso }.exercise();
    match result {
        Ok(evidence) => {
            scenario_run.finish_success(&evidence, Some(&disk), Some(&vars))?;
            Ok(evidence)
        }
        Err(err) => {
            let _ = scenario_run.finish_failed(&format!("FAIL: {err:#}"), Some(&disk), Some(&vars));
            Err(err)
        }
    }
}

struct Machine<'a> {
    ctx: &'a dyn DistroContext,
    disk: &'a Path,
    vars: &'a Path,
    iso: &'a Path,
}

/// A booted, logged-in installed system.
struct Booted {
    vm: session::Vm,
}

impl Machine<'_> {
    fn exercise(&self) -> Result<String> {
        let mut steps = Vec::new();

        // Installed: slot A runs and is the known-good default.
        let mut sys = self.boot(true)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after install")?;
        ensure!(st.next_boot == Some(Slot::A), "after install the next boot is {:?}, not A", st.next_boot);
        ensure!(sys.sh("findmnt -n -o SOURCE /var")?.contains("/dev/"), "/var is not its own partition");

        // Stage the release into slot B and try it once.
        let media = sys.sh("blkid -t TYPE=iso9660 -o device | head -n1")?;
        ensure!(!media.is_empty(), "the update medium is not attached");
        sys.sh(&format!("mkdir -p {MEDIA_MOUNT} && mount -o ro {} {MEDIA_MOUNT}", shell_quote(&media)))?;
        // The rootfs is a file in the ISO, or (the default layout) an
        // appended partition, which a CD drive does not expose: a read-only
        // partition-scanning loop device over the CD does.
        let rootfs = sys.sh(&format!(
            "f={MEDIA_MOUNT}/{ROOTFS_ISO_PATH}; if [ -s \"$f\" ] && [ $(stat -c %s \"$f\") -gt 65536 ]; then echo \"$f\"; \
             else l=$(losetup --find --show --read-only --partscan {}) && udevadm settle && echo \"${{l}}p{ROOTFS_PAYLOAD_PARTITION}\"; fi",
            shell_quote(&media)
        ))?;
        let mut stage = format!(
            "recab stage --rootfs {} --kernel {MEDIA_MOUNT}/{KERNEL_ISO_PATH}",
            shell_quote(&rootfs)
        );
        if self.ctx.uses_initramfs() {
            stage.push_str(&format!(" --initrd {MEDIA_MOUNT}/{INITRAMFS_LIVE_ISO_PATH}"));
        }
        sys.run(&stage, STAGE_TIMEOUT)?;
        sys.sh(&format!("umount {MEDIA_MOUNT}; losetup -D"))?;
        steps.push("staged slot B from the release image");
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after stage")?;
        ensure!(!st.trial_pending, "stage alone must not schedule a boot");

        sys.sh("recab set-next B")?;
        let st = sys.status()?;
        ensure!(st.trial_pending && st.next_boot == Some(Slot::B), "set-next B did not schedule a trial: {st:?}");
        sys.reboot()?;

        // The trial boot runs slot B, uncommitted; the machine's identity came along.
        let mut sys = self.boot(false)?;
        let st = sys.status()?;
        expect(&st, Slot::B, false, "on the trial boot")?;
        ensure!(st.next_boot == Some(Slot::A), "an uncommitted trial must fall back to A, next boot is {:?}", st.next_boot);
        let hostname = sys.sh("cat /etc/hostname")?;
        ensure!(hostname == self.ctx.default_hostname(), "slot B's hostname is '{hostname}'");
        steps.push("trial-booted slot B with the machine's accounts and hostname");
        sys.reboot()?;

        // Not committed: the next boot falls back to A by itself.
        let mut sys = self.boot(false)?;
        expect(&sys.status()?, Slot::A, true, "after an uncommitted trial")?;
        steps.push("fell back to slot A after an uncommitted trial");

        // Try B again and commit it.
        sys.sh("recab set-next B")?;
        sys.reboot()?;
        let mut sys = self.boot(false)?;
        expect(&sys.status()?, Slot::B, false, "on the second trial boot")?;
        sys.sh("recab commit")?;
        let st = sys.status()?;
        expect(&st, Slot::B, true, "after commit")?;
        ensure!(st.default_slot == Some(Slot::B), "commit did not make B the default: {st:?}");
        ensure!(st.rollback_slot == Some(Slot::A), "after commit rollback should return to A: {st:?}");
        sys.reboot()?;

        let mut sys = self.boot(false)?;
        expect(&sys.status()?, Slot::B, true, "on the boot after commit")?;
        steps.push("committed slot B, which stays the default");

        // Roll back to the previous system.
        sys.sh("recab rollback")?;
        sys.reboot()?;
        let mut sys = self.boot(false)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after rollback")?;
        ensure!(st.rollback_slot == Some(Slot::B), "after rollback B should remain a rollback target: {st:?}");
        steps.push("rolled back to slot A");
        sys.poweroff()?;

        Ok(format!("A/B update verified across 6 boots: {}", steps.join("; ")))
    }

    fn boot(&self, with_media: bool) -> Result<Booted> {
        let mut vm = session::spawn(VmSpec {
            disk: Some(self.disk),
            ovmf_vars: Some(self.vars),
            media: with_media.then_some(self.iso),
            ..Default::default()
        })?;
        vm.console
            .wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, self.ctx)?;
        vm.console
            .login("root", &default_password(self.ctx)?, LOGIN_TIMEOUT)?;
        Ok(Booted { vm })
    }
}

impl Booted {
    fn console(&mut self) -> &mut Console {
        &mut self.vm.console
    }

    fn run(&mut self, command: &str, timeout: Duration) -> Result<String> {
        let result = self.console().exec(command, timeout)?;
        if !result.completed {
            bail!("'{command}' did not finish within {}s:\n{}", timeout.as_secs(), result.output);
        }
        if result.exit_code != 0 {
            bail!("'{command}' exited {}:\n{}", result.exit_code, result.output);
        }
        Ok(result.output.trim().to_string())
    }

    fn sh(&mut self, command: &str) -> Result<String> {
        self.run(command, SHELL_TIMEOUT)
    }

    fn status(&mut self) -> Result<Status> {
        let out = self.sh("recab status --json")?;
        let json = out
            .find('{')
            .map(|start| &out[start..])
            .with_context(|| format!("recab status printed no JSON:\n{out}"))?;
        serde_json::from_str(json).with_context(|| format!("parsing recab status:\n{out}"))
    }

    /// Reboot; the VM runs with -no-reboot, so QEMU exits and the next
    /// `boot` starts it again with the same disk and UEFI variables.
    fn reboot(mut self) -> Result<()> {
        let _ = self.console().exec("systemctl reboot", Duration::from_secs(2));
        self.vm.wait_for_exit(SHUTDOWN_TIMEOUT)
    }

    fn poweroff(mut self) -> Result<()> {
        let _ = self.console().exec("systemctl poweroff", Duration::from_secs(2));
        self.vm.wait_for_exit(SHUTDOWN_TIMEOUT)
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
