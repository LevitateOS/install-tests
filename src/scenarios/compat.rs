use super::{run_scenario, run_scenario_forced, run_up_to_scenario, ScenarioId};
use anyhow::{anyhow, bail, Result};

pub fn compatibility_stage_name(scenario: ScenarioId) -> &'static str {
    match scenario {
        ScenarioId::BuildPreflight => "00Build",
        ScenarioId::LiveBoot => "01Boot",
        ScenarioId::LiveTools => "02LiveTools",
        ScenarioId::Install => "03Install",
        ScenarioId::InstalledBoot => "04LoginGate",
        ScenarioId::AutomatedLogin => "05Harness",
        ScenarioId::Runtime => "06Runtime",
    }
}

pub fn compatibility_stage_slug(scenario: ScenarioId) -> &'static str {
    match scenario {
        ScenarioId::BuildPreflight => "s00_build",
        ScenarioId::LiveBoot => "s01_boot",
        ScenarioId::LiveTools => "s02_live_tools",
        ScenarioId::Install => "s03_install",
        ScenarioId::InstalledBoot => "s04_login_gate",
        ScenarioId::AutomatedLogin => "s05_harness",
        ScenarioId::Runtime => "s06_runtime",
    }
}

pub fn compatibility_stage_dirname(scenario: ScenarioId) -> &'static str {
    match scenario {
        ScenarioId::BuildPreflight => "s00-build",
        ScenarioId::LiveBoot => "s01-boot",
        ScenarioId::LiveTools => "s02-live-tools",
        ScenarioId::Install => "s03-install",
        ScenarioId::InstalledBoot => "s04-login-gate",
        ScenarioId::AutomatedLogin => "s05-harness",
        ScenarioId::Runtime => "s06-runtime",
    }
}

pub fn compatibility_stage_number(scenario: ScenarioId) -> u32 {
    scenario.ordinal() as u32
}

pub fn scenario_for_stage_number(stage: u32) -> Result<ScenarioId> {
    match stage {
        0 => Ok(ScenarioId::BuildPreflight),
        1 => Ok(ScenarioId::LiveBoot),
        2 => Ok(ScenarioId::LiveTools),
        3 => Ok(ScenarioId::Install),
        4 => Ok(ScenarioId::InstalledBoot),
        5 => Ok(ScenarioId::AutomatedLogin),
        6 => Ok(ScenarioId::Runtime),
        _ => bail!("Invalid stage number: {} (valid aliases: 00-06)", stage),
    }
}

pub fn parse_scenario_target(value: &str) -> Option<ScenarioId> {
    ScenarioId::parse_key(value).or_else(|| {
        let trimmed = value.trim();
        match trimmed {
            "00Build" | "s00_build" | "0" | "00" => Some(ScenarioId::BuildPreflight),
            "01Boot" | "s01_boot" | "1" | "01" => Some(ScenarioId::LiveBoot),
            "02LiveTools" | "s02_live_tools" | "2" | "02" => Some(ScenarioId::LiveTools),
            "03Install" | "s03_install" | "3" | "03" => Some(ScenarioId::Install),
            "04LoginGate" | "s04_login_gate" | "4" | "04" => Some(ScenarioId::InstalledBoot),
            "05Harness" | "s05_harness" | "5" | "05" => Some(ScenarioId::AutomatedLogin),
            "06Runtime" | "s06_runtime" | "6" | "06" => Some(ScenarioId::Runtime),
            _ => None,
        }
    })
}

pub fn parse_scenario_target_arg(value: &str) -> Result<ScenarioId> {
    parse_scenario_target(value).ok_or_else(|| {
        anyhow!(
            "unsupported scenario '{}'; expected one of: build-preflight, live-boot, live-tools, install, installed-boot, automated-login, runtime; compatibility aliases: 00Build|01Boot|02LiveTools|03Install|04LoginGate|05Harness|06Runtime|0|00|1|01|2|02|3|03|4|04|5|05|6|06",
            value
        )
    })
}

pub fn run_stage(distro_id: &str, stage: u32) -> Result<bool> {
    run_scenario(distro_id, scenario_for_stage_number(stage)?)
}

pub fn run_stage_forced(distro_id: &str, stage: u32) -> Result<bool> {
    run_scenario_forced(distro_id, scenario_for_stage_number(stage)?)
}

pub fn run_up_to_stage(distro_id: &str, target: u32) -> Result<bool> {
    run_up_to_scenario(distro_id, scenario_for_stage_number(target)?)
}
