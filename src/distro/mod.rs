//! Distro context for parameterized testing.
//!
//! The DistroContext trait enables the same test infrastructure to work with
//! LevitateOS (systemd) and AcornOS (OpenRC) by abstracting
//! init system and bootloader differences.

use anyhow::{Context, Result};
use distro_contract::{
    ScenarioContract,
    load_variant_contract_for_distro_from, AutomatedLoginScenario, BootScenario, InstallExperience,
    RuntimePolicyScenario, ToolsScenario,
};
use std::path::PathBuf;

pub mod acorn;
pub mod levitate;
mod openrc_base;

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

    /// Shell to use in chroot.
    fn chroot_shell(&self) -> &str;

    /// Default hostname set during installation.
    fn default_hostname(&self) -> &str;

    /// Expected hostname pattern to check (may include partial match).
    fn hostname_check_pattern(&self) -> &str;

    // ═══════════════════════════════════════════════════════════════════════════
    // Summary Display
    // ═══════════════════════════════════════════════════════════════════════════

    /// Init system name for display (e.g., "systemd", "OpenRC").
    fn init_system_name(&self) -> &str;

    /// Boot target name for display (e.g., "multi-user.target", "default runlevel").
    fn boot_target_name(&self) -> &str;

}

/// Create a DistroContext based on the distro ID string.
pub fn context_for_distro(id: &str) -> Option<Box<dyn DistroContext>> {
    match id {
        "levitate" | "levitateos" => Some(Box::new(levitate::LevitateContext)),
        "acorn" | "acornos" => Some(Box::new(acorn::AcornContext)),
        _ => None,
    }
}

/// Available distro IDs for CLI help.
pub const AVAILABLE_DISTROS: &[&str] = &["levitate", "acorn"];

#[derive(Debug, Clone)]
pub struct InstalledScenarioFacts {
    pub installed_boot: BootScenario,
    pub automated_login: AutomatedLoginScenario,
    pub installed_tools: ToolsScenario,
    pub runtime_policy: RuntimePolicyScenario,
}

pub fn load_install_experience_profile(distro_id: &str) -> Result<String> {
    let contract = load_variant_contract_for_distro_from(&workspace_root(), distro_id)
        .with_context(|| format!("loading canonical variant contract for '{}'", distro_id))?;
    Ok(match contract.scenarios.live_tools.install_experience {
        InstallExperience::Ux => "ux",
        InstallExperience::AutomatedSsh => "automated_ssh",
    }
    .to_string())
}

/// The variant's whole scenario contract.
pub fn load_scenarios(distro_id: &str) -> Result<ScenarioContract> {
    Ok(load_variant_contract_for_distro_from(&workspace_root(), distro_id)
        .with_context(|| format!("loading canonical variant contract for '{}'", distro_id))?
        .scenarios)
}

pub fn load_installed_scenario_facts(distro_id: &str) -> Result<InstalledScenarioFacts> {
    let contract = load_variant_contract_for_distro_from(&workspace_root(), distro_id)
        .with_context(|| format!("loading canonical variant contract for '{}'", distro_id))?;
    Ok(InstalledScenarioFacts {
        installed_boot: contract.scenarios.installed_boot,
        automated_login: contract.scenarios.automated_login,
        installed_tools: contract.scenarios.installed_tools,
        runtime_policy: contract.scenarios.runtime_policy,
    })
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
