//! Console control for QEMU serial I/O.
//!
//! Handles command execution with exit code capture.
//!
//! # STOP. READ. THEN ACT.
//!
//! This module already has:
//! - `exec()` / `exec_ok()` - Run commands with exit code capture
//! - `exec_chroot()` - Run commands in chroot via recchroot
//! - `wait_for_boot()` - Wait for systemd startup
//! - `write_file()` - Write files via serial console
//!
//! Read all methods before adding new ones. Don't duplicate functionality.

use anyhow::{Context, Result};
use std::io::BufRead;
use std::io::BufReader;
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::mpsc::{self, Receiver, Sender};

/// Result of executing a command in QEMU.
#[derive(Debug)]
pub struct CommandResult {
    /// Whether the command completed.
    pub completed: bool,
    /// Exit code (0 = success).
    pub exit_code: i32,
    /// Output from the command.
    pub output: String,
    /// Whether execution was aborted due to fatal error pattern.
    pub aborted_on_error: bool,
    /// Whether execution was aborted due to stall (no output).
    pub stalled: bool,
}

impl CommandResult {
    /// Check if the command succeeded.
    pub fn success(&self) -> bool {
        self.completed && self.exit_code == 0 && !self.aborted_on_error && !self.stalled
    }
}

/// Console controller for QEMU serial I/O.
pub struct Console {
    pub(crate) stdin: ChildStdin,
    pub(crate) rx: Receiver<String>,
    /// Output buffer for all received lines.
    pub(crate) output_buffer: Vec<String>,
    /// Services that failed during boot (tracked for diagnostics).
    pub(crate) failed_services: Vec<String>,
}

impl Console {
    /// Create a new Console from a spawned QEMU process.
    pub fn new(child: &mut Child) -> Result<Self> {
        let stdin = child.stdin.take().context("Failed to get QEMU stdin")?;
        let stdout = child.stdout.take().context("Failed to get QEMU stdout")?;

        // Spawn reader thread
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            Self::reader_thread(stdout, tx);
        });

        Ok(Self {
            stdin,
            rx,
            output_buffer: Vec::new(),
            failed_services: Vec::new(),
        })
    }

    fn reader_thread(stdout: ChildStdout, tx: Sender<String>) {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    }
}
