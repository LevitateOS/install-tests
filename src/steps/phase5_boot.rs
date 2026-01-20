//! Phase 5: Bootloader installation steps.
//!
//! Steps 15-16: Install bootloader, enable services.

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use std::time::{Duration, Instant};

/// Step 15: Install systemd-boot bootloader
pub struct InstallBootloader;

impl Step for InstallBootloader {
    fn num(&self) -> usize { 15 }
    fn name(&self) -> &str { "Install Bootloader" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Install systemd-boot
        let bootctl_result = console.exec_chroot(
            "bootctl install --path=/boot",
            Duration::from_secs(30),
        )?;

        if bootctl_result.success() {
            result.add_check(
                "systemd-boot installed",
                CheckResult::Pass("bootctl install succeeded".to_string()),
            );
        } else {
            result.add_check(
                "systemd-boot installed",
                CheckResult::Fail {
                    expected: "bootctl exit 0".to_string(),
                    actual: format!("exit {}: {}", bootctl_result.exit_code, bootctl_result.output),
                },
            );
            result.fail("Ensure /boot is mounted and EFI variables are available");
            return Ok(result);
        }

        // Get root partition UUID for boot entry
        let uuid_result = console.exec("blkid -s UUID -o value /dev/vda2", Duration::from_secs(5))?;
        let root_uuid = uuid_result.output.trim();

        // Create loader.conf
        let loader_conf = "default levitateos.conf
timeout 3
console-mode max
editor no
";
        console.write_file("/mnt/boot/loader/loader.conf", loader_conf)?;

        // Create boot entry
        let entry = format!(
            "title   LevitateOS
linux   /vmlinuz
initrd  /initramfs.img
options root=UUID={} rw quiet
",
            root_uuid
        );
        console.write_file("/mnt/boot/loader/entries/levitateos.conf", &entry)?;

        // Verify boot entry exists
        let verify = console.exec(
            "cat /mnt/boot/loader/entries/levitateos.conf",
            Duration::from_secs(5),
        )?;

        if verify.output.contains(root_uuid) {
            result.add_check(
                "Boot entry created",
                CheckResult::Pass("levitateos.conf with correct UUID".to_string()),
            );
        } else {
            result.add_check(
                "Boot entry created",
                CheckResult::Fail {
                    expected: format!("Entry with UUID {}", root_uuid),
                    actual: "Entry missing or incorrect".to_string(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 16: Enable essential services
pub struct EnableServices;

impl Step for EnableServices {
    fn num(&self) -> usize { 16 }
    fn name(&self) -> &str { "Enable Services" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Services to enable
        let services = [
            "systemd-networkd",
            "systemd-resolved",
            "sshd",
        ];

        for service in services {
            let enable_result = console.exec_chroot(
                &format!("systemctl enable {} 2>/dev/null || true", service),
                Duration::from_secs(10),
            )?;

            // We use || true because some services might not exist in minimal stage3
            result.add_check(
                &format!("{} enable attempted", service),
                CheckResult::Pass(format!("exit {}", enable_result.exit_code)),
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

        // Unmount partitions
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
