//! QEMU infrastructure for E2E installation tests.
//!
//! Provides builders and console control for running installation steps in QEMU.
//!
//! # Architecture
//!
//! QEMU command lines come from `recqemu::QemuBuilder`; this module adds
//! the test sessions and harness adapters:
//!
//! - `session` - live/installed VM spawns used by the scenarios
//! - `qmp` - QMP backend for visual testing
//! - `serial` - Executor trait adapter for `recqemu::Console`

pub mod qmp;
pub mod serial;
pub mod session;

pub use serial::{Console, SerialExecutorExt};
