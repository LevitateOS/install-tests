//! E2E Installation Test Library for LevitateOS variants.
//!
//! This library provides the shared infrastructure for installation testing:
//! - QEMU backends (serial and QMP)
//! - Named install-test scenarios (`scenarios`)
//! - Distro context for multi-distro support
//!
//! # STOP. READ. THEN ACT.
//!
//! This is the CORRECT location for E2E installation tests. Read before writing.


pub mod boot_injection;
pub mod distro;
pub mod preflight;
pub mod qemu;
pub mod scenarios;

// Re-export commonly used items
pub use distro::{context_for_distro, DistroContext, AVAILABLE_DISTROS};
pub use preflight::{require_preflight, run_preflight, PreflightCheck, PreflightResult};
pub use qemu::{Console, SerialExecutorExt};

/// The LevitateOS checkout this crate sits in.
pub(crate) fn repo_root() -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}
