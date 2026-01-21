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

mod builder;
mod console;

pub use builder::{QemuBuilder, find_ovmf, create_disk};
pub use console::{Console, CommandResult};
