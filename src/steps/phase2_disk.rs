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
use crate::distro::DistroContext;
use crate::executor::Executor;
use anyhow::Result;
use leviso_cheat_guard::cheat_ensure;
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

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Flush any pending output with a simple command
        // This ensures previous steps' async output is cleared
        let _ = executor.exec("true", Duration::from_secs(2))?;

        // Check for /dev/vda (virtio disk)
        // First, list all block devices for diagnostics
        let lsblk_all = executor.exec("lsblk -dn -o NAME,TYPE,SIZE", Duration::from_secs(5))?;

        // CHEAT GUARD: Target disk MUST be detected
        cheat_ensure!(
            lsblk_all.output.contains("vda"),
            protects = "Target disk is detected for installation",
            severity = "CRITICAL",
            cheats = [
                "Skip disk detection",
                "Hardcode disk path",
                "Accept any output"
            ],
            consequence = "No disk to install to, all subsequent steps fail",
            "Target disk /dev/vda not found. lsblk output: {}", lsblk_all.output.trim()
        );

        // Extract disk size from lsblk output for evidence
        let disk_info = lsblk_all.output.lines()
            .find(|l| l.contains("vda"))
            .unwrap_or("vda found");
        result.add_check("Target disk found", CheckResult::pass(format!("/dev/vda: {}", disk_info.trim())));

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

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use sfdisk for non-interactive partitioning
        // Layout from levitate-spec
        let layout = PartitionLayout::default();
        let partition_script = layout.to_sfdisk_script();

        // Write partition table
        let sfdisk_result = executor.exec(
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

        result.add_check("GPT partition table created", CheckResult::pass("sfdisk exit 0"));

        // Wait for kernel to create partition device nodes
        // NOTE: sfdisk already calls BLKRRPART internally, so we don't need blockdev --rereadpt
        // Calling it separately often fails with "device busy" because udev has the device open
        // udevadm settle waits for udev to process device events
        // Wait for udevd to be ready before settle (ping with retry)
        // udevd startup can take time on slow systems (TCG emulation without KVM)
        let mut udev_ready = false;
        for _ in 0..30 {  // 15 seconds total
            let ping = executor.exec("udevadm control --ping", Duration::from_secs(2))?;
            if ping.success() {
                udev_ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !udev_ready {
            anyhow::bail!("udevd not responding after 15 seconds of retries. Check systemd-udevd.service status.");
        }
        executor.exec_ok("udevadm settle --timeout=10", Duration::from_secs(15))?;

        // CRITICAL: Verify partitions actually exist - don't trust sfdisk exit code alone
        let verify = executor.exec("lsblk /dev/vda -o NAME,SIZE,TYPE", Duration::from_secs(5))?;

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

        // Extract partition info for evidence
        let part_lines: Vec<&str> = verify.output.lines()
            .filter(|l| l.contains("vda1") || l.contains("vda2"))
            .collect();
        result.add_check("Partitions created", CheckResult::pass(part_lines.join(", ")));

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

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Format EFI partition as FAT32
        let fat_result = executor.exec(
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

        result.add_check("EFI partition formatted", CheckResult::pass("mkfs.fat -F32 /dev/vda1 exit 0"));

        // Format root partition as ext4
        let ext4_result = executor.exec(
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

        result.add_check("Root partition formatted", CheckResult::pass("mkfs.ext4 /dev/vda2 exit 0"));

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 6: Mount partitions
///
/// IMPORTANT: ESP is mounted at /mnt/boot, NOT /mnt/boot/efi
///
/// Why? systemd-boot can ONLY read files from FAT-formatted partitions.
/// If we mount ESP at /mnt/boot/efi, then kernel/initramfs end up on ext4
/// at /mnt/boot, which systemd-boot cannot read.
///
/// By mounting ESP at /mnt/boot, the kernel and initramfs are stored on
/// the FAT32 ESP partition, where systemd-boot can find them.
///
/// This matches Arch Linux's standard layout and distro-spec's ESP_MOUNT_POINT.
pub struct MountPartitions;

impl Step for MountPartitions {
    fn num(&self) -> usize { 6 }
    fn name(&self) -> &str { "Mount Partitions" }
    fn ensures(&self) -> &str {
        "Root partition at /mnt, EFI partition at /mnt/boot"
    }

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Mount root partition
        executor.exec("mkdir -p /mnt", Duration::from_secs(5))?;
        let mount_root = executor.exec("mount /dev/vda2 /mnt", Duration::from_secs(10))?;

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

        result.add_check("Root mounted", CheckResult::pass("/dev/vda2 → /mnt"));

        // Create and mount EFI partition at /mnt/boot
        // NOTE: ESP is at /boot, NOT /boot/efi
        // systemd-boot can ONLY read from FAT partitions, so kernel must be on ESP
        executor.exec("mkdir -p /mnt/boot", Duration::from_secs(5))?;
        let mount_boot = executor.exec("mount /dev/vda1 /mnt/boot", Duration::from_secs(10))?;

        // CHEAT GUARD: EFI partition MUST be mounted for bootloader
        cheat_ensure!(
            mount_boot.success(),
            protects = "EFI partition is mounted at /boot for bootloader and kernel",
            severity = "CRITICAL",
            cheats = [
                "Skip EFI mount",
                "Mount at wrong location (/boot/efi)",
                "Accept mount failure"
            ],
            consequence = "Kernel not on FAT32, systemd-boot can't find it, system won't boot",
            "Failed to mount /dev/vda1 to /mnt/boot (exit {}): {}", mount_boot.exit_code, mount_boot.output
        );

        result.add_check("EFI mounted", CheckResult::pass("/dev/vda1 → /mnt/boot"));

        // Verify mounts - show actual mount output as evidence
        let mounts = executor.exec("mount | grep /mnt", Duration::from_secs(5))?;
        if mounts.output.contains("/mnt ") && mounts.output.contains("/mnt/boot ") {
            let mount_lines: Vec<&str> = mounts.output.lines().take(2).collect();
            result.add_check("Mounts verified", CheckResult::pass(mount_lines.join(" | ")));
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
