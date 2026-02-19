//! Installation test utility binary.
//!
//! Serial wrapper harness execution is intentionally removed.
//! This binary now only provides step listing metadata.

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;

use install_tests::{
    all_steps_with_experimental, context_for_distro, DistroContext, AVAILABLE_DISTROS,
};

#[derive(Parser)]
#[command(name = "install-tests")]
#[command(about = "Installation test utility (list only; serial run removed)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run installation tests (disabled; legacy serial wrapper removed)
    Run {
        /// Run only a specific step (1-24)
        #[arg(long)]
        step: Option<usize>,

        /// Run only steps in a specific phase (1-6)
        #[arg(long)]
        phase: Option<usize>,

        /// Distro to test (levitate, acorn, iuppiter, ralph)
        #[arg(long, default_value = "levitate")]
        distro: String,
    },

    /// List all test steps
    List {
        /// Distro to list steps for
        #[arg(long, default_value = "levitate")]
        distro: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            step,
            phase,
            distro,
        } => {
            install_tests::enforce_policy_guard("install-tests run")?;
            bail!(
                "Legacy serial wrapper harness is removed for `install-tests run`.\n\
             Use SSH-based stage workflows instead (e.g. `just test 1 <distro>` / `just test-up-to N <distro>`).\n\
             Received args: step={:?}, phase={:?}, distro={}",
                step,
                phase,
                distro
            )
        }
        Commands::List { distro } => {
            let ctx = context_for_distro(&distro).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown distro '{}'. Available: {}",
                    distro,
                    AVAILABLE_DISTROS.join(", ")
                )
            })?;
            list_steps(&*ctx);
            Ok(())
        }
    }
}

fn list_steps(ctx: &dyn DistroContext) {
    println!(
        "{}",
        format!("{} Installation Test Steps", ctx.name()).bold()
    );
    println!();
    println!("Each step has an 'ensures' statement describing what it guarantees.");
    println!();
    println!(
        "{}",
        "Phases 1-5 run on the live ISO, Phase 6 runs after rebooting into the installed system."
            .yellow()
    );
    println!();

    let steps = all_steps_with_experimental();
    let mut current_phase = 0;

    for step in steps {
        if step.phase() != current_phase {
            current_phase = step.phase();
            println!();
            let phase_desc = match current_phase {
                1 => "Phase 1 (Boot Verification)",
                2 => "Phase 2 (Disk Setup)",
                3 => "Phase 3 (Base System)",
                4 => "Phase 4 (Configuration)",
                5 => "Phase 5 (Bootloader)",
                6 => "Phase 6 (Post-Reboot Verification) <- REBOOTS INTO INSTALLED SYSTEM",
                _ => "Unknown Phase",
            };
            println!("{}", phase_desc.blue().bold());
        }
        println!("  {:2}. {}", step.num(), step.name());
        println!("      ensures: {}", step.ensures());
    }
    println!();
}
