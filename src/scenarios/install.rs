//! The install scenario's installation: the shipped tools, run over SSH in
//! the live system in the order the live install docs give them (recpart,
//! recstrap, recfstab, recchroot), then the test system's own setup (boot
//! entries, hostname, root password, serial getty).

use anyhow::{Context, Result};
use distro_spec::shared::live_boot::LIVE_MEDIA_DIR;
use distro_spec::shared::{SERIAL_CONSOLE, VGA_CONSOLE};
use recpart::types::{ApplyResult, DiskListResult, CONFIRM_TOKEN};
use recpart::InstallMode;
use recshuttle::Remote;

use crate::DistroContext;

/// What the test system needs beyond the shipped tools.
pub struct TestSystem<'a> {
    pub mode: InstallMode,
    pub root_password: &'a str,
    /// Whether the distro boots through an initramfs (OpenRC/Acorn does not).
    pub uses_initramfs: bool,
}

/// Quote `value` as one word for the guest's POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Install onto the guest's first writable disk and leave it mounted at the
/// install target. Returns the target and the number of steps run.
pub fn install(
    remote: &Remote,
    ctx: &dyn DistroContext,
    system: &TestSystem<'_>,
) -> Result<(String, usize)> {
    let mut steps = 0usize;
    let mut run = |what: &str, cmd: String| -> Result<String> {
        println!("    -> {what}");
        steps += 1;
        remote.run(what, &cmd)
    };

    let disks: DiskListResult = serde_json::from_str(&run(
        "List disks",
        "recpart list-disks --json".to_string(),
    )?)
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

    run(
        "Extract rootfs (recstrap)",
        format!("recstrap --force {t}"),
    )?;
    run(
        "Generate fstab (recfstab)",
        format!("recfstab {t} >> {}", shell_quote(&format!("{target}/etc/fstab"))),
    )?;
    let mut boot_files = vec!["vmlinuz"];
    if system.uses_initramfs {
        boot_files.push("initramfs.img");
    }
    for file in &boot_files {
        run(
            &format!("Copy {file} to the ESP"),
            format!(
                "cp {} {}",
                shell_quote(&format!("{LIVE_MEDIA_DIR}/boot/{file}")),
                shell_quote(&format!("{target}/boot/{file}"))
            ),
        )?;
    }
    run(
        "Install bootloader (recchroot)",
        format!("recchroot {t} -- {}", ctx.install_bootloader_cmd()),
    )?;

    // Boot entries name their root by PARTUUID, which the kernel resolves
    // with or without an initramfs.
    let mut entries = Vec::new();
    let default_entry = match system.mode {
        InstallMode::Mutable => {
            let root = applied
                .partition_map
                .iter()
                .find(|p| p.mountpoint == "/")
                .context("recpart planned no root partition")?;
            let device = applied
                .mounted
                .iter()
                .find(|m| m.path == target)
                .map(|m| m.device.clone())
                .with_context(|| format!("recpart did not mount {} at {target}", root.name))?;
            entries.push((format!("{}.conf", ctx.id()), ctx.name().to_string(), device, "rw"));
            format!("{}.conf", ctx.id())
        }
        InstallMode::Ab => {
            let slots = &applied.handoff.mode_context;
            let installed = slots
                .install_target_slot
                .clone()
                .context("recpart reported no install slot")?
                .to_lowercase();
            for (slot, device) in [("a", &slots.slot_a_device), ("b", &slots.slot_b_device)] {
                let device = device.clone().with_context(|| format!("no slot {slot} device"))?;
                entries.push((
                    format!("{}-{slot}.conf", ctx.id()),
                    format!("{} (Slot {})", ctx.name(), slot.to_uppercase()),
                    device,
                    "ro",
                ));
            }
            format!("{}-{installed}.conf", ctx.id())
        }
    };

    write_file(
        &mut run,
        &format!("{target}/boot/loader/loader.conf"),
        &format!("default {default_entry}\ntimeout 5\neditor 0\nconsole-mode max\n"),
    )?;
    for (name, title, device, mount_mode) in &entries {
        let partuuid = run(
            &format!("Read PARTUUID of {device}"),
            format!("blkid -s PARTUUID -o value {}", shell_quote(device)),
        )?
        .trim()
        .to_string();
        let mut entry = format!("title {title}\nlinux /vmlinuz\n");
        if system.uses_initramfs {
            entry.push_str("initrd /initramfs.img\n");
        }
        entry.push_str(&format!(
            "options root=PARTUUID={partuuid} {mount_mode} rootwait {VGA_CONSOLE} {SERIAL_CONSOLE}\n"
        ));
        write_file(&mut run, &format!("{target}/boot/loader/entries/{name}"), &entry)?;
    }

    write_file(
        &mut run,
        &format!("{target}/etc/hostname"),
        &format!("{}\n", ctx.default_hostname()),
    )?;
    run(
        "Set root password",
        format!(
            "printf '%s\\n' {} | recchroot {t} -- chpasswd",
            shell_quote(&format!("root:{}", system.root_password))
        ),
    )?;
    run(
        "Enable serial getty",
        format!(
            "recchroot {t} -- sh -c {}",
            shell_quote(&ctx.enable_serial_getty_cmd())
        ),
    )?;

    Ok((target, steps))
}

fn write_file(
    run: &mut impl FnMut(&str, String) -> Result<String>,
    path: &str,
    content: &str,
) -> Result<()> {
    let dir = path.rsplit_once('/').map_or(".", |(dir, _)| dir);
    run(
        &format!("Write {path}"),
        format!(
            "mkdir -p {} && printf '%s' {} > {}",
            shell_quote(dir),
            shell_quote(content),
            shell_quote(path)
        ),
    )?;
    Ok(())
}
