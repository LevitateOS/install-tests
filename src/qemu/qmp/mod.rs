//! QMP (QEMU Machine Protocol) backend for visual install testing.
//!
//! This module provides keystroke injection and screenshot capture via QMP,
//! allowing tests to emulate real user experience: see what they see, type what they type.
//!
//! # Architecture
//!
//! QMP is a JSON-based control interface built into QEMU. It can:
//! - Send keystrokes (`send-key`)
//! - Send mouse events (`input-send-event`)
//! - Capture screenshots (`screendump`)
//! - Control VM state (pause, resume, snapshot)
//!
//! # When to Use QMP vs Serial
//!
//! | Use case | Backend | Why |
//! |----------|---------|-----|
//! | CI/CD pipelines | serial | Fast, text-based verification |
//! | Quick iteration | serial | No rendering overhead |
//! | User experience validation | qmp | Emulates real keyboard input |
//! | Visual regression testing | qmp | Can capture screenshots |
//! | Debugging boot issues | serial | Full text output |
//! | Testing graphical installers | qmp | Required for GUI interaction |
//!
//! # Note on Executor Trait
//!
//! QMP intentionally does NOT implement the Executor trait. QMP cannot capture
//! command output or exit codes without OCR — any Executor impl would be
//! fraudulent (sleeping then returning success). Use the serial backend for
//! step-based testing. QMP is for visual-only workflows (smoke tests, screenshots).

mod capture;
mod client;
mod input;

pub use capture::screendump;
pub use client::QmpClient;
pub use input::{send_key, send_text, KeyCode};
