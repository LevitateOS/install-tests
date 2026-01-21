//! Phase 3: Base system installation steps.
//!
//! Steps 7-10: Mount install media, extract tarball, generate fstab, setup chroot.

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use distro_spec::levitate::{TARBALL_NAME, paths::TARBALL_SEARCH_PATHS};
use distro_spec::shared::partitions::{EFI_FILESYSTEM, ROOT_FILESYSTEM};
use std::time::{Duration, Instant};

/// Step 7: Mount installation media (CDROM)
pub struct MountInstallMedia;

impl Step for MountInstallMedia {
    fn num(&self) -> usize { 7 }
    fn name(&self) -> &str { "Mount Installation Media" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check for /dev/sr0 (CDROM device via virtio-scsi)
        let lsblk = console.exec("lsblk -d -o NAME,TYPE | grep rom", Duration::from_secs(5))?;

        if lsblk.output.contains("sr0") {
            result.add_check(
                "CDROM device found",
                CheckResult::Pass("/dev/sr0 detected".to_string()),
            );
        } else {
            result.add_check(
                "CDROM device found",
                CheckResult::Fail {
                    expected: "/dev/sr0 (virtio-scsi CDROM)".to_string(),
                    actual: format!("Found: {}", lsblk.output.trim()),
                },
            );
            result.fail("Ensure ISO is attached as virtio-scsi CDROM");
            return Ok(result);
        }

        // Create mount point and mount CDROM
        console.exec("mkdir -p /mnt/cdrom", Duration::from_secs(5))?;
        let mount_result = console.exec("mount /dev/sr0 /mnt/cdrom", Duration::from_secs(10))?;

        if mount_result.success() {
            result.add_check(
                "CDROM mounted",
                CheckResult::Pass("/dev/sr0 -> /mnt/cdrom".to_string()),
            );
        } else {
            result.add_check(
                "CDROM mounted",
                CheckResult::Fail {
                    expected: "mount exit 0".to_string(),
                    actual: format!("exit {}: {}", mount_result.exit_code, mount_result.output),
                },
            );
            result.fail("Check kernel modules: virtio_scsi, cdrom, sr_mod, isofs");
            return Ok(result);
        }

        // Verify tarball is accessible
        let tarball_check = console.exec(
            &format!("ls -la /mnt/cdrom/{}", TARBALL_NAME),
            Duration::from_secs(5),
        )?;

        if tarball_check.success() && tarball_check.output.contains(TARBALL_NAME) {
            result.add_check(
                "Tarball accessible",
                CheckResult::Pass(format!("/mnt/cdrom/{}", TARBALL_NAME)),
            );
        } else {
            result.add_check(
                "Tarball accessible",
                CheckResult::Fail {
                    expected: format!("{} on CDROM", TARBALL_NAME),
                    actual: "Tarball not found on CDROM".to_string(),
                },
            );
            result.fail("Ensure ISO contains the base tarball");
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 8: Extract stage3 tarball
pub struct ExtractTarball;

impl Step for ExtractTarball {
    fn num(&self) -> usize { 8 }
    fn name(&self) -> &str { "Extract Stage3 Tarball" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Find tarball by checking each path individually
        // Use exit code (test -f returns 0 if file exists)
        let mut tarball_path: Option<&str> = None;
        for path in TARBALL_SEARCH_PATHS {
            let check = console.exec(
                &format!("test -f {}", path),
                Duration::from_secs(5),
            )?;
            if check.exit_code == 0 {
                tarball_path = Some(path);
                break;
            }
        }

        let tarball_path = match tarball_path {
            Some(path) => path,
            None => {
                result.add_check(
                    "Tarball found",
                    CheckResult::Fail {
                        expected: format!("{} in search paths", TARBALL_NAME),
                        actual: "Tarball not found".to_string(),
                    },
                );
                result.fail(&format!(
                    "Ensure {} is included in the ISO or copied to the live system. Searched: {:?}",
                    TARBALL_NAME, TARBALL_SEARCH_PATHS
                ));
                return Ok(result);
            }
        };

        result.add_check(
            "Tarball found",
            CheckResult::Pass(format!("Found at {}", tarball_path)),
        );

        // Extract tarball (--no-same-owner to avoid ownership errors in live environment)
        let extract = console.exec(
            &format!("tar xpf {} --no-same-owner -C /mnt", tarball_path),
            Duration::from_secs(300), // 5 minutes for extraction
        )?;

        if extract.success() {
            result.add_check(
                "Tarball extracted",
                CheckResult::Pass("Extracted to /mnt".to_string()),
            );
        } else {
            result.add_check(
                "Tarball extracted",
                CheckResult::Fail {
                    expected: "tar exit 0".to_string(),
                    actual: format!("exit {}: {}", extract.exit_code, extract.output),
                },
            );
            return Ok(result);
        }

        // Verify essential directories exist
        let verify = console.exec(
            "ls /mnt/bin /mnt/usr /mnt/etc 2>/dev/null && echo VERIFY_OK",
            Duration::from_secs(5),
        )?;

        if verify.output.contains("VERIFY_OK") {
            result.add_check(
                "Base system verified",
                CheckResult::Pass("/bin, /usr, /etc exist".to_string()),
            );
        } else {
            result.add_check(
                "Base system verified",
                CheckResult::Fail {
                    expected: "Essential directories".to_string(),
                    actual: "Missing directories".to_string(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 8: Generate /etc/fstab
pub struct GenerateFstab;

impl Step for GenerateFstab {
    fn num(&self) -> usize { 9 }
    fn name(&self) -> &str { "Generate fstab" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Get UUIDs for partitions
        // Note: output may contain command echo, so we extract just the UUID line
        let uuid_root = console.exec(
            "blkid -s UUID -o value /dev/vda2",
            Duration::from_secs(5),
        )?;
        let uuid_boot = console.exec(
            "blkid -s UUID -o value /dev/vda1",
            Duration::from_secs(5),
        )?;

        // Extract UUID from output (it's the line that looks like a UUID)
        let root_uuid = uuid_root.output
            .lines()
            .find(|line| {
                let trimmed = line.trim();
                // UUID looks like: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
                trimmed.len() == 36 && trimmed.chars().filter(|c| *c == '-').count() == 4
            })
            .map(|s| s.trim())
            .unwrap_or("");

        let boot_uuid = uuid_boot.output
            .lines()
            .find(|line| {
                let trimmed = line.trim();
                // FAT UUID looks like: XXXX-XXXX (8 chars with dash)
                (trimmed.len() == 9 && trimmed.chars().nth(4) == Some('-')) ||
                // Or standard UUID
                (trimmed.len() == 36 && trimmed.chars().filter(|c| *c == '-').count() == 4)
            })
            .map(|s| s.trim())
            .unwrap_or("");

        if root_uuid.is_empty() || boot_uuid.is_empty() {
            result.add_check(
                "UUIDs retrieved",
                CheckResult::Fail {
                    expected: "UUIDs for both partitions".to_string(),
                    actual: format!("root={}, boot={}", root_uuid, boot_uuid),
                },
            );
            result.fail("Run blkid to check partition UUIDs");
            return Ok(result);
        }

        result.add_check(
            "UUIDs retrieved",
            CheckResult::Pass(format!("root={}, boot={}", root_uuid, boot_uuid)),
        );

        // Generate fstab content using filesystem types from levitate-spec
        let fstab = format!(
            "# /etc/fstab - generated by install-tests
# <file system>  <mount point>  <type>  <options>  <dump>  <pass>
UUID={}  /      {}   defaults  0  1
UUID={}  /boot  {}   defaults  0  2
",
            root_uuid, ROOT_FILESYSTEM, boot_uuid, EFI_FILESYSTEM
        );

        // Write fstab
        console.write_file("/mnt/etc/fstab", &fstab)?;

        // Verify fstab was written
        let verify = console.exec("cat /mnt/etc/fstab", Duration::from_secs(5))?;

        if verify.output.contains(root_uuid) && verify.output.contains(boot_uuid) {
            result.add_check(
                "fstab written",
                CheckResult::Pass("Contains correct UUIDs".to_string()),
            );
        } else {
            result.add_check(
                "fstab written",
                CheckResult::Fail {
                    expected: "UUIDs in fstab".to_string(),
                    actual: verify.output.clone(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 9: Setup chroot environment
pub struct SetupChroot;

impl Step for SetupChroot {
    fn num(&self) -> usize { 10 }
    fn name(&self) -> &str { "Setup Chroot" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Enter chroot (this sets up bind mounts)
        match console.enter_chroot("/mnt") {
            Ok(()) => {
                result.add_check(
                    "Chroot entered",
                    CheckResult::Pass("Bind mounts created".to_string()),
                );
            }
            Err(e) => {
                result.add_check(
                    "Chroot entered",
                    CheckResult::Fail {
                        expected: "Successful chroot setup".to_string(),
                        actual: e.to_string(),
                    },
                );
                result.fail("Check mount points and /mnt contents");
                return Ok(result);
            }
        }

        // Verify we can run commands in chroot
        let verify = console.exec_chroot("echo CHROOT_OK", Duration::from_secs(5))?;

        if verify.output.contains("CHROOT_OK") {
            result.add_check(
                "Chroot functional",
                CheckResult::Pass("Commands execute in chroot".to_string()),
            );
        } else {
            result.add_check(
                "Chroot functional",
                CheckResult::Fail {
                    expected: "CHROOT_OK".to_string(),
                    actual: verify.output.clone(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
