//! The A/B update scenario. On a copy of the latest install disk, the
//! installed system writes the release's system into its inactive slot with
//! `recab stage`, then the slot state machine is checked across real reboots:
//! a degraded trial that reboots back by itself, a healthy trial that
//! recab-trial.service commits, and rollback to the previous slot.

use anyhow::{bail, ensure, Context, Result};
use recab::{Slot, Status};
use recinit::live::{
    KERNEL_ISO_PATH, ROOTFS_ISO_PATH, ROOTFS_PAYLOAD_PARTITION,
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
/// A degraded trial: boot, startup, recab's decision, and the reboot.
const TRIAL_REBOOT_TIMEOUT: Duration = Duration::from_secs(300);
const COMMIT_WAIT: Duration = Duration::from_secs(200);
/// Where the scenario mounts slot B to break and repair it.
const SLOT_MOUNT: &str = "/run/ab-update-slot";
/// A service that always fails, so slot B's trial comes up degraded.
const FAILING_SERVICE: &str = "ab-update-fail";

pub(super) fn run(ctx: &dyn DistroContext, iso: &Path) -> Result<String> {
    if install_mode_for_distro(ctx.id())? != InstallMode::Ab {
        bail!(
            "{} installs in mutable mode (scenarios.runtime_policy.rootfs_mutability in its distro.toml); ab-update needs an A/B install",
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
        let contract = distro_contract::load(&crate::repo_root(), self.ctx.id())?.contract;
        let entry = |slot: &str| {
            format!(
                "/boot/loader/entries/{}-{slot}.conf",
                contract.identity.os_id
            )
        };

        // Installed: slot A runs, is the known-good default, and supervises trials.
        let mut sys = self.boot(true)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after install")?;
        ensure!(st.next_boot == Some(Slot::A), "after install the next boot is {:?}, not A", st.next_boot);
        ensure!(sys.sh("findmnt -n -o SOURCE /var")?.contains("/dev/"), "/var is not its own partition");
        sys.sh(&format!("test -L /{}", self.ctx.init_system().trial_service_link()))?;
        // Installed systems leave sshd off, so there are no host keys yet; make them, as
        // starting sshd would, so the carry into slot B is tested.
        sys.sh("ssh-keygen -A")?;
        let identity = sys.identity()?;

        // Stage the release into slot B.
        sys.stage_from_media(self.ctx, &contract.identity.iso_label)?;
        let st = sys.status()?;
        expect(&st, Slot::A, true, "after stage")?;
        ensure!(!st.trial_pending, "stage alone must not schedule a boot");
        let staged = sys.sh(&format!("cat {}", entry("b")))?;
        ensure!(
            staged.contains("panic="),
            "slot B's staged entry lacks panic=:\n{staged}"
        );
        if self.ctx.uses_initramfs() {
            ensure!(
                staged.contains("rd.emergency=reboot"),
                "slot B's staged entry lacks the initramfs trial options:\n{staged}"
            );
        }
        steps.push("staged slot B from the release image");

        // A trial that comes up degraded reboots back to A by itself.
        let slot_b = format!(
            "/dev/disk/by-partuuid/$(sed -n 's/.*root=PARTUUID=\\([^ ]*\\).*/\\1/p' {})",
            entry("b")
        );
        sys.sh(&format!(
            "mkdir -p {SLOT_MOUNT} && mount {slot_b} {SLOT_MOUNT} && {} && umount {SLOT_MOUNT}",
            failing_service(self.ctx.init_system(), true)
        ))?;
        sys.sh("recab set-next B")?;
        let st = sys.status()?;
        ensure!(st.trial_pending && st.next_boot == Some(Slot::B), "set-next B did not schedule a trial: {st:?}");
        sys.reboot()?;
        self.run_unattended(TRIAL_REBOOT_TIMEOUT)
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
        sys.sh(&format!(
            "mkdir -p {SLOT_MOUNT} && mount {slot_b} {SLOT_MOUNT} && {} && umount {SLOT_MOUNT}",
            failing_service(self.ctx.init_system(), false)
        ))?;
        sys.sh("recab set-next B")?;
        sys.reboot()?;
        let mut sys = self.boot(false)?;
        let hostname = sys.sh("cat /etc/hostname")?;
        ensure!(hostname == self.ctx.default_hostname(), "slot B's hostname is '{hostname}'");
        ensure!(sys.identity()? == identity, "slot B did not keep the machine-id and SSH host keys");
        sys.wait_committed()?;
        let st = sys.status()?;
        expect(&st, Slot::B, true, "after the healthy trial")?;
        ensure!(st.default_slot == Some(Slot::B), "the commit did not make B the default: {st:?}");
        ensure!(st.rollback_slot == Some(Slot::A), "after the commit rollback should return to A: {st:?}");
        let committed_entry = sys.sh(&format!("cat {}", entry("b")))?;
        ensure!(
            !committed_entry.contains("rd.emergency=reboot"),
            "slot B kept the trial options after its commit:\n{committed_entry}"
        );
        steps.push("a healthy trial of slot B, with the machine's identity, committed itself");
        sys.reboot()?;

        // A second generation: from the committed slot B, stage A again.
        let mut sys = self.boot(true)?;
        expect(&sys.status()?, Slot::B, true, "on the boot after the commit")?;
        sys.stage_from_media(self.ctx, &contract.identity.iso_label)?;
        sys.sh("recab set-next A")?;
        sys.reboot()?;
        let mut sys = self.boot(false)?;
        ensure!(sys.identity()? == identity, "slot A, staged from B, did not keep the machine's identity");
        sys.wait_committed()?;
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
    fn run_unattended(&self, timeout: Duration) -> Result<()> {
        let mut vm = session::spawn(VmSpec {
            name: &format!("{}-ab-update", self.ctx.id()),
            disk: Some(self.disk),
            ovmf_vars: Some(self.vars),
            ..Default::default()
        })?;
        vm.wait_for_exit(timeout)
    }

    fn boot(&self, with_media: bool) -> Result<Booted> {
        let mut vm = session::spawn(VmSpec {
            name: &format!("{}-ab-update", self.ctx.id()),
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

    /// The machine-id and SSH host key fingerprints, which `stage` carries.
    fn identity(&mut self) -> Result<String> {
        self.sh("cat /etc/machine-id 2>/dev/null; for k in /etc/ssh/ssh_host_*_key.pub; do ssh-keygen -lf \"$k\"; done")
    }

    /// Wait for recab-trial.service to commit the running trial.
    fn wait_committed(&mut self) -> Result<()> {
        self.run(
            // A subshell: `exit` in the console's own shell would log it out.
            "(for i in $(seq 90); do recab status --json | grep -q '\"committed\": true' && exit 0; sleep 2; done; exit 1)",
            COMMIT_WAIT,
        )
        .context("the healthy trial was not committed")?;
        Ok(())
    }

    /// Stage the inactive slot from the attached release ISO.
    /// `iso_label` picks the release ISO out of the attached discs (the boot-injection
    /// disc is ISO 9660 too, and blkid lists the discs in no set order).
    fn stage_from_media(&mut self, ctx: &dyn DistroContext, iso_label: &str) -> Result<()> {
        let media = self.sh(&format!(
            "blkid -t LABEL={} -o device | head -n1",
            shell_quote(iso_label)
        ))?;
        ensure!(!media.is_empty(), "the update medium (ISO label {iso_label}) is not attached");
        self.sh(&format!("mkdir -p {MEDIA_MOUNT} && mount -o ro {} {MEDIA_MOUNT}", shell_quote(&media)))?;
        // The rootfs is a file in the ISO, or (the default layout) an
        // appended partition, which a CD drive does not expose: a read-only
        // partition-scanning loop device over the CD does.
        let rootfs = self.sh(&format!(
            "f={MEDIA_MOUNT}/{ROOTFS_ISO_PATH}; if [ -s \"$f\" ] && [ $(stat -c %s \"$f\") -gt 65536 ]; then echo \"$f\"; else l=$(losetup --find --show --read-only --partscan {}) && for i in $(seq 50); do [ -e \"${{l}}p{ROOTFS_PAYLOAD_PARTITION}\" ] && break; sleep 0.1; done; echo \"${{l}}p{ROOTFS_PAYLOAD_PARTITION}\"; fi",
            shell_quote(&media)
        ))?;
        let mut stage = format!(
            "recab stage --rootfs {} --kernel {MEDIA_MOUNT}/{KERNEL_ISO_PATH}",
            shell_quote(&rootfs)
        );
        if ctx.uses_initramfs() {
            // The installed system's initramfs, which the ISO carries beside its live one
            // (that one only looks for the live medium), as the install uses.
            stage.push_str(&format!(" --initrd {MEDIA_MOUNT}/boot/initramfs-installed.img"));
        }
        self.run(&stage, STAGE_TIMEOUT)?;
        self.sh(&format!("umount {MEDIA_MOUNT}; losetup -D"))?;
        Ok(())
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
        let _ = self.console().exec("reboot", Duration::from_secs(2));
        self.vm.wait_for_exit(SHUTDOWN_TIMEOUT)
    }

    fn poweroff(mut self) -> Result<()> {
        let _ = self.console().exec("poweroff", Duration::from_secs(2));
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

/// Shell that adds (or removes) a service that always fails to slot B,
/// mounted at [`SLOT_MOUNT`], enabled in the init system's normal boot.
fn failing_service(init: recab::InitSystem, add: bool) -> String {
    let (script, link, content) = match init {
        recab::InitSystem::Systemd => (
            format!("{SLOT_MOUNT}/etc/systemd/system/{FAILING_SERVICE}.service"),
            format!("{SLOT_MOUNT}/etc/systemd/system/multi-user.target.wants/{FAILING_SERVICE}.service"),
            "[Service]\\nType=oneshot\\nExecStart=/bin/false\\n[Install]\\nWantedBy=multi-user.target\\n",
        ),
        recab::InitSystem::OpenRc => (
            format!("{SLOT_MOUNT}/etc/init.d/{FAILING_SERVICE}"),
            format!("{SLOT_MOUNT}/etc/runlevels/default/{FAILING_SERVICE}"),
            "#!/sbin/openrc-run\\nstart() {\\n\\treturn 1\\n}\\n",
        ),
    };
    let target = script.trim_start_matches(SLOT_MOUNT);
    if add {
        format!("printf '{content}' > {script} && chmod 755 {script} && ln -s {target} {link}")
    } else {
        format!("rm {script} {link}")
    }
}
