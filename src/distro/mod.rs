//! Per-distro facts for the install scenarios: the variant contracts hold
//! everything else.

use anyhow::{Context, Result};
use distro_contract::{
    ScenarioContract,
    load_variant_contract_for_distro_from, AutomatedLoginScenario, BootScenario,
    RuntimePolicyScenario, ToolsScenario,
};

pub mod acorn;
pub mod levitate;

/// What the install scenarios need to know about a distro beyond its
/// variant contract.
pub trait DistroContext: Send + Sync {
    /// Display name (e.g. "LevitateOS").
    fn name(&self) -> &str;

    /// Variant id (e.g. "levitate").
    fn id(&self) -> &str;

    /// Longest silence tolerated during live boot before it counts as a stall.
    fn live_boot_stall_timeout_secs(&self) -> u64 {
        60
    }

    /// Shell command, run in the installed system, that enables a getty on ttyS0.
    fn enable_serial_getty_cmd(&self) -> String;

    /// Command, run in the installed system, that installs systemd-boot to `/boot`.
    fn install_bootloader_cmd(&self) -> &str;

    fn default_hostname(&self) -> &str;

    /// Whether the installed system boots through an initramfs (its kernel
    /// then needs no built-in root filesystem driver).
    fn uses_initramfs(&self) -> bool;
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

/// The variant's whole scenario contract.
pub fn load_scenarios(distro_id: &str) -> Result<ScenarioContract> {
    Ok(load_variant_contract_for_distro_from(&crate::repo_root(), distro_id)
        .with_context(|| format!("loading canonical variant contract for '{}'", distro_id))?
        .scenarios)
}

pub fn load_installed_scenario_facts(distro_id: &str) -> Result<InstalledScenarioFacts> {
    let contract = load_variant_contract_for_distro_from(&crate::repo_root(), distro_id)
        .with_context(|| format!("loading canonical variant contract for '{}'", distro_id))?;
    Ok(InstalledScenarioFacts {
        installed_boot: contract.scenarios.installed_boot,
        automated_login: contract.scenarios.automated_login,
        installed_tools: contract.scenarios.installed_tools,
        runtime_policy: contract.scenarios.runtime_policy,
    })
}
