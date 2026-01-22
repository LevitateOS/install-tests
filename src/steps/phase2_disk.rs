//! Phase 2: Disk setup steps.
//!
//! Steps 3-6: Identify, partition, format, and mount the target disk.
//!
//! # Cheat Prevention
//!
//! Each step documents its cheat vectors. Common cheats for disk steps:
//! - Accepting exit code 0 without verifying actual state
//! - Skipping partition/format verification
//! - Not waiting for kernel to create device nodes

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use cheat_guard::cheat_ensure;
use distro_spec::PartitionLayout;
use std::time::{Duration, Instant};

/// Step 3: Identify target disk
pub struct IdentifyDisk;

impl Step for IdentifyDisk {
    fn num(&self) -> usize { 3 }
    fn name(&self) -> &str { "Identify Target Disk" }
    fn ensures(&self) -> &str {
        "Target disk is detected and accessible for installation"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check for /dev/vda (virtio disk)
        // Use simpler command that outputs just the device names
        let lsblk = console.exec("lsblk -dn -o NAME,TYPE | grep disk", Duration::from_secs(5))?;

        // CHEAT GUARD: Target disk MUST be detected
        cheat_ensure!(
            lsblk.output.contains("vda"),
            protects = "Target disk is detected for installation",
            severity = "CRITICAL",
            cheats = [
                "Skip disk detection",
                "Hardcode disk path",
                "Accept any output"
            ],
            consequence = "No disk to install to, all subsequent steps fail",
            "Target disk /dev/vda not found. Got: {}", lsblk.output.trim()
        );

        result.add_check(
            "Target disk found",
            CheckResult::Pass("/dev/vda detected".to_string()),
        );

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 4: Partition the disk with GPT layout
///
/// # Cheat Vectors
/// - EASY: Accept sfdisk exit 0 without verifying partitions actually exist
/// - EASY: Skip waiting for kernel to create device nodes
/// - MEDIUM: Don't verify partition sizes/types match expected layout
///
/// # User Consequence if Cheated
/// Installation fails at format step ("device not found") or boot fails
/// because EFI partition is wrong size/type.
pub struct PartitionDisk;

impl Step for PartitionDisk {
    fn num(&self) -> usize { 4 }
    fn name(&self) -> &str { "Partition Disk (GPT)" }
    fn ensures(&self) -> &str {
        "Disk has GPT layout with EFI and root partitions"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use sfdisk for non-interactive partitioning
        // Layout from levitate-spec
        let layout = PartitionLayout::default();
        let partition_script = layout.to_sfdisk_script();

        // Write partition table
        let sfdisk_result = console.exec(
            &format!("echo '{}' | sfdisk /dev/vda", partition_script),
            Duration::from_secs(30),
        )?;

        // CHEAT GUARD: Don't just check exit code - verify actual state
        cheat_ensure!(
            sfdisk_result.success(),
            protects = "Disk partitioning actually works",
            severity = "CRITICAL",
            cheats = ["Ignore exit code", "Catch and suppress errors"],
            consequence = "No partitions created, format step fails, user stuck",
            "sfdisk failed with exit {}: {}", sfdisk_result.exit_code, sfdisk_result.output
        );

        result.add_check(
            "GPT partition table created",
            CheckResult::Pass("sfdisk completed successfully".to_string()),
        );

        // Wait for kernel to create partition device nodes
        // partprobe forces kernel to re-read partition table, udevadm settle waits for udev
        let _ = console.exec("partprobe /dev/vda 2>/dev/null || true", Duration::from_secs(5))?;
        let _ = console.exec("udevadm settle --timeout=5 2>/dev/null || sleep 2", Duration::from_secs(10))?;

        // CRITICAL: Verify partitions actually exist - don't trust sfdisk exit code alone
        let verify = console.exec("lsblk /dev/vda -o NAME,SIZE,TYPE", Duration::from_secs(5))?;

        // CHEAT GUARD: Must verify BOTH partitions exist
        cheat_ensure!(
            verify.output.contains("vda1") && verify.output.contains("vda2"),
            protects = "Both partitions were actually created",
            severity = "CRITICAL",
            cheats = [
                "Only check exit code",
                "Check for vda1 OR vda2 instead of AND",
                "Skip this verification entirely"
            ],
            consequence = "Missing partition causes format/mount failure, user cannot install",
            "Partitions not found. Expected vda1 AND vda2, got:\n{}", verify.output
        );

        result.add_check(
            "Partitions created",
            CheckResult::Pass("vda1 (EFI) and vda2 (root) exist".to_string()),
        );

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 5: Format partitions
pub struct FormatPartitions;

impl Step for FormatPartitions {
    fn num(&self) -> usize { 5 }
    fn name(&self) -> &str { "Format Partitions" }
    fn ensures(&self) -> &str {
        "Partitions have proper filesystems (FAT32 for EFI, ext4 for root)"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Format EFI partition as FAT32
        let fat_result = console.exec(
            "mkfs.fat -F32 /dev/vda1",
            Duration::from_secs(30),
        )?;

        // CHEAT GUARD: EFI partition MUST be formatted as FAT32
        cheat_ensure!(
            fat_result.success(),
            protects = "EFI partition has FAT32 filesystem for UEFI boot",
            severity = "CRITICAL",
            cheats = [
                "Skip format step",
                "Accept any exit code",
                "Format wrong partition"
            ],
            consequence = "EFI partition unreadable by UEFI firmware, system won't boot",
            "mkfs.fat failed (exit {}): {}", fat_result.exit_code, fat_result.output
        );

        result.add_check(
            "EFI partition formatted",
            CheckResult::Pass("FAT32 on /dev/vda1".to_string()),
        );

        // Format root partition as ext4
        let ext4_result = console.exec(
            "mkfs.ext4 -F /dev/vda2",
            Duration::from_secs(60),
        )?;

        // CHEAT GUARD: Root partition MUST be formatted as ext4
        cheat_ensure!(
            ext4_result.success(),
            protects = "Root partition has ext4 filesystem for system files",
            severity = "CRITICAL",
            cheats = [
                "Skip format step",
                "Accept any exit code",
                "Format wrong partition"
            ],
            consequence = "Root partition unreadable, system cannot mount rootfs, VFS panic",
            "mkfs.ext4 failed (exit {}): {}", ext4_result.exit_code, ext4_result.output
        );

        result.add_check(
            "Root partition formatted",
            CheckResult::Pass("ext4 on /dev/vda2".to_string()),
        );

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 6: Mount partitions
pub struct MountPartitions;

impl Step for MountPartitions {
    fn num(&self) -> usize { 6 }
    fn name(&self) -> &str { "Mount Partitions" }
    fn ensures(&self) -> &str {
        "Root partition at /mnt, EFI partition at /mnt/boot/efi"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Mount root partition
        console.exec("mkdir -p /mnt", Duration::from_secs(5))?;
        let mount_root = console.exec("mount /dev/vda2 /mnt", Duration::from_secs(10))?;

        // CHEAT GUARD: Root partition MUST be mounted for installation
        cheat_ensure!(
            mount_root.success(),
            protects = "Root partition is mounted for file extraction",
            severity = "CRITICAL",
            cheats = [
                "Skip mount",
                "Extract to live filesystem instead",
                "Accept mount failure"
            ],
            consequence = "Files extracted to wrong location, installed system empty",
            "Failed to mount /dev/vda2 to /mnt (exit {}): {}", mount_root.exit_code, mount_root.output
        );

        result.add_check(
            "Root mounted",
            CheckResult::Pass("/dev/vda2 -> /mnt".to_string()),
        );

        // Create and mount EFI partition
        // Note: /mnt/boot is part of ext4 root (supports Unix ownership)
        //       /mnt/boot/efi is FAT32 EFI partition (for bootloader only)
        console.exec("mkdir -p /mnt/boot/efi", Duration::from_secs(5))?;
        let mount_boot = console.exec("mount /dev/vda1 /mnt/boot/efi", Duration::from_secs(10))?;

        // CHEAT GUARD: EFI partition MUST be mounted for bootloader
        cheat_ensure!(
            mount_boot.success(),
            protects = "EFI partition is mounted for bootloader",
            severity = "CRITICAL",
            cheats = [
                "Skip EFI mount",
                "Install bootloader to wrong location",
                "Accept mount failure"
            ],
            consequence = "EFI bootloader can't be installed, system won't boot",
            "Failed to mount /dev/vda1 to /mnt/boot/efi (exit {}): {}", mount_boot.exit_code, mount_boot.output
        );

        result.add_check(
            "EFI mounted",
            CheckResult::Pass("/dev/vda1 -> /mnt/boot/efi".to_string()),
        );

        // Verify mounts
        let mounts = console.exec("mount | grep /mnt", Duration::from_secs(5))?;
        if mounts.output.contains("/mnt") && mounts.output.contains("/mnt/boot/efi") {
            result.add_check(
                "Mounts verified",
                CheckResult::Pass("Both partitions mounted".to_string()),
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
