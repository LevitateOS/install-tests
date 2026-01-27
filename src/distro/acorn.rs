//! AcornOS distro context.
//!
//! AcornOS uses:
//! - OpenRC (init system)
//! - systemd-boot (bootloader, same as LevitateOS)
//! - musl + busybox
//! - ash shell

use super::DistroContext;
use std::path::PathBuf;

/// AcornOS context for OpenRC-based testing.
pub struct AcornContext;

impl DistroContext for AcornContext {
    // ═══════════════════════════════════════════════════════════════════════════
    // Identity
    // ═══════════════════════════════════════════════════════════════════════════

    fn name(&self) -> &str {
        "AcornOS"
    }

    fn id(&self) -> &str {
        "acorn"
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Boot Detection Patterns
    // ═══════════════════════════════════════════════════════════════════════════

    fn live_boot_success_patterns(&self) -> &[&str] {
        // AcornOS test instrumentation markers (in order of preference):
        // 1. ___SHELL_READY___ - emitted by 00-acorn-test.sh when shell is ready (ideal)
        // 2. [autologin] - emitted by serial-autologin before shell starts (fallback)
        // 3. login: - generic login prompt (last resort, may not appear with autologin)
        //
        // NOTE: Do NOT use "=== ACORNOS INIT STARTING ===" - that appears DURING
        // initramfs, long before OpenRC finishes and efivarfs is available
        &[
            "___SHELL_READY___",
            "[autologin]",
            "login:",
        ]
    }

    fn installed_boot_success_patterns(&self) -> &[&str] {
        // For installed system boot detection (in order of preference):
        // 1. ___SHELL_READY___ - test instrumentation (if installed)
        // 2. acornos login: - hostname-prefixed login prompt (reliable)
        // 3. login: - generic login prompt (fallback)
        // 4. Welcome to AcornOS - MOTD/issue message (fallback)
        &[
            "___SHELL_READY___",
            "acornos login:",
            "login:",
            "Welcome to AcornOS",
        ]
    }

    fn boot_error_patterns(&self) -> &[&str] {
        &[
            // === UEFI STAGE ===
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            // === BOOTLOADER STAGE ===
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            // === KERNEL STAGE ===
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "SQUASHFS error",
            "EROFS:",                 // EROFS filesystem error
            // === OPENRC INIT STAGE ===
            "ERROR: cannot start",
            "ERROR: ",
            // === GENERAL ===
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    fn critical_boot_errors(&self) -> &[&str] {
        &[
            // === UEFI STAGE ===
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            // === BOOTLOADER STAGE ===
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            // === KERNEL STAGE ===
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "SQUASHFS error",
            "EROFS:",                 // EROFS filesystem error
            // === GENERAL ===
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    fn service_failure_patterns(&self) -> &[&str] {
        // OpenRC service failure patterns
        &["ERROR: cannot start", "* ERROR:", "crashed"]
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Service Management
    // ═══════════════════════════════════════════════════════════════════════════

    fn enable_service_cmd(&self, service: &str, runlevel: &str) -> String {
        // OpenRC uses runlevels: boot, sysinit, default, shutdown
        format!("rc-update add {} {}", service, runlevel)
    }

    fn check_service_exists_cmd(&self, service: &str) -> String {
        // OpenRC init scripts are in /etc/init.d/
        format!("test -f /etc/init.d/{} && echo {}", service, service)
    }

    fn check_service_status_cmd(&self, service: &str) -> String {
        format!("rc-service {} status", service)
    }

    fn list_failed_services_cmd(&self) -> String {
        "rc-status --crashed 2>/dev/null || rc-status -a | grep -E 'stopped|crashed'".to_string()
    }

    fn enabled_services(&self) -> Vec<(&str, &str, bool)> {
        // (service_name, runlevel, is_required)
        vec![
            ("networking", "boot", true),
            ("chronyd", "default", true),
            ("sshd", "default", false),
        ]
    }

    fn enable_serial_getty_cmd(&self) -> String {
        // Alpine uses agetty in /etc/inittab for serial console
        // This command ensures ttyS0 is enabled in inittab
        "grep -q 'ttyS0' /etc/inittab || echo 'ttyS0::respawn:/sbin/getty -L 115200 ttyS0 vt100' >> /etc/inittab".to_string()
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Init Verification (Phase 6)
    // ═══════════════════════════════════════════════════════════════════════════

    fn expected_pid1_name(&self) -> &str {
        // Busybox init
        "init"
    }

    fn check_target_reached_cmd(&self) -> &str {
        // Check if default runlevel is reached and services started
        "rc-status default 2>/dev/null | grep -q started && echo 'default_reached'"
    }

    fn target_reached_expected(&self) -> &str {
        "default_reached"
    }

    fn count_failed_services_cmd(&self) -> &str {
        // Count crashed/stopped services
        "rc-status --crashed 2>/dev/null | wc -l || echo 0"
    }

    fn check_network_service_cmd(&self) -> &str {
        "rc-service networking status 2>/dev/null | grep -q started && echo 'active'"
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Bootloader
    // ═══════════════════════════════════════════════════════════════════════════

    fn install_bootloader_cmd(&self) -> &str {
        // AcornOS also uses systemd-boot despite using OpenRC
        // Same command as LevitateOS
        "bootctl install --esp-path=/boot --no-variables"
    }

    fn efi_entry_label(&self) -> &str {
        "AcornOS"
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Paths
    // ═══════════════════════════════════════════════════════════════════════════

    fn default_iso_path(&self) -> PathBuf {
        PathBuf::from("../../AcornOS/output/acornos.iso")
    }

    fn chroot_shell(&self) -> &str {
        "/bin/ash"
    }

    fn default_hostname(&self) -> &str {
        "acornos"
    }

    fn hostname_check_pattern(&self) -> &str {
        "acorn"
    }

    fn test_instrumentation_source(&self) -> &str {
        // AcornOS test instrumentation - ash-compatible version
        include_str!("../../../../AcornOS/profile/live-overlay/etc/profile.d/00-acorn-test.sh")
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // User/Auth
    // ═══════════════════════════════════════════════════════════════════════════

    fn default_username(&self) -> &str {
        "acorn"
    }

    fn default_password(&self) -> &str {
        "acorn"
    }

    fn login_prompt_pattern(&self) -> &str {
        "acornos login:"
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Summary Display
    // ═══════════════════════════════════════════════════════════════════════════

    fn init_system_name(&self) -> &str {
        "OpenRC"
    }

    fn boot_target_name(&self) -> &str {
        "default runlevel"
    }
}
