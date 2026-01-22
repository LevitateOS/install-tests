//! Phase 5: Bootloader installation steps.
//!
//! Steps 16-18: Generate initramfs, install bootloader, enable services.
//!
//! # Cheat Prevention
//!
//! Boot-critical steps that MUST work:
//! - initramfs MUST be generated (no initramfs = kernel panic)
//! - boot entry MUST have correct root UUID (wrong UUID = VFS panic)
//! - Essential services MUST be enabled (no getty = no login prompt)

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use cheat_guard::cheat_ensure;
use distro_spec::levitate::{BootEntry, LoaderConfig, ENABLED_SERVICES};
use std::time::{Duration, Instant};

/// Step 16: Generate initramfs with dracut
///
/// Dracut detects installed kernel and hardware, then generates an initramfs
/// containing drivers needed to boot the system.
pub struct GenerateInitramfs;

impl Step for GenerateInitramfs {
    fn num(&self) -> usize { 16 }
    fn name(&self) -> &str { "Generate Initramfs" }
    fn ensures(&self) -> &str {
        "Initramfs exists at /boot/initramfs.img with drivers for installed hardware"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // FIRST: Copy kernel from ISO to ESP
        // The squashfs doesn't include the kernel (it's on the ISO for live boot).
        // We need to copy it to the ESP where systemd-boot can find it.
        // ISO is mounted at /media/cdrom, ESP is mounted at /mnt/boot
        let kernel_copy = console.exec(
            "cp /media/cdrom/boot/vmlinuz /mnt/boot/vmlinuz",
            Duration::from_secs(10),
        )?;

        // CHEAT GUARD: Kernel copy MUST succeed
        cheat_ensure!(
            kernel_copy.success(),
            protects = "Kernel is copied from ISO to ESP for boot",
            severity = "CRITICAL",
            cheats = [
                "Skip kernel copy",
                "Assume kernel exists in squashfs",
                "Accept copy failure"
            ],
            consequence = "No kernel on ESP, systemd-boot can't find it, system won't boot",
            "Failed to copy kernel from ISO to ESP: {}", kernel_copy.output
        );

        result.add_check(
            "kernel copied to ESP",
            CheckResult::Pass("/media/cdrom/boot/vmlinuz -> /mnt/boot/vmlinuz".to_string()),
        );

        // Now verify kernel exists (this check runs in chroot, /boot = ESP)
        let kernel_check = console.exec_chroot(
            "test -f /boot/vmlinuz",
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: Kernel MUST exist before generating initramfs
        cheat_ensure!(
            kernel_check.exit_code == 0,
            protects = "Kernel exists on ESP for initramfs generation",
            severity = "CRITICAL",
            cheats = [
                "Skip kernel check",
                "Generate initramfs without kernel",
                "Assume kernel exists"
            ],
            consequence = "No kernel to boot, system completely unbootable",
            "Kernel not found at /boot/vmlinuz on ESP after copy"
        );

        result.add_check(
            "kernel verified on ESP",
            CheckResult::Pass("/boot/vmlinuz found on ESP".to_string()),
        );

        // Check if dracut is available
        let dracut_check = console.exec_chroot(
            "which dracut",
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: dracut MUST be available
        cheat_ensure!(
            dracut_check.exit_code == 0,
            protects = "initramfs generator is available",
            severity = "CRITICAL",
            cheats = [
                "Skip dracut check",
                "Use pre-built initramfs",
                "Assume dracut exists"
            ],
            consequence = "Cannot generate initramfs, system won't boot",
            "dracut not found - tarball must include dracut package"
        );

        result.add_check(
            "dracut available",
            CheckResult::Pass("dracut found".to_string()),
        );

        // Get kernel version
        let kver_result = console.exec_chroot(
            "ls /usr/lib/modules/ | head -1",
            Duration::from_secs(5),
        )?;
        let kernel_version = kver_result.output.trim();

        // CHEAT GUARD: Kernel modules MUST exist for dracut
        cheat_ensure!(
            !kernel_version.is_empty(),
            protects = "Kernel modules exist for initramfs generation",
            severity = "CRITICAL",
            cheats = [
                "Skip modules check",
                "Accept empty modules directory",
                "Use hardcoded kernel version"
            ],
            consequence = "dracut fails, no initramfs, system won't boot",
            "No kernel modules found in /usr/lib/modules/"
        );

        result.add_check(
            "kernel modules present",
            CheckResult::Pass(format!("modules for kernel {}", kernel_version)),
        );

        // Generate initramfs with dracut
        // --force: overwrite existing initramfs
        // --no-hostonly: include all drivers, not just for current hardware
        // --omit: skip modules that have missing dependencies in minimal squashfs:
        //   - fips: requires sha512hmac from hmaccalc package
        //   - bluetooth, crypt, nfs: not needed for basic VM boot
        //   - rdma: InfiniBand, requires /etc/rdma/mlx4.conf
        //   - systemd-sysusers, systemd-journald, systemd-initrd, dracut-systemd:
        //     Complex dependency chain, the base systemd module works without them
        //
        // Uses STREAMING with STALL DETECTION:
        // - Fails immediately on dracut error patterns
        // - No hard timeout - dracut can take 6+ minutes total
        // - Only fails if dracut stalls (no output for 180s)
        //   Dracut's final stages (cpio archive creation) can be silent for minutes
        let dracut_error_patterns = &[
            "dracut[F]:",  // Fatal errors
            "FATAL:",
        ];
        // NOTE: --add-drivers is no longer needed here because the base system
        // now includes /etc/dracut.conf.d/levitate.conf with:
        //   add_drivers+=" ext4 vfat "
        //   hostonly="no"
        // This was moved to leviso (TEAM_088) so recstrap installs include it
        let dracut_result = console.exec_chroot_streaming(
            &format!(
                "dracut --force \
                 --omit 'fips bluetooth crypt nfs rdma systemd-sysusers systemd-journald systemd-initrd dracut-systemd' \
                 /boot/initramfs.img {}",
                kernel_version
            ),
            Duration::from_secs(180),  // Stall timeout - dracut's cpio creation can be very quiet
            dracut_error_patterns,
        )?;

        // CHEAT GUARD: dracut MUST succeed
        let dracut_error_msg = if dracut_result.stalled {
            format!("dracut STALLED (no output for 60s): {}", dracut_result.output)
        } else if dracut_result.aborted_on_error {
            format!("dracut FAILED on error pattern: {}", dracut_result.output)
        } else {
            format!("dracut failed (exit {}): {}", dracut_result.exit_code, dracut_result.output)
        };
        cheat_ensure!(
            dracut_result.success(),
            protects = "initramfs is generated with required drivers",
            severity = "CRITICAL",
            cheats = [
                "Accept any dracut exit code",
                "Skip initramfs generation",
                "Ignore dracut errors"
            ],
            consequence = "No initramfs, kernel panic at boot (VFS: cannot open root device)",
            "{}", dracut_error_msg
        );

        result.add_check(
            "initramfs generated",
            CheckResult::Pass(format!("/boot/initramfs.img for kernel {}", kernel_version)),
        );

        // Verify initramfs was created
        let verify = console.exec_chroot(
            "test -f /boot/initramfs.img && ls -lh /boot/initramfs.img",
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: initramfs MUST exist after dracut runs
        cheat_ensure!(
            verify.success(),
            protects = "initramfs file was actually written to disk",
            severity = "CRITICAL",
            cheats = [
                "Trust dracut exit code without file check",
                "Skip verification",
                "Accept any file at path"
            ],
            consequence = "dracut claims success but no file, kernel panic at boot",
            "initramfs not found at /boot/initramfs.img after dracut"
        );

        result.add_check(
            "initramfs verified",
            CheckResult::Pass(verify.output.trim().to_string()),
        );

        result.duration = start.elapsed();
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

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check if systemd-boot EFI files exist in the tarball
        let efi_check = console.exec_chroot(
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
            let bootctl_result = console.exec_chroot(
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

            result.add_check(
                "systemd-boot installed",
                CheckResult::Pass("bootctl install succeeded".to_string()),
            );
        }

        // Get root partition UUID for boot entry
        let uuid_result = console.exec("blkid -s UUID -o value /dev/vda2", Duration::from_secs(5))?;
        let root_uuid = uuid_result.output.trim();

        // Create loader.conf using levitate-spec (goes in ESP at /boot)
        let mut loader_config = LoaderConfig::default();
        loader_config.editor = false; // Disable for security
        loader_config.console_mode = Some("max".to_string());
        console.write_file("/mnt/boot/loader/loader.conf", &loader_config.to_loader_conf())?;

        // Create boot entry with serial console output for testing
        // Production installs would use BootEntry::with_root() without console settings
        let mut boot_entry = BootEntry::with_root(format!("UUID={}", root_uuid));
        // Add console settings for QEMU serial output (required for test automation)
        boot_entry.options = format!(
            "root=UUID={} rw console=tty0 console=ttyS0,115200n8",
            root_uuid
        );
        let entry_path = boot_entry.entry_path(); // /boot/loader/entries/X.conf
        console.write_file(&format!("/mnt{}", entry_path), &boot_entry.to_entry_file())?;

        // Verify boot entry exists
        let verify = console.exec(
            &format!("cat /mnt{}", entry_path),
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: Boot entry MUST contain correct root UUID
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

        result.add_check(
            "Boot entry created",
            CheckResult::Pass(format!("{} with correct UUID", boot_entry.filename)),
        );

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

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Services to enable from levitate-spec
        // First check if service unit exists before trying to enable
        for service in ENABLED_SERVICES {
            // Check if service unit file exists
            let unit_check = console.exec_chroot(
                &format!("test -f /usr/lib/systemd/system/{}.service || test -f /lib/systemd/system/{}.service", service.name, service.name),
                Duration::from_secs(5),
            )?;

            if unit_check.exit_code != 0 {
                // Service not present in tarball = TARBALL IS BROKEN
                // If it's in ENABLED_SERVICES, it MUST be in the tarball. No exceptions.
                result.add_check(
                    &format!("{} enabled", service.name),
                    CheckResult::Fail {
                        expected: format!("{}.service exists in tarball", service.name),
                        actual: "Service unit file not found".to_string(),
                    },
                );
                continue;
            }

            let enable_result = console.exec_chroot(
                &service.enable_command(),
                Duration::from_secs(10),
            )?;

            if enable_result.success() {
                result.add_check(
                    &format!("{} enabled", service.name),
                    CheckResult::Pass(service.description.to_string()),
                );
            } else {
                // Service failed to enable = INSTALLATION IS BROKEN
                // If it's in ENABLED_SERVICES, it MUST enable successfully. No exceptions.
                result.add_check(
                    &format!("{} enabled", service.name),
                    CheckResult::Fail {
                        expected: "systemctl enable exit 0".to_string(),
                        actual: format!("exit {}: {}", enable_result.exit_code, enable_result.output.trim()),
                    },
                );
            }
        }

        // Enable serial console getty for testing
        // This is required for post-reboot verification via serial console
        let serial_result = console.exec_chroot(
            "systemctl enable serial-getty@ttyS0.service",
            Duration::from_secs(10),
        )?;

        if serial_result.success() {
            result.add_check(
                "serial-getty@ttyS0 enabled",
                CheckResult::Pass("Serial console login".to_string()),
            );
        } else {
            result.add_check(
                "serial-getty@ttyS0 enabled",
                CheckResult::Fail {
                    expected: "systemctl enable exit 0".to_string(),
                    actual: format!("exit {}: {}", serial_result.exit_code, serial_result.output),
                },
            );
        }

        // Exit chroot since we're done with installation
        if console.is_in_chroot() {
            match console.exit_chroot() {
                Ok(()) => {
                    result.add_check(
                        "Chroot exited",
                        CheckResult::Pass("Unmounted bind mounts".to_string()),
                    );
                }
                Err(e) => {
                    result.add_check(
                        "Chroot exited",
                        CheckResult::Fail {
                            expected: "Clean exit".to_string(),
                            actual: e.to_string(),
                        },
                    );
                }
            }
        }

        // Unmount partitions (EFI first, then root)
        let _ = console.exec("umount /mnt/boot", Duration::from_secs(5));
        let _ = console.exec("umount /mnt", Duration::from_secs(5));

        result.add_check(
            "Partitions unmounted",
            CheckResult::Pass("Ready for reboot".to_string()),
        );

        result.duration = start.elapsed();
        Ok(result)
    }
}
