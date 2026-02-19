//! Stage-based development loop CLI.
//!
//! Lightweight, incremental stages for verifying OS builds.
//!
//! Usage:
//!   cargo run --bin stages -- --distro acorn --stage 0
//!   cargo run --bin stages -- --distro acorn --stage 1
//!   cargo run --bin stages -- --distro acorn --up-to 3
//!   cargo run --bin stages -- --distro acorn --status
//!   cargo run --bin stages -- --distro acorn --reset

use anyhow::{bail, Result};
use clap::Parser;
use std::path::PathBuf;

use install_tests::stages;

#[derive(Parser)]
#[command(name = "stages")]
#[command(about = "Stage-based development loop for LevitateOS variants")]
struct Cli {
    /// Distro to test (levitate, acorn, iuppiter, ralph)
    #[arg(long)]
    distro: String,

    /// Run a specific stage (0-6)
    #[arg(long)]
    stage: Option<u32>,

    /// Run all stages up to N (inclusive, 0-6)
    #[arg(long)]
    up_to: Option<u32>,

    /// Show stage status
    #[arg(long)]
    status: bool,

    /// Reset stage state (forces re-run)
    #[arg(long)]
    reset: bool,

    /// Boot-inject key/value pairs (comma-separated KEY=VALUE entries).
    #[arg(long, value_name = "KEY=VALUE[,KEY=VALUE...]")]
    inject: Option<String>,

    /// Boot-inject payload file path (takes precedence over --inject).
    #[arg(long, value_name = "PATH")]
    inject_file: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    apply_boot_injection_env(&cli)?;
    let requires_guard = cli.stage.is_some() || cli.up_to.is_some();
    if requires_guard {
        install_tests::enforce_policy_guard("install-tests stages")?;
    }

    if cli.reset {
        return stages::reset_state(&cli.distro);
    }

    if cli.status {
        return stages::print_status(&cli.distro);
    }

    if let Some(stage_n) = cli.stage {
        if !(0..=6).contains(&stage_n) {
            bail!("Stage must be 0-6, got {}", stage_n);
        }
        let passed = stages::run_stage(&cli.distro, stage_n)?;
        std::process::exit(if passed { 0 } else { 1 });
    }

    if let Some(target) = cli.up_to {
        if !(0..=6).contains(&target) {
            bail!("--up-to must be 0-6, got {}", target);
        }
        let passed = stages::run_up_to(&cli.distro, target)?;
        std::process::exit(if passed { 0 } else { 1 });
    }

    bail!("Specify --stage N, --up-to N, --status, or --reset");
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
