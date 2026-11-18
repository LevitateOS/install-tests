//! The install scenario's installation: the shipped tools, run over SSH in
//! the live system in the order the live install docs give them (recpart,
//! recstrap, recfstab, recsetup), with test-specific choices supplied as inputs.

use anyhow::{Context, Result};
use recinit::live::{INITRAMFS_INSTALLED_ISO_PATH, KERNEL_ISO_PATH, LIVE_MEDIA_DIR};
use recpart::InstallMode;
use recpart::types::{ApplyResult, CONFIRM_TOKEN, DiskListResult};
use recqemu::shell_quote;
use recshuttle::Remote;
use std::path::Path;

use crate::budget::Budget;
use crate::channel::Channel;

use distro_contract::Variant;

/// Kernel console arguments for the installed system: the screen and the
/// serial port the tests read.
const VGA_CONSOLE: &str = "console=tty0";
const SERIAL_CONSOLE: &str = "console=ttyS0,115200n8";

/// What the test system needs beyond the shipped tools.
pub struct TestSystem<'a> {
    pub mode: InstallMode,
    pub root_password: &'a str,
    pub flavour: distro_contract::profile::Flavour,
    /// The release run directory (`release.json` and the `parts-*.tar.zst` files), whose
    /// parts the installer fetches from a local signed channel; the desktop flavour only.
    pub release_dir: &'a Path,
}

/// Install onto the guest's first writable disk and leave it mounted at the
/// install target. Returns the target and the number of steps run.
pub fn install(
    remote: &mut Remote,
    variant: &Variant,
    system: &TestSystem<'_>,
) -> Result<(String, usize)> {
    let mut steps = 0usize;
    let mut run = |what: &str, cmd: String, budget: Budget| -> Result<String> {
        eprintln!("    -> {what}");
        steps += 1;
        remote.run(what, &cmd, budget.get())
    };

    let disks: DiskListResult =
        serde_json::from_str(&run("List disks", "recpart list-disks --json".to_string(), Budget::Stage)?)
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
        Budget::Stage,
    )?)
    .context("parsing recpart apply JSON")?;
    let target = applied.handoff.install_target.clone();
    let t = shell_quote(&target);

    run("Extract rootfs (recstrap)", format!("recstrap --force {t}"), Budget::Stage)?;
    run(
        "Generate fstab (recfstab)",
        format!(
            "recfstab {t} >> {}",
            shell_quote(&format!("{target}/etc/fstab"))
        ),
        Budget::Stage,
    )?;
    // Every medium extracts the same base system; the release medium also
    // carries the default software set, which recsetup's software step below
    // installs through Recipe.
    let list = shell_quote(&format!("{}/list", install_tools::DEFAULT_SET_DIR));
    let carries = run(
        "Verify the selected medium's default software set",
        format!("if [ -f {list} ]; then echo yes; else echo no; fi"),
        Budget::Stage,
    )?;
    let desktop = system.flavour == distro_contract::profile::Flavour::Desktop;
    anyhow::ensure!(
        (carries.trim() == "yes") == desktop,
        "the selected {} installer {} a default software set",
        system.flavour,
        if desktop { "lacks" } else { "carries" }
    );
    // Exercise the shipped setup frontend. It installs the medium's default
    // software set through Recipe (when the medium carries one), delegates
    // filesystem policy to the setup program installed by the system recipe
    // and, for A/B, the boot layout to the live medium's recab.
    let mut setup = format!(
        "recsetup apply --root {t} --hostname {} --root-password --passwords-stdin --machine-id --boot {} --kernel {} --options {}",
        shell_quote(&variant.contract.identity.default_hostname),
        system.mode,
        shell_quote(&format!("{LIVE_MEDIA_DIR}/{KERNEL_ISO_PATH}")),
        shell_quote(&format!("{VGA_CONSOLE} {SERIAL_CONSOLE}")),
    );
    // The installed system's initramfs, the ISO carries
    // it beside its own live one (that one only finds the live medium); recsetup
    // defaults to it, named here so the scenario fails loudly if the ISO lacks it.
    setup.push_str(&format!(
        " --initrd {}",
        shell_quote(&format!("{LIVE_MEDIA_DIR}/{INITRAMFS_INSTALLED_ISO_PATH}"))
    ));
    match system.mode {
        InstallMode::Ab => {
            let slots = &applied.handoff.mode_context;
            let a = slots
                .slot_a_device
                .as_ref()
                .context("recpart reported no root A device")?;
            let b = slots
                .slot_b_device
                .as_ref()
                .context("recpart reported no root B device")?;
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
    let passwords = format!(
        "printf '%s\\n' {}",
        shell_quote(&format!("root:{}", system.root_password))
    );
    // The release medium carries no package cache: recsetup's software step fetches the
    // release's parts with `recupdate fetch-parts`. The scenario gives it a channel on the
    // host, signed with a throwaway key that is the only one the live system trusts for
    // this run (recsetup hands its environment on to recupdate).
    let mut channel = None;
    let mut env = String::new();
    if desktop {
        let gateway = run(
            "Find the host address the live session reaches (its default gateway)",
            "ip -4 route show default | awk '{print $3; exit}'".to_string(),
            Budget::Command,
        )?;
        let started = Channel::start(system.release_dir, &gateway)?;
        run(
            "Trust the run's throwaway release key in the live session",
            started.guest_setup(),
            Budget::Command,
        )?;
        env = format!("{} ", started.guest_env());
        // A part that is missing must stop the software step with the way forward: the
        // folder holds the signed manifest and none of the parts. recsetup keeps nothing
        // of a step that failed, so the real run below starts the same plan.
        let folder = shell_quote(started.guest_manifest_dir());
        run(
            "A missing package part stops recsetup with the way forward",
            format!(
                "out=$({passwords} | {env}{setup} --parts-from {folder} 2>&1); rc=$?; printf '%s\\n' \"$out\"; \
                 [ \"$rc\" -ne 0 ] && printf '%s\\n' \"$out\" | grep -q 'Parts still to fetch: parts-' \
                 && printf '%s\\n' \"$out\" | grep -q -e '--parts-from DIR'"
            ),
            Budget::Stage,
        )?;
        channel = Some(started);
    }
    run(
        "Configure and boot the installed system (recsetup)",
        format!("{passwords} | {env}{setup}"),
        Budget::Setup,
    )?;
    drop(channel);
    Ok((target, steps))
}
