//! QEMU infrastructure for E2E installation tests.
//!
//! Provides builders and console control for running installation steps in QEMU.
//!
//! # STOP. READ. THEN ACT.
//!
//! This module already has:
//! - `Console` - Serial I/O with command execution and exit code capture
//! - `QemuBuilder` - QEMU command line builder
//! - `find_ovmf()` - OVMF firmware discovery
//! - `create_disk()` - Virtual disk creation
//! - `serial` module - Serial console backend with auth, boot, exec
//! - `qmp` module - QMP backend for visual testing (keystrokes, screenshots)
//!
//! Read `serial/mod.rs`, `qmp/mod.rs`, and `builder.rs` before adding anything.

mod builder;
pub mod patterns;
pub mod qmp;
pub mod serial;

pub use builder::{
    acquire_test_lock, create_disk, find_ovmf, find_ovmf_vars, kill_stale_qemu_processes,
    QemuBuilder,
};
pub use serial::{Console, SerialExecutorExt};
