//! Phase 3: Base system installation steps.
//!
//! Steps 7-10: Mount install media, extract tarball, generate fstab, setup chroot.

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use distro_spec::levitate::{SQUASHFS_NAME, SQUASHFS_CDROM_PATH};
use distro_spec::shared::partitions::{EFI_FILESYSTEM, ROOT_FILESYSTEM};
use std::time::{Duration, Instant};

/// Step 7: Mount installation media (CDROM)
pub struct MountInstallMedia;

impl Step for MountInstallMedia {
    fn num(&self) -> usize { 7 }
    fn name(&self) -> &str { "Mount Installation Media" }
    fn ensures(&self) -> &str {
        "Installation media (ISO) is mounted and squashfs is accessible"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // The init script mounts the ISO at /media/cdrom
        // Verify it's mounted by checking if the directory has content
        let mount_check = console.exec("test -d /media/cdrom/live && echo MOUNTED", Duration::from_secs(5))?;

        if mount_check.output.contains("MOUNTED") {
            result.add_check(
                "ISO mounted",
                CheckResult::Pass("/media/cdrom".to_string()),
            );
        } else {
            result.add_check(
                "ISO mounted",
                CheckResult::Fail {
                    expected: "ISO mounted at /media/cdrom".to_string(),
                    actual: "ISO not mounted by initramfs".to_string(),
                },
            );
            result.fail("Init should mount ISO at /media/cdrom");
            return Ok(result);
        }

        // Verify squashfs is accessible
        let squashfs_check = console.exec(
            &format!("ls -la {}", SQUASHFS_CDROM_PATH),
            Duration::from_secs(5),
        )?;

        if squashfs_check.success() && squashfs_check.output.contains(SQUASHFS_NAME) {
            result.add_check(
                "Squashfs accessible",
                CheckResult::Pass(SQUASHFS_CDROM_PATH.to_string()),
            );
        } else {
            result.add_check(
                "Squashfs accessible",
                CheckResult::Fail {
                    expected: format!("{} on CDROM", SQUASHFS_NAME),
                    actual: "Squashfs not found on CDROM".to_string(),
                },
            );
            result.fail("Ensure ISO contains live/filesystem.squashfs");
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 8: Extract squashfs image
pub struct ExtractSquashfs;

impl Step for ExtractSquashfs {
    fn num(&self) -> usize { 8 }
    fn name(&self) -> &str { "Extract Squashfs Image" }
    fn ensures(&self) -> &str {
        "Base system is extracted with all essential directories present"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check squashfs exists
        let check = console.exec(
            &format!("test -f {}", SQUASHFS_CDROM_PATH),
            Duration::from_secs(5),
        )?;

        if check.exit_code != 0 {
            result.add_check(
                "Squashfs found",
                CheckResult::Fail {
                    expected: SQUASHFS_CDROM_PATH.to_string(),
                    actual: "Squashfs not found".to_string(),
                },
            );
            result.fail("Ensure ISO is mounted at /mnt/cdrom with live/filesystem.squashfs");
            return Ok(result);
        }

        result.add_check(
            "Squashfs found",
            CheckResult::Pass(SQUASHFS_CDROM_PATH.to_string()),
        );

        // Extract squashfs using unsquashfs (much faster than tar)
        // -f = force overwrite, -d = destination
        // Use full path since non-interactive bash doesn't source /etc/profile
        let extract = console.exec(
            &format!("/usr/sbin/unsquashfs -f -d /mnt {}", SQUASHFS_CDROM_PATH),
            Duration::from_secs(300), // 5 minutes for extraction
        )?;

        // unsquashfs returns exit 2 for permission errors (can't chown files)
        // but extraction still succeeds. Check for "created" in output.
        let extraction_ok = extract.output.contains("created") &&
                           (extract.exit_code == 0 || extract.exit_code == 2);

        if extraction_ok {
            result.add_check(
                "Squashfs extracted",
                CheckResult::Pass("Extracted to /mnt".to_string()),
            );
        } else {
            result.add_check(
                "Squashfs extracted",
                CheckResult::Fail {
                    expected: "unsquashfs exit 0 or 2 with files created".to_string(),
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
    fn ensures(&self) -> &str {
        "System has valid /etc/fstab with correct UUIDs for automatic mounting"
    }

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
    fn ensures(&self) -> &str {
        "Chroot environment is configured with necessary bind mounts"
    }

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
