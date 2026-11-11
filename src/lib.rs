//! E2E Installation Test Library for LevitateOS variants.
//!
//! This library provides the shared infrastructure for installation testing:
//! - QEMU sessions and the serial console (`qemu`)
//! - Named install-test scenarios (`scenarios`) and what each expects of a
//!   variant (`expect`)
//! - Preflight checks of built artifacts against the variant (`preflight`)

pub mod boot_injection;
pub mod budget;
pub mod guest;
pub mod expect;
pub mod interactive;
pub mod preflight;
pub mod qemu;
pub mod scenarios;

pub mod update_fixture;
