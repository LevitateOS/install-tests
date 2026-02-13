//! Interactive checkpoint mode.
//!
//! Like "loading a video game save" - boots to a checkpoint state and drops
//! you into an interactive shell to inspect and debug.
//!
//! # Usage
//!
//! ```bash
//! just checkpoint 2 acorn
//! ```
//!
//! This will:
//! 1. Boot QEMU with the live ISO
//! 2. Wait for boot completion
//! 3. Auto-run the checkpoint test script (e.g., checkpoint-2-live-tools.sh)
//! 4. Drop you into an interactive shell to inspect the state
//!
//! # Philosophy
//!
//! Traditional testing: Test → Pass/Fail → Exit
//! Interactive checkpoints: Test → Pass/Fail → **Interactive Shell**
//!
//! This lets you:
//! - Inspect why a test failed
//! - Manually verify test results
//! - Explore the environment
//! - Run additional commands
//! - Debug interactively

use crate::distro::{context_for_distro, DistroContext};
use crate::qemu::session;
use crate::qemu::SerialExecutorExt;
use anyhow::{bail, Result};
use colored::Colorize;
use std::path::Path;
use std::time::Duration;

/// Run an interactive checkpoint session.
///
/// This boots QEMU, optionally runs a checkpoint test script, and leaves
/// the system running for manual interaction.
pub fn run_interactive_checkpoint(distro_id: &str, checkpoint: u32) -> Result<()> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;

    let iso_path = session::resolve_iso(&*ctx)?;

    println!();
    println!(
        "{} Interactive Checkpoint {} - {}",
        ">>>".cyan().bold(),
        checkpoint,
        ctx.name()
    );
    println!();

    match checkpoint {
        1 | 2 => run_live_interactive(&*ctx, &iso_path, checkpoint),
        3..=6 => run_installed_interactive(&*ctx, checkpoint),
        _ => bail!("Invalid checkpoint number: {} (valid: 1-6)", checkpoint),
    }
}

/// Run an interactive session in the live environment (checkpoints 1-2).
fn run_live_interactive(ctx: &dyn DistroContext, iso_path: &Path, checkpoint: u32) -> Result<()> {
    println!("  Booting live ISO in QEMU...");
    let (mut child, mut console) = session::spawn_live(ctx, iso_path)?;

    println!("  Waiting for boot...");
    console.wait_for_live_boot_with_context(Duration::from_secs(60), ctx)?;
    println!("  {}", "Boot successful!".green().bold());
    println!();

    // If checkpoint > 1, run the checkpoint test script
    if checkpoint > 1 {
        let script = format!(
            "checkpoint-{}-{}.sh",
            checkpoint,
            checkpoint_name_slug(checkpoint)
        );
        println!("  Running checkpoint test script: {}", script);
        println!("  {}", "─".repeat(60));
        println!();

        // Execute the checkpoint script
        let result = console.exec(&script, Duration::from_secs(120))?;

        // Print the script output (colored test results)
        println!("{}", result.output);
        println!();
        println!("  {}", "─".repeat(60));

        if result.success() {
            println!("  {}", "Checkpoint test PASSED ✓".green().bold());
        } else {
            println!("  {}", "Checkpoint test FAILED ✗".red().bold());
        }
        println!();
    }

    println!("{}", "━".repeat(60).cyan());
    println!();
    println!("  {}", "Interactive shell ready!".green().bold());
    println!();
    println!("  You can now:");
    println!("    - Inspect the environment");
    println!("    - Run checkpoint tests manually: checkpoint-N-*.sh");
    println!("    - Verify tools work: <tool> --version");
    println!("    - Debug failures");
    println!();
    println!("  Available checkpoint scripts:");
    println!("    checkpoint-1-live-boot.sh");
    println!("    checkpoint-2-live-tools.sh");
    println!();
    println!("  Press Ctrl+A, then X to exit QEMU");
    println!();
    println!("{}", "━".repeat(60).cyan());
    println!();

    // Hand control to the user by proxying stdio to the QEMU serial console.
    // This returns when QEMU exits (or its stdout closes).
    console.attach_stdio()?;

    let status = child.wait()?;

    if status.success() {
        println!("  QEMU exited normally");
    } else {
        println!("  QEMU exited with status: {}", status);
    }

    Ok(())
}

/// Run an interactive session in the installed environment (checkpoints 3-6).
fn run_installed_interactive(_ctx: &dyn DistroContext, checkpoint: u32) -> Result<()> {
    bail!(
        "Interactive mode for checkpoint {} not yet implemented.\n\
         Currently only checkpoints 1-2 (live environment) are supported in interactive mode.",
        checkpoint
    );
}

fn checkpoint_name_slug(n: u32) -> &'static str {
    match n {
        1 => "live-boot",
        2 => "live-tools",
        3 => "installation",
        4 => "installed-boot",
        5 => "automated-login",
        6 => "daily-driver",
        _ => "unknown",
    }
}
