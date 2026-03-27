//! LevitateOS distro context.
//!
//! LevitateOS uses:
//! - systemd (init system)
//! - systemd-boot (bootloader)
//! - glibc + GNU coreutils
//! - bash shell

use super::DistroContext;
/// LevitateOS context for systemd-based testing.
pub struct LevitateContext;

impl DistroContext for LevitateContext {
    // ═══════════════════════════════════════════════════════════════════════════
    // Identity
    // ═══════════════════════════════════════════════════════════════════════════

    fn name(&self) -> &str {
        "LevitateOS"
    }

    fn id(&self) -> &str {
        "levitate"
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Boot Detection Patterns
    // ═══════════════════════════════════════════════════════════════════════════

    fn live_boot_success_patterns(&self) -> &[&str] {
        // Live boot requires an actually interactive shell on the serial console.
        // These markers are emitted by `/etc/profile.d/00-live-test.sh` only when
        // an interactive shell is active on ttyS0.
        &["___SHELL_READY___"]
    }

    fn boot_error_patterns(&self) -> &[&str] {
        &[
            // === UEFI PHASE ===
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            // === BOOTLOADER PHASE ===
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            // === KERNEL PHASE ===
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "SQUASHFS error",
            "EROFS:", // EROFS filesystem error
            // === INIT PHASE ===
            "emergency shell",
            "Emergency shell",
            "emergency.target",
            "rescue.target",
            "Timed out waiting for device",
            "Rootfs payload partition not found",
            // === GENERAL ===
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    fn critical_boot_errors(&self) -> &[&str] {
        &[
            // === UEFI PHASE ===
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            // === BOOTLOADER PHASE ===
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            // === KERNEL PHASE ===
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "SQUASHFS error",
            "EROFS:", // EROFS filesystem error
            // === INIT PHASE (critical) ===
            "emergency shell",
            "Emergency shell",
            "emergency.target",
            "rescue.target",
            "Timed out waiting for device",
            "Rootfs payload partition not found",
            // === GENERAL ===
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    fn service_failure_patterns(&self) -> &[&str] {
        &["Failed to start", "[FAILED]", "Dependency failed"]
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Service Management
    // ═══════════════════════════════════════════════════════════════════════════

    fn enable_service_cmd(&self, service: &str, _target: &str) -> String {
        // systemd ignores target for enable (uses [Install] section)
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
        // (service_name, target, is_required)
        // Note: Rocky 10 uses NetworkManager (not systemd-networkd) and chronyd
        vec![
            ("NetworkManager", "multi-user.target", true),
            ("chronyd", "multi-user.target", true),
            ("sshd", "multi-user.target", false),
        ]
    }

    fn enable_serial_getty_cmd(&self) -> String {
        "systemctl enable serial-getty@ttyS0.service".to_string()
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Init Verification (Phase 6)
    // ═══════════════════════════════════════════════════════════════════════════

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

    // ═══════════════════════════════════════════════════════════════════════════
    // Bootloader
    // ═══════════════════════════════════════════════════════════════════════════

    fn install_bootloader_cmd(&self) -> &str {
        // ESP is at /boot (FAT32)
        // --esp-path=/boot: REQUIRED in chroot - mount detection doesn't work
        // --no-variables: Skip EFI variable setup (not available in chroot)
        "bootctl install --esp-path=/boot --no-variables"
    }

    fn efi_entry_label(&self) -> &str {
        "LevitateOS"
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Paths
    // ═══════════════════════════════════════════════════════════════════════════

    fn chroot_shell(&self) -> &str {
        "/bin/bash"
    }

    fn default_hostname(&self) -> &str {
        "levitateos"
    }

    fn hostname_check_pattern(&self) -> &str {
        "levitate"
    }

    fn test_instrumentation_source(&self) -> &str {
        include_str!("../../../../distro-spec/src/shared/auth/files/00-levitate-test.sh")
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Summary Display
    // ═══════════════════════════════════════════════════════════════════════════

    fn init_system_name(&self) -> &str {
        "systemd"
    }

    fn boot_target_name(&self) -> &str {
        "multi-user.target"
    }

    fn live_tools(&self) -> &[&str] {
        &[
            // === Core Installation Tools ===
            "recstrap",
            "recfstab",
            "recchroot",
            "sfdisk",
            "mkfs.ext4",
            // === Network & Connectivity (daily driver) ===
            "ip",   // iproute2
            "ping", // iputils
            "curl", // curl
            // === Hardware Diagnostics (daily driver) ===
            "lspci", // pciutils
            "lsusb", // usbutils
            // === Editors & Viewers (daily driver) ===
            "vi",   // vim-minimal (Rocky default)
            "less", // less
            // === System Utilities (daily driver) ===
            "grep", // grep (coreutils)
            "find", // findutils
        ]
    }
}
