//! E2E Installation Test Library for LevitateOS and AcornOS.
//!
//! This library provides the shared infrastructure for installation testing:
//! - QEMU backends (serial and QMP)
//! - Test steps for each installation phase
//! - Distro context for multi-distro support
//! - Executor trait for abstracting I/O backends
//!
//! # STOP. READ. THEN ACT.
//!
//! This is the CORRECT location for E2E installation tests.
//! NOT `leviso/tests/`. THIS crate. Read before writing.

pub mod checkpoints;
pub mod distro;
pub mod executor;
pub mod interactive;
pub mod preflight;
pub mod qemu;
pub mod steps;

// Re-export commonly used items
pub use distro::{context_for_distro, DistroContext, AVAILABLE_DISTROS};
pub use executor::{ExecResult, Executor};
pub use preflight::{
    require_preflight, require_preflight_for_distro, run_preflight, run_preflight_for_distro,
    run_preflight_with_iso, run_preflight_with_iso_distro, PreflightCheck, PreflightResult,
};
pub use qemu::{
    acquire_test_lock, create_disk, find_ovmf, find_ovmf_vars, kill_stale_qemu_processes, Console,
    QemuBuilder, SerialExecutorExt,
};
pub use steps::{
    all_steps, all_steps_with_experimental, steps_for_phase, steps_for_phase_experimental,
    CheckResult, CommandLog, Step, StepResult,
};
