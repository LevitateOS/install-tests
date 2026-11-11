//! The install scenario's installation: the shipped tools, run over SSH in
//! the live system in the order the live install docs give them (recpart,
//! recstrap, recfstab, recsetup), with test-specific choices supplied as inputs.

use anyhow::{Context, Result};
use recinit::live::{INITRAMFS_INSTALLED_ISO_PATH, KERNEL_ISO_PATH, LIVE_MEDIA_DIR};
use recpart::InstallMode;
use recpart::types::{ApplyResult, CONFIRM_TOKEN, DiskListResult};
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
    pub update_fixture: bool,
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
    if system.update_fixture {
        anyhow::ensure!(
            system.mode == InstallMode::Ab,
            "update fixture requires an A/B installation"
        );
        run("Mount explicit update fixture", "mkdir -p /run/recab-p13-media && mount -o ro /dev/disk/by-label/RECAB_P13 /run/recab-p13-media".into())?;
        let original: distro_contract::release::Policy = serde_json::from_str(&run(
            "Read original release policy",
            format!("cat {t}/usr/lib/recab/update-source.json"),
        )?)?;
        let fixture: distro_contract::release::Policy = serde_json::from_str(&run(
            "Read fixture public source policy",
            "cat /run/recab-p13-media/source.json".into(),
        )?)?;
        fixture.validate()?;
        anyhow::ensure!(
            original.channel == fixture.channel
                && original.sequence == fixture.sequence
                && fixture.source.is_some()
                && fixture.trust.is_some(),
            "fixture must configure this original release, not change its identity/sequence"
        );
        run(
            "Capture original release and seed public test policy",
            format!(
                "recab baseline --root {t} && cp /run/recab-p13-media/source.json {t}/usr/lib/recab/update-source.json && cp /run/recab-p13-media/acquire.rhai {t}/usr/share/recab/update-acquire.rhai && umount /run/recab-p13-media"
            ),
        )?;
        run(
            "Seed local fidelity probes before either root is sealed",
            format!(
                r#"mkdir -p {t}/etc/p13-local && printf 'local hardlinks and metadata\n' > {t}/etc/p13-local/hard-a && chmod 640 {t}/etc/p13-local/hard-a && chown 1001:1002 {t}/etc/p13-local/hard-a && ln {t}/etc/p13-local/hard-a {t}/etc/p13-local/hard-b && ln -s hard-a {t}/etc/p13-local/symlink && dd if=/dev/zero of={t}/etc/p13-local/sparse bs=1 count=1 seek=16777215 2>/dev/null && raw=$(printf 'raw-\377') && printf 'non-UTF8 system name\n' > {t}/etc/p13-local/"$raw""#
            ),
        )?;
    }
    // Exercise the shipped setup frontend. It delegates filesystem policy to
    // the setup program installed by the system recipe and boot state to recab.
    let mut setup = format!(
        "recsetup apply --root {t} --hostname {} --root-password --passwords-stdin --machine-id --boot {} --kernel {} --options {}",
        shell_quote(&variant.contract.identity.default_hostname),
        system.mode,
        shell_quote(&format!("{LIVE_MEDIA_DIR}/{KERNEL_ISO_PATH}")),
        shell_quote(&format!("{VGA_CONSOLE} {SERIAL_CONSOLE}")),
    );
    if system.update_fixture {
        setup.push_str(" --service sshd");
    }
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
