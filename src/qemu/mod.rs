//! QEMU for the scenarios. Command lines come from `recqemu::QemuBuilder`;
//! `session` spawns the live and installed VMs, and `serial` adds the
//! distro's boot patterns to `recqemu::Console`.

pub mod serial;
pub mod session;

pub use serial::{Console, SerialExecutorExt};
