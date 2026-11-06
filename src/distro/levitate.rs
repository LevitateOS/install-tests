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

    // ═══════════════════════════════════════════════════════════════════════════
    // Summary Display
    // ═══════════════════════════════════════════════════════════════════════════

    fn init_system_name(&self) -> &str {
        "systemd"
    }

    fn boot_target_name(&self) -> &str {
        "multi-user.target"
    }

}
