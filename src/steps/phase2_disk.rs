//! Phase 2: Disk setup steps.
//!
//! Steps 3-6: Identify, partition, format, and mount the target disk.

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use std::time::{Duration, Instant};

/// Step 3: Identify target disk
pub struct IdentifyDisk;

impl Step for IdentifyDisk {
    fn num(&self) -> usize { 3 }
    fn name(&self) -> &str { "Identify Target Disk" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check for /dev/vda (virtio disk)
        let lsblk = console.exec("lsblk -d -o NAME,SIZE,TYPE | grep disk", Duration::from_secs(5))?;

        if lsblk.output.contains("vda") {
            result.add_check(
                "Target disk found",
                CheckResult::Pass("/dev/vda detected".to_string()),
            );
        } else {
            result.add_check(
                "Target disk found",
                CheckResult::Fail {
                    expected: "/dev/vda (virtio disk)".to_string(),
                    actual: format!("Found: {}", lsblk.output.trim()),
                },
            );
            result.fail("Ensure QEMU is started with a virtio disk (-drive if=virtio)");
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 4: Partition the disk with GPT layout
pub struct PartitionDisk;

impl Step for PartitionDisk {
    fn num(&self) -> usize { 4 }
    fn name(&self) -> &str { "Partition Disk (GPT)" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use sfdisk for non-interactive partitioning
        // Layout: 512M EFI (type EF00), rest for root (type 8300)
        let partition_script = r#"label: gpt
,512M,U,*
,,L
"#;

        // Write partition table
        let sfdisk_result = console.exec(
            &format!("echo '{}' | sfdisk /dev/vda", partition_script),
            Duration::from_secs(30),
        )?;

        if sfdisk_result.success() {
            result.add_check(
                "GPT partition table created",
                CheckResult::Pass("sfdisk completed successfully".to_string()),
            );
        } else {
            result.add_check(
                "GPT partition table created",
                CheckResult::Fail {
                    expected: "sfdisk exit 0".to_string(),
                    actual: format!("exit {}: {}", sfdisk_result.exit_code, sfdisk_result.output),
                },
            );
            result.fail("Check disk state with 'lsblk' and 'sfdisk -d /dev/vda'");
            return Ok(result);
        }

        // Verify partitions exist
        let verify = console.exec("lsblk /dev/vda -o NAME,SIZE,TYPE", Duration::from_secs(5))?;

        if verify.output.contains("vda1") && verify.output.contains("vda2") {
            result.add_check(
                "Partitions created",
                CheckResult::Pass("vda1 (EFI) and vda2 (root) exist".to_string()),
            );
        } else {
            result.add_check(
                "Partitions created",
                CheckResult::Fail {
                    expected: "vda1 and vda2".to_string(),
                    actual: verify.output.clone(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 5: Format partitions
pub struct FormatPartitions;

impl Step for FormatPartitions {
    fn num(&self) -> usize { 5 }
    fn name(&self) -> &str { "Format Partitions" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Format EFI partition as FAT32
        let fat_result = console.exec(
            "mkfs.fat -F32 /dev/vda1",
            Duration::from_secs(30),
        )?;

        if fat_result.success() {
            result.add_check(
                "EFI partition formatted",
                CheckResult::Pass("FAT32 on /dev/vda1".to_string()),
            );
        } else {
            result.add_check(
                "EFI partition formatted",
                CheckResult::Fail {
                    expected: "mkfs.fat exit 0".to_string(),
                    actual: format!("exit {}", fat_result.exit_code),
                },
            );
        }

        // Format root partition as ext4
        let ext4_result = console.exec(
            "mkfs.ext4 -F /dev/vda2",
            Duration::from_secs(60),
        )?;

        if ext4_result.success() {
            result.add_check(
                "Root partition formatted",
                CheckResult::Pass("ext4 on /dev/vda2".to_string()),
            );
        } else {
            result.add_check(
                "Root partition formatted",
                CheckResult::Fail {
                    expected: "mkfs.ext4 exit 0".to_string(),
                    actual: format!("exit {}", ext4_result.exit_code),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 6: Mount partitions
pub struct MountPartitions;

impl Step for MountPartitions {
    fn num(&self) -> usize { 6 }
    fn name(&self) -> &str { "Mount Partitions" }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Mount root partition
        console.exec("mkdir -p /mnt", Duration::from_secs(5))?;
        let mount_root = console.exec("mount /dev/vda2 /mnt", Duration::from_secs(10))?;

        if mount_root.success() {
            result.add_check(
                "Root mounted",
                CheckResult::Pass("/dev/vda2 -> /mnt".to_string()),
            );
        } else {
            result.add_check(
                "Root mounted",
                CheckResult::Fail {
                    expected: "mount exit 0".to_string(),
                    actual: format!("exit {}: {}", mount_root.exit_code, mount_root.output),
                },
            );
            return Ok(result);
        }

        // Create and mount boot partition
        console.exec("mkdir -p /mnt/boot", Duration::from_secs(5))?;
        let mount_boot = console.exec("mount /dev/vda1 /mnt/boot", Duration::from_secs(10))?;

        if mount_boot.success() {
            result.add_check(
                "Boot mounted",
                CheckResult::Pass("/dev/vda1 -> /mnt/boot".to_string()),
            );
        } else {
            result.add_check(
                "Boot mounted",
                CheckResult::Fail {
                    expected: "mount exit 0".to_string(),
                    actual: format!("exit {}", mount_boot.exit_code),
                },
            );
        }

        // Verify mounts
        let mounts = console.exec("mount | grep /mnt", Duration::from_secs(5))?;
        if mounts.output.contains("/mnt") && mounts.output.contains("/mnt/boot") {
            result.add_check(
                "Mounts verified",
                CheckResult::Pass("Both partitions mounted".to_string()),
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
