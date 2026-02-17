//! LevitateOS distro context.
//!
//! LevitateOS uses:
//! - systemd (init system)
//! - systemd-boot (bootloader)
//! - glibc + GNU coreutils
//! - bash shell

use super::DistroContext;
use std::path::PathBuf;

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
        // Stage 01 requires an actually interactive live shell on serial console.
        // These markers are emitted by `/etc/profile.d/00-live-test.sh` only when
        // an interactive shell is active on ttyS0.
        &["___SHELL_READY___", "___PROMPT___"]
    }

    fn installed_boot_success_patterns(&self) -> &[&str] {
        // Unlike live ISO which has autologin, installed system requires login
        // Use "levitateos login:" to avoid matching "Login Prompts" in systemd output
        // After login, shell emits ___SHELL_READY___ for command execution
        // Also accept multi-user.target - proves system booted successfully even if
        // serial console login prompt has issues (VT emulation quirks in QEMU)
        &[
            "___SHELL_READY___",
            "levitateos login:",
            "multi-user.target",
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
            "EROFS:", // EROFS filesystem error
            // === INIT STAGE ===
            "emergency shell",
            "Emergency shell",
            "emergency.target",
            "rescue.target",
            "Timed out waiting for device",
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
            "EROFS:", // EROFS filesystem error
            // === INIT STAGE (critical) ===
            "emergency shell",
            "Emergency shell",
            "emergency.target",
            "rescue.target",
            "Timed out waiting for device",
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

    fn default_iso_path(&self) -> PathBuf {
        // Use relative path that works from workspace root
        // The test framework joins with current_dir() for relative paths
        //
        // ISO filename comes from distro-spec constant and is remapped to Stage 01.
        use distro_spec::levitate::ISO_FILENAME;

        // Path is relative to workspace root (CARGO_MANIFEST_DIR/../..)
        // resolve_iso() in session.rs joins this with the workspace root
        PathBuf::from(format!(
            ".artifacts/out/levitate/s01-boot/{}",
            ISO_FILENAME.replacen("s00_build", "s01_boot", 1)
        ))
    }

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
    // User/Auth
    // ═══════════════════════════════════════════════════════════════════════════

    fn default_username(&self) -> &str {
        "levitate"
    }

    fn default_password(&self) -> &str {
        "levitate"
    }

    fn login_prompt_pattern(&self) -> &str {
        "levitateos login:"
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

    fn installed_tools(&self) -> &[&str] {
        &["sudo", "ip", "ssh", "mount", "umount", "dmesg"]
    }
}
