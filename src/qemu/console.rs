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
    /// Whether execution was aborted due to fatal error pattern
    pub aborted_on_error: bool,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.completed && self.exit_code == 0 && !self.aborted_on_error
    }
}

/// Fatal error patterns that should cause immediate failure
/// When ANY of these appear in output, stop waiting and return failure
const FATAL_ERROR_PATTERNS: &[&str] = &[
    "dracut[F]:",                   // dracut fatal error
    "dracut[E]: FAILED:",           // dracut install failed
    "dracut-install: ERROR:",       // dracut-install binary failed
    "FATAL:",                       // Generic fatal
    "Kernel panic",                 // Kernel panic
    "not syncing",                  // Kernel panic continuation
    "Segmentation fault",           // Segfault
    "core dumped",                  // Core dump
    "systemd-coredump",             // Systemd detected crash
];

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
        // First, send a sync command and wait for its completion
        // This ensures all previous output has been flushed through the serial console
        let sync_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let sync_marker = format!("___SYNC_{}___", sync_id);

        // Send sync command
        let sync_cmd = format!("echo '{}'\n", sync_marker);
        self.stdin.write_all(sync_cmd.as_bytes())?;
        self.stdin.flush()?;

        // Wait for sync marker with short timeout, draining everything before it
        let sync_start = Instant::now();
        loop {
            if sync_start.elapsed() > Duration::from_secs(5) {
                // Sync timeout - continue anyway but log it
                eprintln!("  WARN: Sync timeout, continuing anyway");
                break;
            }
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());
                    let clean = strip_ansi_codes(&line);
                    if clean.contains(&sync_marker) {
                        // Found sync marker - all previous output has been flushed
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Small additional delay for shell to be ready for next command
        std::thread::sleep(Duration::from_millis(50));

        // Generate unique markers for this command
        let cmd_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let start_marker = format!("___START_{}___", cmd_id);
        let done_marker = format!("___DONE_{}___", cmd_id);

        // Build command with unique start and end markers
        let full_cmd = format!(
            "echo '{}'; {}; echo '{}' $?\n",
            start_marker, command, done_marker
        );

        self.stdin.write_all(full_cmd.as_bytes())?;
        self.stdin.flush()?;

        let exec_start = Instant::now();
        let mut output = String::new();
        let mut collecting = false;

        loop {
            if exec_start.elapsed() > timeout {
                return Ok(CommandResult {
                    completed: false,
                    exit_code: -1,
                    output,
                    aborted_on_error: false,
                });
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());

                    // Strip ANSI escape codes for cleaner matching
                    let clean_line = strip_ansi_codes(&line);
                    let trimmed = clean_line.trim();

                    // FAIL FAST: Check for fatal error patterns IMMEDIATELY
                    for pattern in FATAL_ERROR_PATTERNS {
                        if trimmed.contains(pattern) {
                            eprintln!("  FATAL ERROR DETECTED: {}", trimmed);
                            output.push_str(&line);
                            output.push('\n');
                            return Ok(CommandResult {
                                completed: false,
                                exit_code: 1,
                                output,
                                aborted_on_error: true,
                            });
                        }
                    }

                    // Wait for start marker before collecting output
                    // Check if line contains our marker (not just ends with - may have terminal codes)
                    if trimmed.contains(&start_marker) {
                        collecting = true;
                        continue;
                    }

                    // Check for completion marker (unique per command)
                    // The done marker is followed by a space and exit code (number).
                    // Using unique markers avoids matching output from previous commands.
                    if let Some(pos) = trimmed.find(&done_marker) {
                        let rest = &trimmed[pos + done_marker.len()..];
                        let rest_trimmed = rest.trim();
                        // Only match if the rest starts with a digit (the exit code)
                        if rest_trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                            let exit_code = rest_trimmed.split_whitespace().next()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(-1);
                            return Ok(CommandResult {
                                completed: true,
                                exit_code,
                                output,
                                aborted_on_error: false,
                            });
                        }
                    }

                    // Only collect output after we've seen the start marker
                    if !collecting {
                        continue;
                    }

                    // Filter out:
                    // 1. Shell prompts (root@host:~#, [root@host ~]#, etc)
                    // 2. Command echo lines (contain the done marker in quotes)
                    // 3. Start marker lines
                    let is_prompt = line.contains("root@") || line.contains("# ");
                    let is_command_echo = line.contains(&done_marker) || line.contains(&start_marker);

                    if !is_prompt && !is_command_echo {
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
                        aborted_on_error: false,
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

/// Strip ANSI escape codes from a string
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip escape sequence
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Skip until we find a letter (the command)
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}
