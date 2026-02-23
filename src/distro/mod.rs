//! Distro context for parameterized testing.
//!
//! The DistroContext trait enables the same test infrastructure to work with
//! LevitateOS/RalphOS (systemd) and AcornOS/IuppiterOS (OpenRC) by abstracting
//! init system and bootloader differences.

pub mod acorn;
pub mod iuppiter;
pub mod levitate;
mod openrc_base;
pub mod ralph;

use std::path::PathBuf;

/// Context for distro-specific test behavior.
///
/// This trait abstracts the differences between init systems (systemd vs OpenRC),
/// boot detection patterns, and system verification commands.
pub trait DistroContext: Send + Sync {
    // ═══════════════════════════════════════════════════════════════════════════
    // Identity
    // ═══════════════════════════════════════════════════════════════════════════

    /// Display name for the distro (e.g., "LevitateOS", "AcornOS").
    fn name(&self) -> &str;

    /// Short identifier (e.g., "levitate", "acorn").
    fn id(&self) -> &str;

    // ═══════════════════════════════════════════════════════════════════════════
    // Boot Detection Patterns
    // ═══════════════════════════════════════════════════════════════════════════

    /// Patterns indicating successful live ISO boot.
    ///
    /// Any of these appearing in console output signals boot success.
    fn live_boot_success_patterns(&self) -> &[&str];

    /// Patterns indicating successful installed system boot.
    fn installed_boot_success_patterns(&self) -> &[&str];

    /// Patterns indicating fatal boot error.
    ///
    /// If any of these appear, the test fails immediately.
    fn boot_error_patterns(&self) -> &[&str];

    /// Patterns indicating critical boot errors (always fatal).
    fn critical_boot_errors(&self) -> &[&str];

    /// Patterns indicating service failures to track (not immediately fatal).
    #[allow(dead_code)]
    fn service_failure_patterns(&self) -> &[&str];

    /// Max silence window tolerated during live boot before declaring stall.
    ///
    /// OpenRC early boot can be quiet for longer than systemd.
    fn live_boot_stall_timeout_secs(&self) -> u64 {
        60
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Service Management
    // ═══════════════════════════════════════════════════════════════════════════

    /// Command to enable a service.
    ///
    /// For systemd: `systemctl enable <service>`
    /// For OpenRC: `rc-update add <service> <runlevel>`
    fn enable_service_cmd(&self, service: &str, target: &str) -> String;

    /// Command to check if a service exists (unit file present).
    fn check_service_exists_cmd(&self, service: &str) -> String;

    /// Command to check service status.
    #[allow(dead_code)]
    fn check_service_status_cmd(&self, service: &str) -> String;

    /// Command to list failed services.
    fn list_failed_services_cmd(&self) -> String;

    /// Services that should be enabled during installation.
    ///
    /// Returns (service_name, target/runlevel, is_required).
    fn enabled_services(&self) -> Vec<(&str, &str, bool)>;

    /// Command to enable serial console getty for testing.
    fn enable_serial_getty_cmd(&self) -> String;

    // ═══════════════════════════════════════════════════════════════════════════
    // Init Verification (Phase 6)
    // ═══════════════════════════════════════════════════════════════════════════

    /// Expected name of PID 1 process.
    ///
    /// For systemd: "systemd"
    /// For OpenRC: "init"
    fn expected_pid1_name(&self) -> &str;

    /// Command to check if system reached boot target.
    ///
    /// For systemd: `systemctl is-active multi-user.target`
    /// For OpenRC: `rc-status default | grep -q started`
    fn check_target_reached_cmd(&self) -> &str;

    /// Expected output indicating target reached.
    fn target_reached_expected(&self) -> &str;

    /// Command to count failed units/services.
    fn count_failed_services_cmd(&self) -> &str;

    /// Command to get network service status.
    fn check_network_service_cmd(&self) -> &str;

    // ═══════════════════════════════════════════════════════════════════════════
    // Bootloader
    // ═══════════════════════════════════════════════════════════════════════════

    /// Command to install the bootloader (run in chroot).
    #[allow(dead_code)]
    fn install_bootloader_cmd(&self) -> &str;

    /// EFI entry label for efibootmgr.
    fn efi_entry_label(&self) -> &str;

    // ═══════════════════════════════════════════════════════════════════════════
    // Paths
    // ═══════════════════════════════════════════════════════════════════════════

    /// Default ISO path (relative to workspace root).
    fn default_iso_path(&self) -> PathBuf;

    /// Shell to use in chroot.
    fn chroot_shell(&self) -> &str;

    /// Default hostname set during installation.
    fn default_hostname(&self) -> &str;

    /// Expected hostname pattern to check (may include partial match).
    fn hostname_check_pattern(&self) -> &str;

    /// Path to test instrumentation script to copy to installed system.
    fn test_instrumentation_source(&self) -> &str;

    // ═══════════════════════════════════════════════════════════════════════════
    // User/Auth
    // ═══════════════════════════════════════════════════════════════════════════

    /// Default username created during installation.
    fn default_username(&self) -> &str;

    /// Default password for both root and user.
    fn default_password(&self) -> &str;

    /// Login prompt pattern to detect.
    #[allow(dead_code)]
    fn login_prompt_pattern(&self) -> &str;

    // ═══════════════════════════════════════════════════════════════════════════
    // Summary Display
    // ═══════════════════════════════════════════════════════════════════════════

    /// Init system name for display (e.g., "systemd", "OpenRC").
    fn init_system_name(&self) -> &str;

    /// Boot target name for display (e.g., "multi-user.target", "default runlevel").
    fn boot_target_name(&self) -> &str;

    // ═══════════════════════════════════════════════════════════════════════════
    // Tool Expectations (Stages)
    // ═══════════════════════════════════════════════════════════════════════════

    /// Tools expected to be present in the live ISO environment.
    fn live_tools(&self) -> &[&str];

    /// Stage 02 install experience profile expected in the live ISO.
    ///
    /// Allowed values:
    /// - `ux`
    /// - `automated_ssh`
    fn stage02_install_experience(&self) -> &str;

    /// Tools expected to be present on the installed system.
    fn installed_tools(&self) -> &[&str];
}

/// Create a DistroContext based on the distro ID string.
pub fn context_for_distro(id: &str) -> Option<Box<dyn DistroContext>> {
    match id {
        "levitate" | "levitateos" => Some(Box::new(levitate::LevitateContext)),
        "acorn" | "acornos" => Some(Box::new(acorn::AcornContext)),
        "iuppiter" | "iuppiteros" => Some(Box::new(iuppiter::IuppiterContext)),
        "ralph" | "ralphos" => Some(Box::new(ralph::RalphContext)),
        _ => None,
    }
}

/// Available distro IDs for CLI help.
pub const AVAILABLE_DISTROS: &[&str] = &["levitate", "acorn", "iuppiter", "ralph"];
