//! QEMU infrastructure for E2E installation tests.
//!
//! Provides builders and console control for running installation steps in QEMU.

mod builder;
mod console;

pub use builder::{QemuBuilder, find_ovmf, create_disk};
pub use console::{Console, CommandResult};
