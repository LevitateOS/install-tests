//! RalphOS distro context.
//!
//! RalphOS currently follows the same systemd/systemd-boot test contract shape
//! as LevitateOS for install-test staging.

use super::DistroContext;
use std::path::PathBuf;

/// RalphOS context for systemd-based testing.
pub struct RalphContext;

impl DistroContext for RalphContext {
    fn name(&self) -> &str {
        "RalphOS"
    }

    fn id(&self) -> &str {
        "ralph"
    }

    fn live_boot_success_patterns(&self) -> &[&str] {
        &["___SHELL_READY___", "___PROMPT___"]
    }

    fn installed_boot_success_patterns(&self) -> &[&str] {
        &["___SHELL_READY___", "ralphos login:", "multi-user.target"]
    }

    fn boot_error_patterns(&self) -> &[&str] {
        &[
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "EROFS:",
            "emergency shell",
            "Emergency shell",
            "emergency.target",
            "rescue.target",
            "Failed to start",
            "Timed out waiting for device",
            "Dependency failed",
            "FAILED:",
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    fn critical_boot_errors(&self) -> &[&str] {
        &[
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "EROFS:",
            "emergency shell",
            "Emergency shell",
            "emergency.target",
            "rescue.target",
            "Timed out waiting for device",
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    fn service_failure_patterns(&self) -> &[&str] {
        &["Failed to start", "[FAILED]", "Dependency failed"]
    }

    fn enable_service_cmd(&self, service: &str, _target: &str) -> String {
        format!("systemctl enable {}", service)
    }

    fn check_service_exists_cmd(&self, service: &str) -> String {
        format!(
            "test -f /usr/lib/systemd/system/{}.service && echo {}",
            service, service
        )
    }

    fn check_service_status_cmd(&self, service: &str) -> String {
        format!("systemctl is-active {}", service)
    }

    fn list_failed_services_cmd(&self) -> String {
        "systemctl --failed --no-pager".to_string()
    }

    fn enabled_services(&self) -> Vec<(&str, &str, bool)> {
        vec![
            ("NetworkManager", "multi-user.target", true),
            ("chronyd", "multi-user.target", true),
            ("sshd", "multi-user.target", false),
        ]
    }

    fn enable_serial_getty_cmd(&self) -> String {
        "systemctl enable serial-getty@ttyS0.service".to_string()
    }

    fn expected_pid1_name(&self) -> &str {
        "systemd"
    }

    fn check_target_reached_cmd(&self) -> &str {
        "systemctl is-active multi-user.target"
    }

    fn target_reached_expected(&self) -> &str {
        "active"
    }

    fn count_failed_services_cmd(&self) -> &str {
        "systemctl --failed --no-legend | wc -l"
    }

    fn check_network_service_cmd(&self) -> &str {
        "systemctl is-active systemd-networkd || systemctl is-active NetworkManager"
    }

    fn install_bootloader_cmd(&self) -> &str {
        "bootctl install --esp-path=/boot --no-variables"
    }

    fn efi_entry_label(&self) -> &str {
        "RalphOS"
    }

    fn default_iso_path(&self) -> PathBuf {
        PathBuf::from(format!(
            ".artifacts/out/ralph/s01-boot/{}",
            distro_spec::ralph::ISO_FILENAME.replacen("s00_build", "s01_boot", 1)
        ))
    }

    fn chroot_shell(&self) -> &str {
        "/bin/bash"
    }

    fn default_hostname(&self) -> &str {
        "ralphos"
    }

    fn hostname_check_pattern(&self) -> &str {
        "ralph"
    }

    fn test_instrumentation_source(&self) -> &str {
        include_str!("../../../../distro-spec/src/shared/auth/files/00-levitate-test.sh")
    }

    fn default_username(&self) -> &str {
        "ralph"
    }

    fn default_password(&self) -> &str {
        "ralph"
    }

    fn login_prompt_pattern(&self) -> &str {
        "ralphos login:"
    }

    fn init_system_name(&self) -> &str {
        "systemd"
    }

    fn boot_target_name(&self) -> &str {
        "multi-user.target"
    }

    fn live_tools(&self) -> &[&str] {
        &[
            "recstrap",
            "recfstab",
            "recchroot",
            "sfdisk",
            "mkfs.ext4",
            "mount",
            "ip",
            "ping",
            "curl",
            "grep",
            "find",
        ]
    }

    fn installed_tools(&self) -> &[&str] {
        &[
            "sudo", "ip", "ssh", "bash", "mount", "umount", "dmesg", "ps", "ls", "cat",
        ]
    }
}
