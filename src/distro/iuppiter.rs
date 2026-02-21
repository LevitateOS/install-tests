//! IuppiterOS distro context.
//!
//! IuppiterOS uses:
//! - OpenRC (init system)
//! - systemd-boot (bootloader, same as LevitateOS)
//! - musl + busybox
//! - ash shell
//! - Serial console only (headless appliance)

use super::openrc_base::OpenRcBase;
use super::DistroContext;
use std::path::PathBuf;

/// IuppiterOS context for OpenRC-based testing on headless appliance.
pub struct IuppiterContext;

static BASE: OpenRcBase = OpenRcBase;

impl DistroContext for IuppiterContext {
    fn name(&self) -> &str {
        "IuppiterOS"
    }

    fn id(&self) -> &str {
        "iuppiter"
    }

    fn live_boot_success_patterns(&self) -> &[&str] {
        &["___SHELL_READY___"]
    }

    fn installed_boot_success_patterns(&self) -> &[&str] {
        &[
            "___SHELL_READY___",
            "iuppiter login:",
            "login:",
            "Welcome to IuppiterOS",
        ]
    }

    fn boot_error_patterns(&self) -> &[&str] {
        BASE.boot_error_patterns()
    }

    fn critical_boot_errors(&self) -> &[&str] {
        // Fixed: was missing "SQUASHFS error" and "EROFS:" — now shared via OpenRcBase
        BASE.critical_boot_errors()
    }

    fn service_failure_patterns(&self) -> &[&str] {
        BASE.service_failure_patterns()
    }

    fn live_boot_stall_timeout_secs(&self) -> u64 {
        BASE.live_boot_stall_timeout_secs()
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
            ("iuppiter-engine", "default", false),
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
        "IuppiterOS"
    }

    fn default_iso_path(&self) -> PathBuf {
        // Relative to repo root; session::resolve_iso() prefixes with workspace root.
        PathBuf::from(format!(
            ".artifacts/out/iuppiter/s01-boot/{}",
            distro_spec::iuppiter::ISO_FILENAME.replacen("s00_build", "s01_boot", 1)
        ))
    }

    fn chroot_shell(&self) -> &str {
        BASE.chroot_shell()
    }

    fn default_hostname(&self) -> &str {
        "iuppiter"
    }

    fn hostname_check_pattern(&self) -> &str {
        "iuppiter"
    }

    fn test_instrumentation_source(&self) -> &str {
        include_str!(
            "../../../../distro-variants/iuppiter/profile/live-overlay/etc/profile.d/00-iuppiter-test.sh"
        )
    }

    fn default_username(&self) -> &str {
        "operator"
    }

    fn default_password(&self) -> &str {
        "iuppiter"
    }

    fn login_prompt_pattern(&self) -> &str {
        "iuppiter login:"
    }

    fn init_system_name(&self) -> &str {
        BASE.init_system_name()
    }

    fn boot_target_name(&self) -> &str {
        BASE.boot_target_name()
    }

    fn live_tools(&self) -> &[&str] {
        &[
            // === Core Installation Tools ===
            "recstrap",
            "recfstab",
            "recchroot",
            "sfdisk",
            "mkfs.ext4",
            "mount",
            // === Appliance HDD Tools (critical) ===
            "smartctl", // smartmontools - disk health
            "hdparm",   // hdparm - disk parameters
            "sg_inq",   // sg3_utils - SCSI inquiry
            // === Network & Connectivity (essential) ===
            "ip",   // iproute2
            "ping", // iputils
            // === System Utilities (essential) ===
            "less", // less - pager for logs
            "grep", // grep - text search
            "find", // findutils - file search
        ]
    }

    fn installed_tools(&self) -> &[&str] {
        &[
            "sudo", "ip", "ssh", "ash", "smartctl", "hdparm", "sg_inq", "mount", "umount", "dmesg",
        ]
    }
}
