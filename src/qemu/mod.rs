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
//!
//! Read `console.rs` and `builder.rs` before adding anything.

mod ansi;
mod boot;
mod builder;
mod chroot;
mod console;
mod exec;
mod patterns;
mod sync;
mod utils;

pub use builder::{
    acquire_test_lock, create_disk, find_ovmf, find_ovmf_vars, kill_stale_qemu_processes,
    QemuBuilder,
};
pub use console::Console;
