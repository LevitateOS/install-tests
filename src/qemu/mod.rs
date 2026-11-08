//! The scenario VMs: `session` is the one launcher (libvirt domains through
//! `recqemu::libvirt`), and `serial` adds the variant's boot patterns to
//! `recqemu::Console`.

pub mod serial;
pub mod session;

pub use serial::{Console, SerialExecutorExt};
