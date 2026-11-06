//! E2E Installation Test Library for LevitateOS variants.
//!
//! This library provides the shared infrastructure for installation testing:
//! - QEMU sessions and the serial console (`qemu`)
//! - Named install-test scenarios (`scenarios`)
//! - Distro context for multi-distro support

pub mod boot_injection;
pub mod distro;
pub mod preflight;
pub mod qemu;
pub mod scenarios;

/// The LevitateOS checkout this crate sits in.
pub(crate) fn repo_root() -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}
