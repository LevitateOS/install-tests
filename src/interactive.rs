//! Interactive stage mode.
//!
//! Like "loading a video game save" - boots to a stage state and drops
//! you into an interactive shell to inspect and debug.
//!
//! # Usage
//!
//! ```bash
//! just stage 2 acorn
//! ```
//!
//! This will:
//! 1. Boot QEMU with the live ISO
//! 2. Wait for boot completion
//! 3. Auto-run the stage test script (e.g., stage-02-live-tools.sh)
//! 4. Drop you into an interactive shell to inspect the state
//!
//! # Philosophy
//!
//! Traditional testing: Test → Pass/Fail → Exit
//! Interactive stages: Test → Pass/Fail → **Interactive Shell**
//!
//! This lets you:
//! - Inspect why a test failed
//! - Manually verify test results
//! - Explore the environment
//! - Run additional commands
//! - Debug interactively

use crate::distro::{context_for_distro, DistroContext};
use crate::preflight::require_preflight_for_distro;
use crate::qemu::session;
use crate::qemu::SerialExecutorExt;
use anyhow::{bail, Result};
use colored::Colorize;
use std::path::Path;
use std::time::Duration;

/// Run an interactive stage session.
///
/// This boots QEMU, optionally runs a stage test script, and leaves
/// the system running for manual interaction.
pub fn run_interactive_stage(distro_id: &str, stage: u32) -> Result<()> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;

    let iso_path = session::resolve_iso(&*ctx)?;
    let iso_dir = iso_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "Could not resolve ISO parent directory for '{}'",
            iso_path.display()
        )
    })?;

    require_preflight_for_distro(iso_dir, distro_id)?;

    println!();
    println!(
        "{} Interactive Stage {} - {}",
        ">>>".cyan().bold(),
        stage,
        ctx.name()
    );
    println!();

    match stage {
        1 | 2 => run_live_interactive(&*ctx, &iso_path, stage),
        3..=6 => run_installed_interactive(&*ctx, stage),
        _ => bail!("Invalid stage number: {} (valid: 1-6)", stage),
    }
}

/// Run an interactive session in the live environment (stages 1-2).
fn run_live_interactive(ctx: &dyn DistroContext, iso_path: &Path, stage: u32) -> Result<()> {
    println!("  Booting live ISO in QEMU...");
    let (mut child, mut console) = session::spawn_live(ctx, iso_path)?;

    println!("  Waiting for boot...");
    console.wait_for_live_boot_with_context(Duration::from_secs(60), ctx)?;
    println!("  {}", "Boot successful!".green().bold());
    println!();

    // If stage > 1, run the stage test script
    if stage > 1 {
        let script = format!("stage-{stage:02}-{}.sh", stage_name_slug(stage));
        println!("  Running stage test script: {}", script);
        println!("  {}", "─".repeat(60));
        println!();

        // Execute the stage script
        let result = console.exec(&script, Duration::from_secs(120))?;

        // Print the script output (colored test results)
        println!("{}", result.output);
        println!();
        println!("  {}", "─".repeat(60));

        if result.success() {
            println!("  {}", "Stage test PASSED ✓".green().bold());
        } else {
            println!("  {}", "Stage test FAILED ✗".red().bold());
        }
        println!();
    }

    println!("{}", "━".repeat(60).cyan());
    println!();
    println!("  {}", "Interactive shell ready!".green().bold());
    println!();
    println!("  You can now:");
    println!("    - Inspect the environment");
    println!("    - Run stage tests manually: stage-N-*.sh");
    println!("    - Verify tools work: <tool> --version");
    println!("    - Debug failures");
    println!();
    println!("  Available stage scripts:");
    println!("    stage-01-live-boot.sh");
    println!("    stage-02-live-tools.sh");
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

/// Run an interactive session in the installed environment (stages 3-6).
fn run_installed_interactive(_ctx: &dyn DistroContext, stage: u32) -> Result<()> {
    bail!(
        "Interactive mode for stage {} not yet implemented.\n\
         Currently only stages 1-2 (live environment) are supported in interactive mode.",
        stage
    );
}

fn stage_name_slug(n: u32) -> &'static str {
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
