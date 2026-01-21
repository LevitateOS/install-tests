//! Console control for QEMU serial I/O.
//!
//! Handles command execution with exit code capture and chroot state tracking.
//!
//! # STOP. READ. THEN ACT.
//!
//! This module already has:
//! - `exec()` / `exec_ok()` - Run commands with exit code capture
//! - `wait_for_boot()` - Wait for systemd startup
//! - `enter_chroot()` / `exit_chroot()` - Chroot management with bind mounts
//! - `write_file()` - Write files via serial console
//!
//! Read all methods before adding new ones. Don't duplicate functionality.

use anyhow::{bail, Context, Result};
use distro_spec::{mounts_in_order, mounts_in_unmount_order};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Result of executing a command in QEMU
#[derive(Debug)]
pub struct CommandResult {
    /// Whether the command completed
    pub completed: bool,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Output from the command
    pub output: String,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.completed && self.exit_code == 0
    }
}

/// Console controller for QEMU serial I/O
pub struct Console {
    stdin: ChildStdin,
    rx: Receiver<String>,
    /// Track whether we're in a chroot
    in_chroot: bool,
    /// Path to chroot (if any)
    chroot_path: Option<String>,
    /// Output buffer for all received lines
    output_buffer: Vec<String>,
}

const DONE_MARKER: &str = "___INSTALL_TEST_DONE___";

impl Console {
    /// Create a new Console from a spawned QEMU process
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
            in_chroot: false,
            chroot_path: None,
            output_buffer: Vec::new(),
        })
    }

    fn reader_thread(stdout: ChildStdout, tx: Sender<String>) {
        let reader = BufReader::new(stdout);
        for line in reader.lines().flatten() {
            if tx.send(line).is_err() {
                break;
            }
        }
    }

    /// Wait for the system to boot (detect "Startup finished")
    pub fn wait_for_boot(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();

        loop {
            if start.elapsed() > timeout {
                bail!("Timeout waiting for boot ({}s)", timeout.as_secs());
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());
                    if line.contains("Startup finished") {
                        // Give shell time to be ready (2s to be safe)
                        std::thread::sleep(Duration::from_secs(2));
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("QEMU exited before boot completed");
                }
            }
        }
    }

    /// Execute a command and capture output + exit code
    pub fn exec(&mut self, command: &str, timeout: Duration) -> Result<CommandResult> {
        // Build command with exit code capture
        let full_cmd = format!(
            "{}; echo '{}' $?\n",
            command, DONE_MARKER
        );

        self.stdin.write_all(full_cmd.as_bytes())?;
        self.stdin.flush()?;

        let start = Instant::now();
        let mut output = String::new();

        loop {
            if start.elapsed() > timeout {
                return Ok(CommandResult {
                    completed: false,
                    exit_code: -1,
                    output,
                });
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());

                    // Check for completion marker at the START of the trimmed line
                    // This avoids matching the command echo which contains the marker in quotes
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix(DONE_MARKER) {
                        let exit_code = rest.trim().parse().unwrap_or(-1);
                        return Ok(CommandResult {
                            completed: true,
                            exit_code,
                            output,
                        });
                    }

                    // Don't add command echo lines to output (they contain the typed command)
                    // These lines typically contain # or root@ prompt
                    if !line.contains("root@") && !line.contains("# ") {
                        output.push_str(&line);
                        output.push('\n');
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok(CommandResult {
                        completed: false,
                        exit_code: -1,
                        output,
                    });
                }
            }
        }
    }

    /// Execute a command that's expected to succeed
    pub fn exec_ok(&mut self, command: &str, timeout: Duration) -> Result<String> {
        let result = self.exec(command, timeout)?;
        if !result.success() {
            bail!(
                "Command failed (exit {}): {}\nOutput: {}",
                result.exit_code,
                command,
                result.output
            );
        }
        Ok(result.output)
    }

    /// Enter a chroot environment
    pub fn enter_chroot(&mut self, path: &str) -> Result<()> {
        if self.in_chroot {
            bail!("Already in chroot at {:?}", self.chroot_path);
        }

        // Bind mount essential filesystems in order from levitate-spec
        for mount in mounts_in_order() {
            let target = mount.full_target(path);
            let result = self.exec(
                &mount.mount_command(path),
                Duration::from_secs(5),
            )?;

            if !result.success() {
                if mount.required {
                    bail!(
                        "Failed to mount {} -> {}: {}",
                        mount.source, target, result.output
                    );
                }
                // Non-required mount failed, continue
            }
        }

        self.in_chroot = true;
        self.chroot_path = Some(path.to_string());
        Ok(())
    }

    /// Execute command in chroot context
    pub fn exec_chroot(&mut self, command: &str, timeout: Duration) -> Result<CommandResult> {
        let path = self.chroot_path.as_ref()
            .context("Not in chroot")?;

        let chroot_cmd = format!("chroot {} /bin/bash -c '{}'", path, command.replace('\'', "'\\''"));
        self.exec(&chroot_cmd, timeout)
    }

    /// Execute command in chroot context, expecting success
    pub fn exec_chroot_ok(&mut self, command: &str, timeout: Duration) -> Result<String> {
        let result = self.exec_chroot(command, timeout)?;
        if !result.success() {
            bail!(
                "Chroot command failed (exit {}): {}\nOutput: {}",
                result.exit_code,
                command,
                result.output
            );
        }
        Ok(result.output)
    }

    /// Exit chroot environment
    pub fn exit_chroot(&mut self) -> Result<()> {
        if !self.in_chroot {
            bail!("Not in chroot");
        }

        let path = self.chroot_path.take().unwrap();
        self.in_chroot = false;

        // Unmount in reverse order from levitate-spec
        for mount in mounts_in_unmount_order() {
            let target = mount.full_target(&path);
            // Use lazy unmount to avoid busy errors
            let _ = self.exec(&format!("umount -l {}", target), Duration::from_secs(5));
        }

        Ok(())
    }

    /// Check if currently in chroot
    pub fn is_in_chroot(&self) -> bool {
        self.in_chroot
    }

    /// Get all captured output
    pub fn get_all_output(&self) -> &[String] {
        &self.output_buffer
    }

    /// Write a file directly (useful for configs)
    pub fn write_file(&mut self, path: &str, content: &str) -> Result<()> {
        // Use printf with escaped content (heredocs don't work well with serial console)
        // Escape special characters for shell
        let escaped = content
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
            .replace('\n', "\\n");

        let cmd = format!("printf \"{}\" > {}", escaped, path);
        self.exec_ok(&cmd, Duration::from_secs(10))?;
        Ok(())
    }
}
