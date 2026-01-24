//! Phase 3: Base system installation steps.
//!
//! Steps 7-10: Mount install media, run recstrap, generate fstab, verify chroot.
//!
//! Uses the LevitateOS installation tools:
//! - recstrap (like pacstrap) - extracts squashfs to target
//! - recfstab (like genfstab) - generates fstab from mounts
//! - recchroot (like arch-chroot) - runs commands in chroot
//!
//! # Cheat Prevention
//!
//! Critical steps that must actually work:
//! - recstrap must extract ALL files (not just some)
//! - recfstab must generate valid fstab with correct UUIDs
//! - recchroot must actually enter the new root

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use leviso_cheat_guard::cheat_ensure;
use distro_spec::levitate::{SQUASHFS_NAME, SQUASHFS_CDROM_PATH};
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

        result.add_check("ISO mounted", CheckResult::pass("/media/cdrom/live exists"));

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

        // Show squashfs size as evidence
        let size_info = squashfs_check.output.lines().next().unwrap_or("found");
        result.add_check("Squashfs accessible", CheckResult::pass(size_info.trim()));

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 8: Extract base system using recstrap
///
/// recstrap is like pacstrap for Arch - extracts the squashfs to target.
/// User does partitioning/formatting/mounting manually before this step.
pub struct ExtractSquashfs;

impl Step for ExtractSquashfs {
    fn num(&self) -> usize { 8 }
    fn name(&self) -> &str { "Extract Base System (recstrap)" }
    fn ensures(&self) -> &str {
        "Base system is extracted with all essential directories present"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check recstrap is available
        let recstrap_check = console.exec(
            "which recstrap",
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: recstrap MUST be available
        cheat_ensure!(
            recstrap_check.success(),
            protects = "recstrap installer is available in live ISO",
            severity = "CRITICAL",
            cheats = [
                "Skip recstrap check",
                "Use unsquashfs directly",
                "Hardcode extraction command"
            ],
            consequence = "No installer available, cannot extract system",
            "recstrap not found. ISO may be incomplete."
        );

        result.add_check("recstrap available", CheckResult::pass(recstrap_check.output.trim()));

        // Run recstrap to extract base system
        // recstrap handles squashfs location automatically (/media/cdrom/live/filesystem.squashfs)
        // Use --force because the freshly formatted ext4 contains lost+found
        let extract = console.exec(
            "recstrap --force /mnt",
            Duration::from_secs(300), // 5 minutes for extraction
        )?;

        // CHEAT GUARD: recstrap MUST succeed
        cheat_ensure!(
            extract.success(),
            protects = "Base system files are actually extracted to disk",
            severity = "CRITICAL",
            cheats = [
                "Accept any exit code as success",
                "Skip checking recstrap output",
                "Ignore extraction errors"
            ],
            consequence = "Empty /mnt, no system installed, boot fails",
            "recstrap failed (exit {}): {}", extract.exit_code, extract.output
        );

        result.add_check("recstrap completed", CheckResult::pass("exit 0"));

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
            "Essential directories missing after recstrap. /bin, /usr, /etc must exist."
        );

        result.add_check("Base system verified", CheckResult::pass("/mnt/{bin,usr,etc} exist"));

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 9: Generate /etc/fstab using recfstab
///
/// recfstab is like genfstab for Arch - reads mounted filesystems and generates fstab.
pub struct GenerateFstab;

impl Step for GenerateFstab {
    fn num(&self) -> usize { 9 }
    fn name(&self) -> &str { "Generate fstab (recfstab)" }
    fn ensures(&self) -> &str {
        "System has valid /etc/fstab with correct UUIDs for automatic mounting"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check recfstab is available
        let recfstab_check = console.exec(
            "which recfstab",
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: recfstab MUST be available
        cheat_ensure!(
            recfstab_check.success(),
            protects = "recfstab is available in live ISO",
            severity = "CRITICAL",
            cheats = [
                "Skip recfstab check",
                "Manually write fstab",
                "Hardcode UUIDs"
            ],
            consequence = "Cannot generate fstab automatically, user must do it manually",
            "recfstab not found. ISO may be incomplete."
        );

        result.add_check("recfstab available", CheckResult::pass(recfstab_check.output.trim()));

        // Generate fstab using recfstab
        // recfstab reads mounted filesystems under /mnt and outputs fstab entries
        let fstab_result = console.exec(
            "recfstab /mnt >> /mnt/etc/fstab",
            Duration::from_secs(10),
        )?;

        // CHEAT GUARD: recfstab MUST succeed
        cheat_ensure!(
            fstab_result.success(),
            protects = "fstab is generated with correct UUIDs",
            severity = "CRITICAL",
            cheats = [
                "Accept any exit code",
                "Skip fstab generation",
                "Use placeholder UUIDs"
            ],
            consequence = "No fstab or wrong UUIDs, system won't mount partitions at boot",
            "recfstab failed (exit {}): {}", fstab_result.exit_code, fstab_result.output
        );

        result.add_check("recfstab completed", CheckResult::pass("exit 0"));

        // Verify fstab contains UUIDs
        let verify = console.exec("cat /mnt/etc/fstab", Duration::from_secs(5))?;

        // CHEAT GUARD: fstab MUST contain UUID entries
        cheat_ensure!(
            verify.output.contains("UUID="),
            protects = "fstab uses UUIDs for reliable mounting",
            severity = "CRITICAL",
            cheats = [
                "Only check if fstab exists",
                "Accept empty fstab",
                "Skip content verification"
            ],
            consequence = "fstab without UUIDs may fail to mount after device changes",
            "fstab doesn't contain UUID entries:\n{}", verify.output
        );

        // Show actual UUIDs found
        let uuid_line = verify.output.lines()
            .find(|l| l.contains("UUID="))
            .unwrap_or("UUID= found");
        result.add_check("fstab contains UUIDs", CheckResult::pass(uuid_line.trim()));

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 10: Verify chroot environment works
///
/// recchroot is like arch-chroot - handles bind mounts automatically.
/// This step verifies recchroot is available and functional.
pub struct VerifyChroot;

impl Step for VerifyChroot {
    fn num(&self) -> usize { 10 }
    fn name(&self) -> &str { "Verify Chroot (recchroot)" }
    fn ensures(&self) -> &str {
        "recchroot can execute commands in the installed system"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check recchroot is available
        let recchroot_check = console.exec(
            "which recchroot",
            Duration::from_secs(5),
        )?;

        // CHEAT GUARD: recchroot MUST be available
        cheat_ensure!(
            recchroot_check.success(),
            protects = "recchroot is available for system configuration",
            severity = "CRITICAL",
            cheats = [
                "Skip recchroot check",
                "Use plain chroot",
                "Configure outside chroot"
            ],
            consequence = "Cannot properly configure installed system",
            "recchroot not found. ISO may be incomplete."
        );

        result.add_check("recchroot available", CheckResult::pass(recchroot_check.output.trim()));

        // Verify recchroot can execute commands
        let verify = console.exec_chroot("/mnt", "echo CHROOT_OK", Duration::from_secs(10))?;

        // CHEAT GUARD: recchroot MUST actually work
        cheat_ensure!(
            verify.output.contains("CHROOT_OK"),
            protects = "Commands execute inside the installed system",
            severity = "CRITICAL",
            cheats = [
                "Skip chroot verification",
                "Accept any output",
                "Pretend chroot works"
            ],
            consequence = "Configuration commands won't run in installed system",
            "recchroot test failed: {}", verify.output
        );

        result.add_check("recchroot functional", CheckResult::pass("echo CHROOT_OK returned CHROOT_OK"));

        result.duration = start.elapsed();
        Ok(result)
    }
}
