//! Scenario runner CLI.
//!
//! Lightweight, incremental scenario runner for verifying OS builds.
//!
//! Usage:
//!   cargo run --bin scenarios -- --distro acorn --scenario live-boot
//!   cargo run --bin scenarios -- --distro acorn --scenario live-tools
//!   cargo run --bin scenarios -- --distro acorn --up-to-scenario install
//!   cargo run --bin scenarios -- --distro acorn --status
//!   cargo run --bin scenarios -- --distro acorn --reset

use anyhow::{bail, Result};
use clap::Parser;
use std::path::PathBuf;

use install_tests::scenarios;

#[derive(Parser)]
#[command(name = "scenarios")]
#[command(about = "Scenario runner for LevitateOS variants")]
struct Cli {
    /// Distro to test (levitate, acorn, iuppiter, ralph)
    #[arg(long)]
    distro: String,

    /// Run a specific canonical scenario.
    #[arg(long, value_name = "NAME")]
    scenario: Option<String>,

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

    /// Re-run the requested scenario even if it is already cached as passed.
    #[arg(long)]
    force: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    apply_boot_injection_env(&cli)?;
    let requires_guard = cli.scenario.is_some() || cli.up_to_scenario.is_some();
    if requires_guard {
        install_tests::enforce_policy_guard("install-tests scenarios")?;
    }

    if cli.reset {
        return scenarios::reset_state(&cli.distro);
    }

    if cli.status {
        return scenarios::print_status(&cli.distro);
    }

    if cli.force && cli.scenario.is_none() {
        bail!("--force requires --scenario NAME");
    }

    if let Some(scenario_name) = cli.scenario.as_deref() {
        let scenario = scenarios::parse_scenario_name(scenario_name)?;
        let passed = if cli.force {
            scenarios::run_scenario_forced(&cli.distro, scenario)?
        } else {
            scenarios::run_scenario(&cli.distro, scenario)?
        };
        std::process::exit(if passed { 0 } else { 1 });
    }

    if let Some(target) = cli.up_to_scenario.as_deref() {
        let scenario = scenarios::parse_scenario_name(target)?;
        let passed = scenarios::run_up_to_scenario(&cli.distro, scenario)?;
        std::process::exit(if passed { 0 } else { 1 });
    }

    bail!("Specify --scenario NAME, --up-to-scenario NAME, --status, or --reset");
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
