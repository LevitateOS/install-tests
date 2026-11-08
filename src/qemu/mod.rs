//! The scenario VMs: `session` is the one launcher (libvirt domains through
//! `recqemu::libvirt`), and `serial` holds the boot waits.

pub mod serial;
pub mod session;

pub use serial::{wait_installed_boot, wait_live_boot, Console};
