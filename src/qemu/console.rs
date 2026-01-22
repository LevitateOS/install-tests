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
    /// Whether execution was aborted due to stall (no output)
    pub stalled: bool,
}

impl CommandResult {
    pub fn success(&self) -> bool {
        self.completed && self.exit_code == 0 && !self.aborted_on_error && !self.stalled
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

/// Boot error patterns - FAIL IMMEDIATELY when seen
/// Organized by boot stage for clarity
const BOOT_ERROR_PATTERNS: &[&str] = &[
    // === UEFI STAGE ===
    "No bootable device",           // UEFI found nothing
    "Boot Failed",                  // UEFI boot failed
    "Default Boot Device Missing",  // No default boot
    "Shell>",                       // Dropped to UEFI shell (no bootloader)
    "ASSERT_EFI_ERROR",             // UEFI assertion failed
    "map: Cannot find",             // UEFI can't find device

    // === BOOTLOADER STAGE ===
    "systemd-boot: Failed",         // systemd-boot error
    "loader: Failed",               // Generic loader error
    "vmlinuz: not found",           // Kernel not on ESP
    "initramfs: not found",         // Initramfs not on ESP
    "Error loading",                // Boot file load error
    "File not found",               // Missing boot file

    // === KERNEL STAGE ===
    "Kernel panic",                 // Kernel panic
    "not syncing",                  // Panic continuation
    "VFS: Cannot open root device", // Root not found
    "No init found",                // init missing
    "Attempted to kill init",       // init crashed
    "can't find /init",             // initramfs broken
    "No root device",               // Root device missing
    "SQUASHFS error",               // Squashfs corruption

    // === INIT STAGE ===
    "emergency shell",              // Dropped to emergency
    "Emergency shell",              // Alternate casing
    "emergency.target",             // Systemd emergency
    "rescue.target",                // Systemd rescue mode
    "Failed to start",              // Service start failure (broad)
    "Timed out waiting for device", // Device timeout
    "Dependency failed",            // Systemd dep failure

    // === GENERAL ===
    "FAILED:",                      // Generic failure marker
    "fatal error",                  // Generic fatal
    "Segmentation fault",           // Segfault
    "core dumped",                  // Core dump
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

    /// Wait for the system to boot.
    ///
    /// FAIL FAST DESIGN with STALL DETECTION:
    /// - Detect failure patterns IMMEDIATELY and bail
    /// - Detect success patterns IMMEDIATELY and return
    /// - Stall timeout only triggers if NO OUTPUT for N seconds
    /// - Boot can take as long as it needs as long as progress is being made
    pub fn wait_for_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        self.wait_for_boot_with_patterns(
            stall_timeout,
            // Success patterns for live ISO boot
            // "serial-console.service" = console ready, we can interact
            &["Startup finished", "login:", "LevitateOS Live", "serial-console.service"],
            // Error patterns (shared)
            &BOOT_ERROR_PATTERNS,
        )
    }

    /// Wait for installed system to boot (different success patterns).
    pub fn wait_for_installed_boot(&mut self, stall_timeout: Duration) -> Result<()> {
        self.wait_for_boot_with_patterns(
            stall_timeout,
            // Success patterns for installed system
            // Installed systems use serial-getty@ttyS0.service (not serial-console.service which is live-only)
            &["Startup finished", "login:", "serial-getty@ttyS0", "getty.target"],
            // Error patterns (shared)
            &BOOT_ERROR_PATTERNS,
        )
    }

    /// Core boot waiting logic with configurable patterns.
    ///
    /// Uses STALL DETECTION: only fails if no output for `stall_timeout`.
    /// Boot can take as long as it needs as long as it's making progress.
    fn wait_for_boot_with_patterns(
        &mut self,
        stall_timeout: Duration,
        success_patterns: &[&str],
        error_patterns: &[&str],
    ) -> Result<()> {
        let mut last_output_time = Instant::now();

        // Track what stage we're in for better error messages
        let mut saw_uefi = false;
        let mut saw_bootloader = false;
        let mut saw_kernel = false;

        loop {
            // STALL DETECTION: Only fail if no output for stall_timeout
            // This allows boot to take as long as needed while making progress
            if last_output_time.elapsed() > stall_timeout {
                let stage = if saw_kernel {
                    "Kernel started but init STALLED (no output)"
                } else if saw_bootloader {
                    "Bootloader ran but kernel STALLED (no output)"
                } else if saw_uefi {
                    "UEFI ran but then STALLED (no output)"
                } else {
                    "No output received - QEMU or serial broken"
                };

                let last_lines: Vec<_> = self.output_buffer.iter().rev().take(30).collect();
                let context = last_lines.into_iter().rev().cloned().collect::<Vec<_>>().join("\n");
                bail!(
                    "BOOT STALLED: {}\n\
                     No output for {} seconds - system appears hung.\n\n\
                     Last output:\n{}",
                    stage, stall_timeout.as_secs(), context
                );
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    // Got output - reset stall timer
                    last_output_time = Instant::now();
                    self.output_buffer.push(line.clone());

                    // Track boot stage for better diagnostics
                    if line.contains("UEFI") || line.contains("BdsDxe") || line.contains("EFI") {
                        saw_uefi = true;
                    }
                    if line.contains("systemd-boot") || line.contains("Loading Linux") || line.contains("loader") {
                        saw_bootloader = true;
                    }
                    if line.contains("Linux version") || line.contains("Booting Linux") || line.contains("KASLR") {
                        saw_kernel = true;
                    }

                    // FAIL FAST: Check error patterns FIRST
                    for pattern in error_patterns {
                        if line.contains(pattern) {
                            let last_lines: Vec<_> = self.output_buffer.iter().rev().take(30).collect();
                            let context = last_lines.into_iter().rev().cloned().collect::<Vec<_>>().join("\n");
                            bail!("Boot failed: {}\n\nContext:\n{}", pattern, context);
                        }
                    }

                    // Check success patterns
                    for pattern in success_patterns {
                        if line.contains(pattern) {
                            // Small settle time for system to be ready
                            std::thread::sleep(Duration::from_millis(500));
                            return Ok(());
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let last_lines: Vec<_> = self.output_buffer.iter().rev().take(20).collect();
                    let context = last_lines.into_iter().rev().cloned().collect::<Vec<_>>().join("\n");
                    bail!("QEMU process died\n\nLast output:\n{}", context);
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
        let mut sync_found = false;
        loop {
            if sync_start.elapsed() > Duration::from_secs(5) {
                // Sync timeout - send a second sync to force shell ready state
                eprintln!("  WARN: Sync timeout, sending secondary sync...");

                // Drain any pending output
                loop {
                    match self.rx.try_recv() {
                        Ok(line) => self.output_buffer.push(line),
                        Err(_) => break,
                    }
                }

                // Send a secondary sync with a different marker
                let sync2_marker = format!("___SYNC2_{}___", sync_id);
                let _ = self.stdin.write_all(format!("echo '{}'\n", sync2_marker).as_bytes());
                let _ = self.stdin.flush();

                // Wait for secondary sync marker (shorter timeout)
                let sync2_start = Instant::now();
                while sync2_start.elapsed() < Duration::from_secs(3) {
                    match self.rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(line) => {
                            self.output_buffer.push(line.clone());
                            if strip_ansi_codes(&line).contains(&sync2_marker) {
                                break;
                            }
                        }
                        Err(_) => continue,
                    }
                }

                // Final drain
                std::thread::sleep(Duration::from_millis(100));
                loop {
                    match self.rx.try_recv() {
                        Ok(line) => self.output_buffer.push(line),
                        Err(_) => break,
                    }
                }
                break;
            }
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());
                    let clean = strip_ansi_codes(&line);
                    if clean.contains(&sync_marker) {
                        // Found sync marker - all previous output has been flushed
                        sync_found = true;
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        // Small delay for shell to be ready for next command
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
                    stalled: false,
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
                                stalled: false,
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
                                stalled: false,
                            });
                        }
                    }

                    // Only collect output after we've seen the start marker
                    if !collecting {
                        continue;
                    }

                    // Filter out:
                    // 1. Shell prompts (root@host:~#, [root@host ~]#, etc)
                    // 2. ANY marker lines (___START___, ___DONE___, ___SYNC___)
                    //    This includes markers from previous commands that arrive late
                    let is_prompt = line.contains("root@") || line.contains("# ");
                    let is_any_marker = trimmed.contains("___START_")
                        || trimmed.contains("___DONE_")
                        || trimmed.contains("___SYNC_");

                    if !is_prompt && !is_any_marker {
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
                        stalled: false,
                    });
                }
            }
        }
    }

    /// Execute a long-running command with STALL DETECTION instead of hard timeout.
    ///
    /// BEST PRACTICE FOR LONG-RUNNING PROCESSES:
    /// - Output is streamed and checked for errors in real-time
    /// - Error patterns cause IMMEDIATE failure (fail-fast)
    /// - No hard timeout - process runs as long as it's producing output
    /// - Only fails if process STALLS (no output for stall_timeout)
    ///
    /// Use this for commands like dracut that legitimately take a long time
    /// but should fail fast if they error.
    pub fn exec_streaming(
        &mut self,
        command: &str,
        stall_timeout: Duration,
        error_patterns: &[&str],
    ) -> Result<CommandResult> {
        // Generate unique markers
        let cmd_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let start_marker = format!("___START_{}___", cmd_id);
        let done_marker = format!("___DONE_{}___", cmd_id);

        // Build command with markers
        let full_cmd = format!(
            "echo '{}'; {}; echo '{}' $?\n",
            start_marker, command, done_marker
        );

        self.stdin.write_all(full_cmd.as_bytes())?;
        self.stdin.flush()?;

        let mut last_output_time = Instant::now();
        let mut output = String::new();
        let mut collecting = false;

        loop {
            // STALL DETECTION: Only fail if no output for stall_timeout
            if last_output_time.elapsed() > stall_timeout {
                return Ok(CommandResult {
                    completed: false,
                    exit_code: -1,
                    output,
                    aborted_on_error: false,
                    stalled: true,
                });
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    // Got output - reset stall timer
                    last_output_time = Instant::now();
                    self.output_buffer.push(line.clone());

                    let clean_line = strip_ansi_codes(&line);
                    let trimmed = clean_line.trim();

                    // FAIL FAST: Check for error patterns IMMEDIATELY
                    for pattern in error_patterns {
                        if trimmed.contains(pattern) {
                            output.push_str(&line);
                            output.push('\n');
                            return Ok(CommandResult {
                                completed: false,
                                exit_code: 1,
                                output,
                                aborted_on_error: true,
                                stalled: false,
                            });
                        }
                    }

                    // Also check fatal patterns
                    for pattern in FATAL_ERROR_PATTERNS {
                        if trimmed.contains(pattern) {
                            output.push_str(&line);
                            output.push('\n');
                            return Ok(CommandResult {
                                completed: false,
                                exit_code: 1,
                                output,
                                aborted_on_error: true,
                                stalled: false,
                            });
                        }
                    }

                    // Start marker detection
                    if trimmed.contains(&start_marker) {
                        collecting = true;
                        continue;
                    }

                    // Done marker detection
                    if let Some(pos) = trimmed.find(&done_marker) {
                        let rest = &trimmed[pos + done_marker.len()..];
                        let rest_trimmed = rest.trim();
                        if rest_trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                            let exit_code = rest_trimmed.split_whitespace().next()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(-1);
                            return Ok(CommandResult {
                                completed: true,
                                exit_code,
                                output,
                                aborted_on_error: false,
                                stalled: false,
                            });
                        }
                    }

                    // Collect output
                    if collecting {
                        let is_prompt = line.contains("root@") || line.contains("# ");
                        let is_command_echo = line.contains(&done_marker) || line.contains(&start_marker);
                        if !is_prompt && !is_command_echo {
                            output.push_str(&line);
                            output.push('\n');
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok(CommandResult {
                        completed: false,
                        exit_code: -1,
                        output,
                        aborted_on_error: false,
                        stalled: false,
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

    /// Execute a long-running command in chroot with STALL DETECTION.
    ///
    /// Same as exec_streaming but runs the command inside the chroot.
    /// Use this for commands like dracut that legitimately take a long time.
    pub fn exec_chroot_streaming(
        &mut self,
        command: &str,
        stall_timeout: Duration,
        error_patterns: &[&str],
    ) -> Result<CommandResult> {
        let path = self.chroot_path.as_ref()
            .context("Not in chroot")?;

        let chroot_cmd = format!("chroot {} /bin/bash -c '{}'", path, command.replace('\'', "'\\''"));
        self.exec_streaming(&chroot_cmd, stall_timeout, error_patterns)
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

    /// Login to the console (handles both login prompt and autologin scenarios)
    ///
    /// If autologin is enabled (console-autologin.service), we'll already be at a shell prompt.
    /// If not, we need to enter username and password.
    pub fn login(&mut self, username: &str, password: &str, timeout: Duration) -> Result<()> {
        let start = Instant::now();

        // Wait for boot output to settle and serial-console.service to be fully ready
        // The service might have "started" but bash is still initializing
        std::thread::sleep(Duration::from_millis(5000));

        // Drain any pending output first
        loop {
            match self.rx.try_recv() {
                Ok(line) => {
                    self.output_buffer.push(line);
                }
                Err(_) => break,
            }
        }

        let mut sent_username = false;
        let mut sent_password = false;
        let mut test_cmd_attempts = 0;
        let mut last_lines: Vec<String> = Vec::new();
        let mut last_action = Instant::now();

        // Send test command - if we're in a shell, this works
        self.stdin.write_all(b"echo ___LOGIN_OK___\n")?;
        self.stdin.flush()?;
        test_cmd_attempts += 1;

        loop {
            if start.elapsed() > timeout {
                let context = last_lines.iter().rev().take(25).rev().cloned().collect::<Vec<_>>().join("\n");
                bail!("Timeout waiting for login to complete\nLast output:\n{}", context);
            }

            match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());
                    last_lines.push(line.clone());
                    if last_lines.len() > 50 {
                        last_lines.remove(0);
                    }

                    let clean = strip_ansi_codes(&line);
                    let trimmed = clean.trim();
                    let lower = clean.to_lowercase();

                    // Check if we got our test marker back (proves we're in a shell)
                    // The marker might be on its own line or prefixed with bash prompt
                    if trimmed.contains("___LOGIN_OK___") && !trimmed.starts_with("echo ") {
                        return Ok(());
                    }

                    // Check for login prompt (need to send username)
                    if lower.contains("login:") && !sent_username {
                        std::thread::sleep(Duration::from_millis(100));
                        self.stdin.write_all(format!("{}\n", username).as_bytes())?;
                        self.stdin.flush()?;
                        sent_username = true;
                        last_action = Instant::now();
                    } else if lower.contains("password") && !sent_password && sent_username {
                        // Send password
                        std::thread::sleep(Duration::from_millis(100));
                        self.stdin.write_all(format!("{}\n", password).as_bytes())?;
                        self.stdin.flush()?;
                        sent_password = true;
                        last_action = Instant::now();
                    } else if lower.contains("login incorrect") || lower.contains("authentication failure") {
                        bail!("Login failed: incorrect password");
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Periodically retry the test command
                    if last_action.elapsed() > Duration::from_secs(3) && test_cmd_attempts < 8 {
                        self.stdin.write_all(b"echo ___LOGIN_OK___\n")?;
                        self.stdin.flush()?;
                        test_cmd_attempts += 1;
                        last_action = Instant::now();
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("Console disconnected during login");
                }
            }
        }
    }
}

/// Check if a line looks like a shell prompt
fn is_shell_prompt(line: &str) -> bool {
    // Empty lines aren't prompts
    if line.is_empty() {
        return false;
    }

    // Common shell prompt endings
    if line.ends_with('#') || line.ends_with('$') {
        return true;
    }

    // Prompts with space after: "root@host:~# " or "[root@host ~]$ "
    if line.ends_with("# ") || line.ends_with("$ ") {
        return true;
    }

    // Bash-style: [user@host path]# or [user@host path]$
    if line.contains("]#") || line.contains("]$") {
        return true;
    }

    // Also check for patterns like "root@hostname:path#" or "hostname:path#"
    if line.contains('@') && (line.contains('#') || line.contains('$')) {
        return true;
    }

    false
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
