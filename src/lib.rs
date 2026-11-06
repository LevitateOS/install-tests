//! E2E Installation Test Library for LevitateOS variants.
//!
//! This library provides the shared infrastructure for installation testing:
//! - QEMU backends (serial and QMP)
//! - Named install-test scenarios (`scenarios`)
//! - Distro context for multi-distro support
//! - Executor trait for abstracting I/O backends
//!
//! # STOP. READ. THEN ACT.
//!
//! This is the CORRECT location for E2E installation tests. Read before writing.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod boot_injection;
pub mod distro;
pub mod executor;
pub mod preflight;
pub mod qemu;
pub mod scenarios;

// Re-export commonly used items
pub use boot_injection::{
    boot_injection_from_env, BootInjection, FW_CFG_NAME as BOOT_INJECTION_FW_CFG_NAME,
};
pub use distro::{context_for_distro, DistroContext, AVAILABLE_DISTROS};
pub use executor::{ExecResult, Executor};
pub use preflight::{
    require_preflight, require_preflight_for_distro, require_preflight_with_iso_for_distro,
    run_preflight, run_preflight_for_distro, run_preflight_with_iso, run_preflight_with_iso_distro,
    PreflightCheck, PreflightResult,
};
pub use qemu::{Console, SerialExecutorExt};

pub fn enforce_policy_guard(entrypoint: &str) -> Result<()> {
    let repo_root = locate_repo_root()?;
    let status = Command::new("cargo")
        .current_dir(&repo_root)
        .args(["xtask", "policy", "audit-legacy-bindings"])
        .status()
        .with_context(|| {
            format!(
                "running legacy-binding policy guard before '{}' execution",
                entrypoint
            )
        })?;

    if status.success() {
        return Ok(());
    }

    bail!(
        "policy guard failed before '{}' execution (exit: {}). \
Run `cargo xtask policy audit-legacy-bindings` and fix violations first.",
        entrypoint,
        status
    )
}

fn locate_repo_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest_dir.ancestors() {
        let candidate = Path::new(ancestor);
        if candidate.join("xtask").is_dir() && candidate.join("distro-variants").is_dir() {
            return Ok(candidate.to_path_buf());
        }
    }
    bail!(
        "unable to locate repository root from '{}' for policy guard",
        manifest_dir.display()
    )
}
