//! Phase 5: Bootloader installation steps.
//!
//! Steps 16-18: Copy/install initramfs, install bootloader, enable services.
//!
//! # Cheat Prevention
//!
//! Boot-critical steps that MUST work:
//! - initramfs MUST be copied from ISO (no initramfs = kernel panic)
//! - boot entry MUST have correct root UUID (wrong UUID = VFS panic)
//! - Essential services MUST be enabled (no getty = no login prompt)

use super::{CheckResult, Step, StepResult};
use crate::distro::DistroContext;
use crate::executor::Executor;
use anyhow::Result;
use leviso_cheat_guard::cheat_ensure;
use distro_spec::shared::boot::{BootEntry, LoaderConfig};
use std::time::{Duration, Instant};

/// Step 16: Copy/install initramfs from ISO
///
/// Copies the pre-built initramfs from the ISO to the ESP.
/// The initramfs was generated during ISO build with generic drivers.
pub struct GenerateInitramfs;

impl Step for GenerateInitramfs {
    fn num(&self) -> usize { 16 }
    fn name(&self) -> &str { "Copy/Install Initramfs" }
    fn ensures(&self) -> &str {
        "Initramfs exists at /boot/initramfs.img with drivers for installed hardware"
    }

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let step_start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // ═══════════════════════════════════════════════════════════════════════
        // KERNEL COPY: ISO → ESP
        // ═══════════════════════════════════════════════════════════════════════
        // The rootfs (EROFS) doesn't include the kernel (it's on the ISO for live boot).
        // We need to copy it to the ESP where systemd-boot can find it.
        let kernel_cmd = "cp /media/cdrom/boot/vmlinuz /mnt/boot/vmlinuz";
        let cmd_start = Instant::now();
        let kernel_copy = executor.exec(kernel_cmd, Duration::from_secs(10))?;
        result.log_command(kernel_cmd, kernel_copy.exit_code, &kernel_copy.output, cmd_start.elapsed());

        cheat_ensure!(
            kernel_copy.success(),
            protects = "Kernel is copied from ISO to ESP for boot",
            severity = "CRITICAL",
            cheats = ["Skip kernel copy", "Assume kernel exists in rootfs", "Accept copy failure"],
            consequence = "No kernel on ESP, systemd-boot can't find it, system won't boot",
            "Failed to copy kernel from ISO to ESP: {}", kernel_copy.output
        );

        // Get kernel size as evidence - skeptics want to see actual bytes
        let cmd_start = Instant::now();
        let kernel_size = executor.exec("stat -c '%s' /mnt/boot/vmlinuz", Duration::from_secs(5))?;
        result.log_command("stat -c '%s' /mnt/boot/vmlinuz", kernel_size.exit_code, &kernel_size.output, cmd_start.elapsed());

        let kernel_bytes: u64 = kernel_size.output.trim().parse().unwrap_or(0);
        let kernel_mb = kernel_bytes as f64 / 1024.0 / 1024.0;

        // SKEPTIC-PROOF: Show actual size, not just "exists"
        if kernel_bytes > 1_000_000 {
            result.pass("kernel on ESP", format!("{:.1}MB at /mnt/boot/vmlinuz", kernel_mb));
        } else {
            result.fail(
                "kernel on ESP",
                "kernel > 1MB",
                format!("kernel is only {} bytes (corrupt or empty?)", kernel_bytes),
            );
        }

        // ═══════════════════════════════════════════════════════════════════════
        // INITRAMFS COPY: ISO → ESP
        // ═══════════════════════════════════════════════════════════════════════
        let copy_cmd = "cp /media/cdrom/boot/initramfs-installed.img /mnt/boot/initramfs.img";
        let cmd_start = Instant::now();
        let copy_result = executor.exec(copy_cmd, Duration::from_secs(30))?;
        result.log_command(copy_cmd, copy_result.exit_code, &copy_result.output, cmd_start.elapsed());

        cheat_ensure!(
            copy_result.success(),
            protects = "initramfs is copied from ISO to ESP",
            severity = "CRITICAL",
            cheats = ["Skip initramfs copy", "Accept missing initramfs on ISO"],
            consequence = "No initramfs, system won't boot. Rebuild ISO with 'leviso build'",
            "Failed to copy initramfs from ISO: {}", copy_result.output
        );

        // Get initramfs size as evidence
        let cmd_start = Instant::now();
        let initramfs_size = executor.exec("stat -c '%s' /mnt/boot/initramfs.img", Duration::from_secs(5))?;
        result.log_command("stat -c '%s' /mnt/boot/initramfs.img", initramfs_size.exit_code, &initramfs_size.output, cmd_start.elapsed());

        let initramfs_bytes: u64 = initramfs_size.output.trim().parse().unwrap_or(0);
        let initramfs_mb = initramfs_bytes as f64 / 1024.0 / 1024.0;

        // SKEPTIC-PROOF: An initramfs under 10MB is suspiciously small
        if initramfs_bytes > 10_000_000 {
            result.pass("initramfs on ESP", format!("{:.1}MB at /mnt/boot/initramfs.img", initramfs_mb));
        } else {
            result.fail(
                "initramfs on ESP",
                "initramfs > 10MB (typical: 30-60MB)",
                format!("initramfs is only {:.1}MB (missing drivers?)", initramfs_mb),
            );
        }

        result.duration = step_start.elapsed();
        Ok(result)
    }
}

/// Step 17: Install systemd-boot bootloader
pub struct InstallBootloader;

impl Step for InstallBootloader {
    fn num(&self) -> usize { 17 }
    fn name(&self) -> &str { "Install Bootloader" }
    fn ensures(&self) -> &str {
        "System is bootable via systemd-boot with correct kernel and root"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check if systemd-boot EFI files exist in the tarball
        let efi_check = executor.exec_chroot(
            "/mnt",
            "test -d /usr/lib/systemd/boot/efi",
            Duration::from_secs(5),
        )?;

        if efi_check.exit_code != 0 {
            // EFI files not present = TARBALL IS BROKEN
            // A daily driver OS MUST be able to boot. No "manual bootloader setup" escape hatch.
            result.add_check(
                "systemd-boot files present",
                CheckResult::Fail {
                    expected: "/usr/lib/systemd/boot/efi exists".to_string(),
                    actual: "systemd-boot EFI files missing from tarball".to_string(),
                },
            );
            result.duration = start.elapsed();
            return Ok(result);
        } else {
            // Install systemd-boot
            // ESP is at /boot (FAT32)
            // --esp-path=/boot: REQUIRED in chroot - mount detection doesn't work
            // --no-variables: Skip EFI variable setup (not available in chroot)
            let bootctl_result = executor.exec_chroot(
                "/mnt",
                "bootctl install --esp-path=/boot --no-variables",
                Duration::from_secs(30),
            )?;

            // CHEAT GUARD: bootctl MUST succeed if EFI files exist
            cheat_ensure!(
                bootctl_result.success(),
                protects = "systemd-boot bootloader is installed",
                severity = "CRITICAL",
                cheats = [
                    "Accept any bootctl exit code",
                    "Skip bootloader installation",
                    "Ignore EFI setup errors"
                ],
                consequence = "No bootloader, UEFI can't find boot entry, system won't start",
                "bootctl install failed (exit {}): {}", bootctl_result.exit_code, bootctl_result.output
            );

            result.add_check("systemd-boot installed", CheckResult::pass("bootctl install exit 0"));

            // Create EFI boot entry using efibootmgr
            // Run from live environment (not chroot) since efibootmgr needs /sys/firmware/efi/efivars
            // This creates a real UEFI boot entry instead of relying on fallback boot path
            //
            // efivarfs should already be mounted rw by systemd mount unit (sys-firmware-efi-efivars.mount)
            // If it's not mounted or not writable, that's a product bug we want to catch
            let _ = executor.exec(
                "mount -t efivarfs -o rw efivarfs /sys/firmware/efi/efivars || mount -o remount,rw /sys/firmware/efi/efivars",
                Duration::from_secs(5),
            )?;
            let efi_label = ctx.efi_entry_label();
            let efi_entry = executor.exec(
                &format!("efibootmgr --create --disk /dev/vda --part 1 --label '{}' --loader '\\EFI\\systemd\\systemd-bootx64.efi' 2>&1", efi_label),
                Duration::from_secs(10),
            )?;

            // ANTI-CHEAT: EFI boot entry MUST be created for proper UEFI boot
            // Now that we boot through real UEFI, this should always work
            cheat_ensure!(
                efi_entry.output.contains("BootOrder") || efi_entry.output.contains("Boot0"),
                protects = "EFI boot entry created for installed system",
                severity = "CRITICAL",
                cheats = [
                    "Rely on fallback path only",
                    "Use --no-variables",
                    "Skip efibootmgr entirely"
                ],
                consequence = "No EFI entry = depends on fallback = may not boot on real hardware",
                "efibootmgr failed: {}", efi_entry.output.trim()
            );

            result.add_check("EFI boot entry created", CheckResult::pass(format!("efibootmgr created {} entry", efi_label)));
        }

        // Get root partition UUID for boot entry
        let uuid_result = executor.exec("blkid -s UUID -o value /dev/vda2", Duration::from_secs(5))?;
        let root_uuid = uuid_result.output.trim();

        // Create loader.conf (goes in ESP at /boot)
        let loader_config = LoaderConfig::with_defaults(ctx.id())
            .disable_editor()  // Disable for security
            .with_console_mode("max");
        executor.write_file("/mnt/boot/loader/loader.conf", &loader_config.to_loader_conf())?;

        // Create boot entry with serial console output for testing
        // Production installs would use default_boot_entry().set_root() without console settings
        let mut boot_entry = BootEntry::with_defaults(
            ctx.id(),
            ctx.name(),
            "vmlinuz",
            "initramfs.img",
        ).set_root(format!("UUID={}", root_uuid));
        // Add console settings for QEMU serial output (required for test automation)
        // rd.debug enables initrd debug logging to show exactly what systemd/udev is doing
        // systemd.log_level=debug shows detailed systemd unit activation
        // rd.shell=1 drops to shell on failure (disabled - causes timeout issues)
        boot_entry.options = format!(
            "root=UUID={} rw console=tty0 console=ttyS0,115200n8 rd.info rd.debug systemd.log_level=debug",
            root_uuid
        );
        let entry_path = boot_entry.entry_path(); // /boot/loader/entries/X.conf
        executor.write_file(&format!("/mnt{}", entry_path), &boot_entry.to_entry_file())?;

        // Verify boot entry exists and has correct content
        let verify = executor.exec(
            &format!("cat /mnt{}", entry_path),
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: Boot entry MUST have all required fields
        // Check for linux (kernel path)
        cheat_ensure!(
            verify.output.contains("linux") && verify.output.contains("/vmlinuz"),
            protects = "Boot entry has correct kernel path",
            severity = "CRITICAL",
            cheats = [
                "Only check file exists",
                "Skip content validation",
                "Accept any linux line"
            ],
            consequence = "Wrong kernel path = kernel not found = won't boot",
            "Boot entry missing kernel path:\n{}", verify.output
        );

        // Check for initrd (initramfs path)
        cheat_ensure!(
            verify.output.contains("initrd") && verify.output.contains("/initramfs"),
            protects = "Boot entry has correct initramfs path",
            severity = "CRITICAL",
            cheats = [
                "Only check file exists",
                "Skip initramfs line check"
            ],
            consequence = "Wrong initramfs path = no initramfs = kernel panic at mount",
            "Boot entry missing initramfs path:\n{}", verify.output
        );

        // Check for root UUID in options
        cheat_ensure!(
            verify.output.contains(root_uuid),
            protects = "Boot entry points to correct root partition",
            severity = "CRITICAL",
            cheats = [
                "Use hardcoded UUID",
                "Skip UUID verification",
                "Only check if entry file exists"
            ],
            consequence = "Wrong root UUID in boot entry, VFS panic (cannot mount root)",
            "Boot entry missing or has wrong UUID. Expected {}, got:\n{}", root_uuid, verify.output
        );

        result.add_check("Boot entry content verified", CheckResult::pass(
            format!("linux=/vmlinuz, initrd=/initramfs.img, root=UUID={}", root_uuid)
        ));

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 18: Enable essential services
pub struct EnableServices;

impl Step for EnableServices {
    fn num(&self) -> usize { 18 }
    fn name(&self) -> &str { "Enable Services" }
    fn ensures(&self) -> &str {
        "Essential services (networkd, sshd, getty) start automatically on boot"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Get services to enable from distro context
        let enabled_services = ctx.enabled_services();

        // Enable each service using the distro-specific command
        for (service_name, target, is_required) in &enabled_services {
            // Check if service exists
            let check_cmd = ctx.check_service_exists_cmd(service_name);
            let check_result = executor.exec_chroot("/mnt", &check_cmd, Duration::from_secs(5))?;

            if !check_result.output.contains(service_name) {
                if *is_required {
                    result.add_check(
                        &format!("{} enabled", service_name),
                        CheckResult::Fail {
                            expected: format!("{} service exists", service_name),
                            actual: "Service not found".to_string(),
                        },
                    );
                } else {
                    result.add_check(
                        &format!("{} enabled", service_name),
                        CheckResult::Skip(format!("{} not available (optional)", service_name)),
                    );
                }
                continue;
            }

            // Enable the service
            let enable_cmd = ctx.enable_service_cmd(service_name, target);
            let enable_result = executor.exec_chroot("/mnt", &enable_cmd, Duration::from_secs(10))?;

            if enable_result.success() {
                result.add_check(&format!("{} enabled", service_name), CheckResult::pass("enabled"));
            } else {
                result.add_check(
                    &format!("{} enabled", service_name),
                    CheckResult::Fail {
                        expected: "enable success".to_string(),
                        actual: format!("exit {}: {}", enable_result.exit_code, enable_result.output.trim()),
                    },
                );
            }
        }

        // Enable serial console getty for testing using distro-specific command
        let serial_cmd = ctx.enable_serial_getty_cmd();
        let serial_result = executor.exec_chroot("/mnt", &serial_cmd, Duration::from_secs(10))?;

        if serial_result.success() {
            result.add_check("serial getty enabled", CheckResult::pass("serial console configured"));
        } else {
            result.add_check(
                "serial getty enabled",
                CheckResult::Fail {
                    expected: "serial getty enable success".to_string(),
                    actual: format!("exit {}: {}", serial_result.exit_code, serial_result.output),
                },
            );
        }

        // NOTE: No autologin - installed system should behave like a normal install.
        // The test harness must handle normal login (username + password).
        // Autologin would mask login-related bugs and is not Arch-like behavior.

        // PRE-REBOOT VERIFICATION: Catch issues before rebooting saves debugging time
        // Verify kernel exists
        let kernel_verify = executor.exec("test -f /mnt/boot/vmlinuz", Duration::from_secs(5))?;
        cheat_ensure!(
            kernel_verify.success(),
            protects = "Kernel exists on ESP before reboot",
            severity = "CRITICAL",
            cheats = ["Skip pre-reboot verification"],
            consequence = "System won't boot - no kernel",
            "Kernel not found at /mnt/boot/vmlinuz"
        );
        result.add_check("Pre-reboot: kernel present", CheckResult::pass("/mnt/boot/vmlinuz exists"));

        // Verify initramfs exists
        let initramfs_verify = executor.exec("test -f /mnt/boot/initramfs.img", Duration::from_secs(5))?;
        cheat_ensure!(
            initramfs_verify.success(),
            protects = "Initramfs exists on ESP before reboot",
            severity = "CRITICAL",
            cheats = ["Skip pre-reboot verification"],
            consequence = "System won't boot - no initramfs",
            "Initramfs not found at /mnt/boot/initramfs.img"
        );
        result.add_check("Pre-reboot: initramfs present", CheckResult::pass("/mnt/boot/initramfs.img exists"));

        // Verify root password is set (not locked)
        let password_verify = executor.exec(
            "grep '^root:' /mnt/etc/shadow | grep -v ':!:' | grep -v ':\\*:'",
            Duration::from_secs(5),
        )?;
        cheat_ensure!(
            password_verify.success(),
            protects = "Root password is set before reboot",
            severity = "CRITICAL",
            cheats = ["Skip pre-reboot verification"],
            consequence = "Cannot login after reboot - account locked",
            "Root password not set in /mnt/etc/shadow"
        );
        result.add_check("Pre-reboot: root password set", CheckResult::pass("root has hash in /etc/shadow"));

        // Verify fstab has boot entry
        let fstab_verify = executor.exec(
            "grep '/boot' /mnt/etc/fstab",
            Duration::from_secs(5),
        )?;
        cheat_ensure!(
            fstab_verify.success(),
            protects = "fstab has ESP mount entry before reboot",
            severity = "CRITICAL",
            cheats = ["Skip pre-reboot verification"],
            consequence = "ESP won't be mounted after reboot - kernel updates will fail",
            "No /boot entry in /mnt/etc/fstab"
        );
        result.add_check("Pre-reboot: fstab has /boot", CheckResult::pass(fstab_verify.output.trim()));

        // Copy test instrumentation to installed system
        // This enables ___SHELL_READY___ and ___PROMPT___ markers after reboot
        // Without this, the installed system won't have the markers that install-tests requires
        let test_script = ctx.test_instrumentation_source();
        let script_name = format!("00-{}-test.sh", ctx.id());
        let script_path = format!("/mnt/etc/profile.d/{}", script_name);
        executor.write_file(&script_path, test_script)?;
        executor.exec_ok(&format!("chmod +x {}", script_path), Duration::from_secs(5))?;
        result.add_check("Test instrumentation installed", CheckResult::pass(format!("/etc/profile.d/{}", script_name)));

        // Unmount partitions (EFI first, then root)
        let _ = executor.exec("umount /mnt/boot", Duration::from_secs(5));
        let _ = executor.exec("umount /mnt", Duration::from_secs(5));

        result.add_check("Partitions unmounted", CheckResult::pass("umount /mnt/boot and /mnt"));

        result.duration = start.elapsed();
        Ok(result)
    }
}
