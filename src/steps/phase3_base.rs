//! Phase 3: Base system installation steps.
//!
//! Steps 7-10: Mount install media, extract tarball, generate fstab, setup chroot.
//!
//! # Cheat Prevention
//!
//! Critical steps that must actually work:
//! - Squashfs must be extracted with ALL files (not just some)
//! - fstab must have correct UUIDs (not placeholder values)
//! - Chroot must actually enter the new root (not stay in live environment)

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use cheat_guard::cheat_ensure;
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

        // CHEAT GUARD: ISO MUST be mounted - can't proceed without installation media
        cheat_ensure!(
            mount_check.output.contains("MOUNTED"),
            protects = "Installation media is accessible",
            severity = "CRITICAL",
            cheats = [
                "Return hardcoded path without checking",
                "Skip mount check entirely",
                "Accept any output as success"
            ],
            consequence = "No installation files available, user cannot install OS",
            "ISO not mounted at /media/cdrom. Init should mount this automatically."
        );

        result.add_check(
            "ISO mounted",
            CheckResult::Pass("/media/cdrom".to_string()),
        );

        // Verify squashfs is accessible
        let squashfs_check = console.exec(
            &format!("ls -la {}", SQUASHFS_CDROM_PATH),
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: Squashfs MUST exist and be accessible
        cheat_ensure!(
            squashfs_check.success() && squashfs_check.output.contains(SQUASHFS_NAME),
            protects = "Squashfs image contains the base system",
            severity = "CRITICAL",
            cheats = [
                "Skip file existence check",
                "Accept any file as squashfs",
                "Hardcode path without verification"
            ],
            consequence = "No base system to install, extraction fails, user stuck",
            "Squashfs not found at {}. ISO must contain live/{}", SQUASHFS_CDROM_PATH, SQUASHFS_NAME
        );

        result.add_check(
            "Squashfs accessible",
            CheckResult::Pass(SQUASHFS_CDROM_PATH.to_string()),
        );

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

        // CHEAT GUARD: Squashfs MUST exist before extraction
        cheat_ensure!(
            check.exit_code == 0,
            protects = "Base system image is available for extraction",
            severity = "CRITICAL",
            cheats = [
                "Skip file check",
                "Proceed with extraction anyway",
                "Use wrong path"
            ],
            consequence = "Extraction fails, no files installed, user cannot boot",
            "Squashfs not found at {}. Ensure ISO is mounted at /media/cdrom", SQUASHFS_CDROM_PATH
        );

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

        // CHEAT GUARD: Extraction MUST succeed with files created
        cheat_ensure!(
            extraction_ok,
            protects = "Base system files are actually extracted to disk",
            severity = "CRITICAL",
            cheats = [
                "Accept any exit code as success",
                "Skip checking if files were created",
                "Ignore unsquashfs output"
            ],
            consequence = "Empty /mnt, no system installed, boot fails",
            "unsquashfs failed (exit {}): {}", extract.exit_code, extract.output
        );

        result.add_check(
            "Squashfs extracted",
            CheckResult::Pass("Extracted to /mnt".to_string()),
        );

        // Verify essential directories exist
        let verify = console.exec(
            "ls /mnt/bin /mnt/usr /mnt/etc 2>/dev/null && echo VERIFY_OK",
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: Essential directories MUST exist after extraction
        cheat_ensure!(
            verify.output.contains("VERIFY_OK"),
            protects = "Essential FHS directories exist for bootable system",
            severity = "CRITICAL",
            cheats = [
                "Only check one directory",
                "Accept partial extraction",
                "Skip verification entirely"
            ],
            consequence = "Incomplete system, missing binaries, boot fails or crashes",
            "Essential directories missing after extraction. /bin, /usr, /etc must exist."
        );

        result.add_check(
            "Base system verified",
            CheckResult::Pass("/bin, /usr, /etc exist".to_string()),
        );

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

        // CHEAT GUARD: UUIDs MUST be valid - fstab with wrong UUIDs = unbootable
        cheat_ensure!(
            !root_uuid.is_empty() && !boot_uuid.is_empty(),
            protects = "fstab uses real partition UUIDs",
            severity = "CRITICAL",
            cheats = [
                "Use hardcoded/placeholder UUIDs",
                "Skip UUID extraction",
                "Accept empty UUIDs"
            ],
            consequence = "fstab has wrong UUIDs, system won't mount partitions, boot fails",
            "Failed to get UUIDs: root='{}', boot='{}'", root_uuid, boot_uuid
        );

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

        // CHEAT GUARD: fstab MUST contain the correct UUIDs we just extracted
        cheat_ensure!(
            verify.output.contains(root_uuid) && verify.output.contains(boot_uuid),
            protects = "fstab contains correct UUIDs for automatic mounting",
            severity = "CRITICAL",
            cheats = [
                "Only check if fstab exists",
                "Check for one UUID but not both",
                "Skip fstab content verification"
            ],
            consequence = "Wrong UUIDs in fstab, partitions won't mount at boot",
            "fstab doesn't contain expected UUIDs:\nExpected root={}, boot={}\nGot:\n{}",
            root_uuid, boot_uuid, verify.output
        );

        result.add_check(
            "fstab written",
            CheckResult::Pass("Contains correct UUIDs".to_string()),
        );

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
        let chroot_result = console.enter_chroot("/mnt");

        // CHEAT GUARD: Chroot MUST succeed for configuration steps
        cheat_ensure!(
            chroot_result.is_ok(),
            protects = "Configuration happens in the installed system, not live environment",
            severity = "CRITICAL",
            cheats = [
                "Skip chroot and configure live environment",
                "Ignore chroot errors",
                "Pretend chroot succeeded"
            ],
            consequence = "Configuration goes to live system, installed system unconfigured, boot fails",
            "Failed to enter chroot: {}", chroot_result.as_ref().err().map(|e| e.to_string()).unwrap_or_default()
        );

        result.add_check(
            "Chroot entered",
            CheckResult::Pass("Bind mounts created".to_string()),
        );

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
