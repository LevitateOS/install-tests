//! The install scenario's installation: the shipped tools, run over SSH in
//! the live system in the order the live install docs give them (recpart,
//! recstrap, recfstab, recsetup), with test-specific choices supplied as inputs.

use anyhow::{Context, Result};
use recinit::live::LIVE_MEDIA_DIR;
use recpart::types::{ApplyResult, DiskListResult, CONFIRM_TOKEN};
use recpart::InstallMode;
use recqemu::shell_quote;
use recshuttle::Remote;

use crate::budget::Budget;

use distro_contract::Variant;

/// Kernel console arguments for the installed system: the screen and the
/// serial port the tests read.
const VGA_CONSOLE: &str = "console=tty0";
const SERIAL_CONSOLE: &str = "console=ttyS0,115200n8";

/// What the test system needs beyond the shipped tools.
pub struct TestSystem<'a> {
    pub mode: InstallMode,
    pub root_password: &'a str,
}

/// Install onto the guest's first writable disk and leave it mounted at the
/// install target. Returns the target and the number of steps run.
pub fn install(
    remote: &mut Remote,
    variant: &Variant,
    system: &TestSystem<'_>,
) -> Result<(String, usize)> {
    let mut steps = 0usize;
    let mut run = |what: &str, cmd: String| -> Result<String> {
        eprintln!("    -> {what}");
        steps += 1;
        remote.run(what, &cmd, Budget::Stage.get())
    };

    let disks: DiskListResult =
        serde_json::from_str(&run("List disks", "recpart list-disks --json".to_string())?)
            .context("parsing recpart list-disks JSON")?;
    let disk = disks
        .disks
        .iter()
        .find(|d| !d.read_only && !d.target.path.starts_with("/dev/loop"))
        .context("recpart lists no writable disk to install to")?
        .target
        .path
        .display()
        .to_string();

    let applied: ApplyResult = serde_json::from_str(&run(
        "Partition, format and mount (recpart)",
        format!(
            "recpart apply --disk {} --mode {} --confirm {CONFIRM_TOKEN} --json",
            shell_quote(&disk),
            system.mode
        ),
    )?)
    .context("parsing recpart apply JSON")?;
    let target = applied.handoff.install_target.clone();
    let t = shell_quote(&target);

    run("Extract rootfs (recstrap)", format!("recstrap --force {t}"))?;
    run(
        "Generate fstab (recfstab)",
        format!(
            "recfstab {t} >> {}",
            shell_quote(&format!("{target}/etc/fstab"))
        ),
    )?;
    // Exercise the shipped setup frontend. It delegates filesystem policy to
    // the setup program installed by the system recipe and boot state to recab.
    let mut setup = format!(
        "recsetup apply --root {t} --hostname {} --root-password --passwords-stdin --machine-id --boot {} --kernel {} --options {}",
        shell_quote(&variant.contract.identity.default_hostname), system.mode,
        shell_quote(&format!("{LIVE_MEDIA_DIR}/boot/vmlinuz")),
        shell_quote(&format!("{VGA_CONSOLE} {SERIAL_CONSOLE}")),
    );
    // The installed system's initramfs, when the variant declares one: the ISO carries
    // it beside its own live one (that one only finds the live medium); recsetup
    // defaults to it, named here so the scenario fails loudly if the ISO lacks it.
    if let Some(initramfs) = &variant.contract.artifacts.initramfs_installed {
        setup.push_str(&format!(
            " --initrd {}",
            shell_quote(&format!("{LIVE_MEDIA_DIR}/boot/{initramfs}"))
        ));
    }
    match system.mode {
        InstallMode::Ab => {
            let slots = &applied.handoff.mode_context;
            let a = slots
                .slot_a_device
                .as_ref()
                .context("recpart reported no slot A device")?;
            let b = slots
                .slot_b_device
                .as_ref()
                .context("recpart reported no slot B device")?;
            setup.push_str(&format!(
                " --slot-a {} --slot-b {}",
                shell_quote(a),
                shell_quote(b)
            ));
        }
        InstallMode::Mutable => {
            let device = &applied
                .handoff
                .mount_map
                .iter()
                .find(|m| m.path == target)
                .context("recpart mounted no root filesystem")?
                .device;
            setup.push_str(&format!(" --root-device {}", shell_quote(device)));
        }
    }
    run(
        "Configure and boot the installed system (recsetup)",
        format!(
            "printf '%s\\n' {} | {setup}",
            shell_quote(&format!("root:{}", system.root_password)),
        ),
    )?;
    Ok((target, steps))
}
