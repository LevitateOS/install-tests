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
//!   cargo run --bin stages -- --distro acorn --stage 2 --interactive

use anyhow::{bail, Result};
use clap::Parser;

use install_tests::interactive;
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

    /// Interactive mode: run stage test and drop to shell (like loading a video game save)
    #[arg(long)]
    interactive: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.reset {
        return stages::reset_state(&cli.distro);
    }

    if cli.status {
        return stages::print_status(&cli.distro);
    }

    if cli.interactive {
        if cli.up_to.is_some() {
            bail!("--interactive cannot be used with --up-to");
        }
        let Some(stage_n) = cli.stage else {
            bail!("--interactive requires --stage N");
        };
        if !(1..=6).contains(&stage_n) {
            bail!("Stage must be 1-6, got {}", stage_n);
        }
        return interactive::run_interactive_stage(&cli.distro, stage_n);
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
