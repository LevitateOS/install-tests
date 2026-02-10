//! QEMU infrastructure for E2E installation tests.
//!
//! Provides builders and console control for running installation steps in QEMU.
//!
//! # Architecture
//!
//! This module re-exports from `recqemu` and adds test-specific extensions:
//!
//! - `QemuBuilder` - Local builder with anti-cheat protections
//! - `Console` - Re-export from recqemu (serial I/O)
//! - `patterns` - Re-export from recqemu (boot/error patterns)
//! - `qmp` - Local QMP backend for visual testing
//! - `serial` - Executor trait adapter for Console

mod builder;
pub mod patterns;
pub mod qmp;
pub mod serial;
pub mod session;

pub use builder::{
    acquire_test_lock, create_disk, find_ovmf, find_ovmf_vars, kill_stale_qemu_processes,
    QemuBuilder,
};
pub use serial::{Console, SerialExecutorExt};
