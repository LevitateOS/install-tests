//! Scenario runner CLI with stage-number compatibility aliases.
//!
//! Lightweight, incremental scenario runner for verifying OS builds.
//!
//! Usage:
//!   cargo run --bin stages -- --distro acorn --scenario live-boot
//!   cargo run --bin stages -- --distro acorn --scenario live-tools
//!   cargo run --bin stages -- --distro acorn --stage 1
//!   cargo run --bin stages -- --distro acorn --up-to 3
//!   cargo run --bin stages -- --distro acorn --status
//!   cargo run --bin stages -- --distro acorn --reset

use anyhow::{bail, Result};
use clap::Parser;
use std::path::PathBuf;

use install_tests::stages::{self, compat};

#[derive(Parser)]
#[command(name = "scenarios", alias = "stages")]
#[command(about = "Scenario runner for LevitateOS variants (stage aliases retained)")]
struct Cli {
    /// Distro to test (levitate, acorn, iuppiter, ralph)
    #[arg(long)]
    distro: String,

    /// Run a compatibility stage alias (0-6).
    #[arg(long)]
    stage: Option<u32>,

    /// Run a specific canonical scenario.
    #[arg(long, value_name = "NAME")]
    scenario: Option<String>,

    /// Run all compatibility stage aliases up to N (inclusive, 0-6).
    #[arg(long)]
    up_to: Option<u32>,

    /// Run all scenarios up to the named canonical scenario.
    #[arg(long = "up-to-scenario", value_name = "NAME")]
    up_to_scenario: Option<String>,

    /// Show scenario status.
    #[arg(long)]
    status: bool,

    /// Reset scenario state (forces re-run).
    #[arg(long)]
    reset: bool,

    /// Boot-inject key/value pairs (comma-separated KEY=VALUE entries).
    #[arg(long, value_name = "KEY=VALUE[,KEY=VALUE...]")]
    inject: Option<String>,

    /// Boot-inject payload file path (takes precedence over --inject).
    #[arg(long, value_name = "PATH")]
    inject_file: Option<PathBuf>,

    /// Re-run the requested scenario or stage alias even if it is already cached as passed.
    #[arg(long)]
    force: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    apply_boot_injection_env(&cli)?;
    let requires_guard = cli.stage.is_some()
        || cli.scenario.is_some()
        || cli.up_to.is_some()
        || cli.up_to_scenario.is_some();
    if requires_guard {
        install_tests::enforce_policy_guard("install-tests scenarios")?;
    }

    if cli.reset {
        return stages::reset_state(&cli.distro);
    }

    if cli.status {
        return stages::print_status(&cli.distro);
    }

    if cli.force && cli.stage.is_none() && cli.scenario.is_none() {
        bail!("--force requires --stage N or --scenario NAME");
    }

    if let Some(scenario_name) = cli.scenario.as_deref() {
        let scenario = stages::parse_scenario_name(scenario_name)?;
        let passed = if cli.force {
            stages::run_scenario_forced(&cli.distro, scenario)?
        } else {
            stages::run_scenario(&cli.distro, scenario)?
        };
        std::process::exit(if passed { 0 } else { 1 });
    }

    if let Some(stage_n) = cli.stage {
        if !(0..=6).contains(&stage_n) {
            bail!("compatibility stage alias must be 0-6, got {}", stage_n);
        }
        let passed = if cli.force {
            compat::run_stage_forced(&cli.distro, stage_n)?
        } else {
            compat::run_stage(&cli.distro, stage_n)?
        };
        std::process::exit(if passed { 0 } else { 1 });
    }

    if let Some(target) = cli.up_to_scenario.as_deref() {
        let scenario = stages::parse_scenario_name(target)?;
        let passed = stages::run_up_to_scenario(&cli.distro, scenario)?;
        std::process::exit(if passed { 0 } else { 1 });
    }

    if let Some(target) = cli.up_to {
        if !(0..=6).contains(&target) {
            bail!(
                "--up-to compatibility stage alias must be 0-6, got {}",
                target
            );
        }
        let passed = compat::run_up_to_stage(&cli.distro, target)?;
        std::process::exit(if passed { 0 } else { 1 });
    }

    bail!("Specify --scenario NAME, --stage N, --up-to-scenario NAME, --up-to N, --status, or --reset");
}

fn apply_boot_injection_env(cli: &Cli) -> Result<()> {
    if let Some(path) = &cli.inject_file {
        if !path.is_file() {
            bail!("--inject-file is not a readable file: {}", path.display());
        }
        std::env::set_var("LEVITATE_BOOT_INJECTION_FILE", path);
        return Ok(());
    }
    if let Some(kv) = &cli.inject {
        if kv.trim().is_empty() {
            bail!("--inject cannot be empty");
        }
        std::env::set_var("LEVITATE_BOOT_INJECTION_KV", kv);
    }
    Ok(())
}
