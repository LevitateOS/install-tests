//! AcornOS distro context.
//!
//! AcornOS uses:
//! - OpenRC (init system)
//! - systemd-boot (bootloader, same as LevitateOS)
//! - musl + busybox
//! - ash shell

use super::openrc_base::OpenRcBase;
use super::DistroContext;
use std::path::PathBuf;

/// AcornOS context for OpenRC-based testing.
pub struct AcornContext;

static BASE: OpenRcBase = OpenRcBase;

impl DistroContext for AcornContext {
    fn name(&self) -> &str {
        "AcornOS"
    }

    fn id(&self) -> &str {
        "acorn"
    }

    fn live_boot_success_patterns(&self) -> &[&str] {
        &["___SHELL_READY___", "[autologin]", "login:"]
    }

    fn installed_boot_success_patterns(&self) -> &[&str] {
        &[
            "___SHELL_READY___",
            "acornos login:",
            "login:",
            "Welcome to AcornOS",
        ]
    }

    fn boot_error_patterns(&self) -> &[&str] {
        BASE.boot_error_patterns()
    }

    fn critical_boot_errors(&self) -> &[&str] {
        BASE.critical_boot_errors()
    }

    fn service_failure_patterns(&self) -> &[&str] {
        BASE.service_failure_patterns()
    }

    fn enable_service_cmd(&self, service: &str, runlevel: &str) -> String {
        BASE.enable_service_cmd(service, runlevel)
    }

    fn check_service_exists_cmd(&self, service: &str) -> String {
        BASE.check_service_exists_cmd(service)
    }

    fn check_service_status_cmd(&self, service: &str) -> String {
        BASE.check_service_status_cmd(service)
    }

    fn list_failed_services_cmd(&self) -> String {
        BASE.list_failed_services_cmd()
    }

    fn enabled_services(&self) -> Vec<(&str, &str, bool)> {
        vec![
            ("networking", "boot", true),
            ("chronyd", "default", true),
            ("sshd", "default", false),
        ]
    }

    fn enable_serial_getty_cmd(&self) -> String {
        BASE.enable_serial_getty_cmd()
    }

    fn expected_pid1_name(&self) -> &str {
        BASE.expected_pid1_name()
    }

    fn check_target_reached_cmd(&self) -> &str {
        BASE.check_target_reached_cmd()
    }

    fn target_reached_expected(&self) -> &str {
        BASE.target_reached_expected()
    }

    fn count_failed_services_cmd(&self) -> &str {
        BASE.count_failed_services_cmd()
    }

    fn check_network_service_cmd(&self) -> &str {
        BASE.check_network_service_cmd()
    }

    fn install_bootloader_cmd(&self) -> &str {
        BASE.install_bootloader_cmd()
    }

    fn efi_entry_label(&self) -> &str {
        "AcornOS"
    }

    fn default_iso_path(&self) -> PathBuf {
        PathBuf::from("../../AcornOS/output/acornos.iso")
    }

    fn chroot_shell(&self) -> &str {
        BASE.chroot_shell()
    }

    fn default_hostname(&self) -> &str {
        "acornos"
    }

    fn hostname_check_pattern(&self) -> &str {
        "acorn"
    }

    fn test_instrumentation_source(&self) -> &str {
        include_str!("../../../../AcornOS/profile/live-overlay/etc/profile.d/00-acorn-test.sh")
    }

    fn default_username(&self) -> &str {
        "acorn"
    }

    fn default_password(&self) -> &str {
        "acorn"
    }

    fn login_prompt_pattern(&self) -> &str {
        "acornos login:"
    }

    fn init_system_name(&self) -> &str {
        BASE.init_system_name()
    }

    fn boot_target_name(&self) -> &str {
        BASE.boot_target_name()
    }

    fn live_tools(&self) -> &[&str] {
        &[
            "recstrap",
            "recfstab",
            "recchroot",
            "sfdisk",
            "mkfs.ext4",
            "mount",
        ]
    }

    fn installed_tools(&self) -> &[&str] {
        &[
            "sudo", "ip", "ssh", "ash", "mount", "umount", "dmesg", "ps", "ls", "cat",
        ]
    }
}
