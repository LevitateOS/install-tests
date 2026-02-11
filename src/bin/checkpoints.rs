//! Checkpoint-based development loop CLI.
//!
//! Lightweight, incremental checkpoints for verifying OS builds.
//!
//! Usage:
//!   cargo run --bin checkpoints -- --distro acorn --checkpoint 1
//!   cargo run --bin checkpoints -- --distro acorn --up-to 3
//!   cargo run --bin checkpoints -- --distro acorn --status
//!   cargo run --bin checkpoints -- --distro acorn --reset
//!   cargo run --bin checkpoints -- --distro acorn --checkpoint 2 --interactive

use anyhow::{bail, Result};
use clap::Parser;

use install_tests::checkpoints;

#[derive(Parser)]
#[command(name = "checkpoints")]
#[command(about = "Checkpoint-based development loop for LevitateOS variants")]
struct Cli {
    /// Distro to test (acorn, iuppiter, levitate)
    #[arg(long)]
    distro: String,

    /// Run a specific checkpoint (1-6)
    #[arg(long)]
    checkpoint: Option<u32>,

    /// Run all checkpoints up to N (inclusive)
    #[arg(long)]
    up_to: Option<u32>,

    /// Show checkpoint status
    #[arg(long)]
    status: bool,

    /// Reset checkpoint state (forces re-run)
    #[arg(long)]
    reset: bool,

    /// Interactive mode: run checkpoint test and drop to shell (like loading a video game save)
    #[arg(long)]
    interactive: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.reset {
        return checkpoints::reset_state(&cli.distro);
    }

    if cli.status {
        return checkpoints::print_status(&cli.distro);
    }

    if let Some(cp) = cli.checkpoint {
        if !(1..=6).contains(&cp) {
            bail!("Checkpoint must be 1-6, got {}", cp);
        }
        let passed = checkpoints::run_checkpoint(&cli.distro, cp)?;
        std::process::exit(if passed { 0 } else { 1 });
    }

    if let Some(target) = cli.up_to {
        if !(1..=6).contains(&target) {
            bail!("--up-to must be 1-6, got {}", target);
        }
        let passed = checkpoints::run_up_to(&cli.distro, target)?;
        std::process::exit(if passed { 0 } else { 1 });
    }

    bail!("Specify --checkpoint N, --up-to N, --status, or --reset");
}
