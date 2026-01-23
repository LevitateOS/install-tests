//! Utility functions for QEMU console.
//!
//! Provides write_file() and login().

use anyhow::{bail, Result};
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::ansi::strip_ansi_codes;
use super::console::Console;

impl Console {
    /// Write a file directly (useful for configs).
    pub fn write_file(&mut self, path: &str, content: &str) -> Result<()> {
        // Use printf with escaped content (heredocs don't work well with serial console)
        // Escape special characters for shell
        // Bug #1 fix: Add % escape for printf format specifiers
        let escaped = content
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
            .replace('%', "%%") // Bug #1 fix: escape % for printf
            .replace('\n', "\\n");

        let cmd = format!("printf \"{}\" > {}", escaped, path);
        self.exec_ok(&cmd, Duration::from_secs(10))?;
        Ok(())
    }

    /// Login to the console (handles both login prompt and autologin scenarios).
    ///
    /// If autologin is enabled (console-autologin.service), we'll already be at a shell prompt.
    /// If not, we need to enter username and password.
    pub fn login(&mut self, username: &str, password: &str, timeout: Duration) -> Result<()> {
        let start = Instant::now();

        // Wait for boot output to settle and serial-getty service to be fully ready
        std::thread::sleep(Duration::from_millis(3000));

        // Drain any pending output first
        while let Ok(line) = self.rx.try_recv() {
            self.output_buffer.push(line);
        }

        let mut sent_username = false;
        let mut sent_password = false;
        let mut login_complete = false;
        let mut last_lines: Vec<String> = Vec::new();
        let mut awaiting_shell_test = false;

        loop {
            if start.elapsed() > timeout {
                let context = last_lines
                    .iter()
                    .rev()
                    .take(25)
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                bail!(
                    "Timeout waiting for login to complete\nLast output:\n{}",
                    context
                );
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

                    // If we already sent username+password and are waiting for shell test response
                    if awaiting_shell_test {
                        // Check if we got our test marker back (proves we're in a shell)
                        if trimmed.contains("___LOGIN_OK___")
                            && !trimmed.starts_with("echo ")
                            && !lower.contains("login:")
                        {
                            eprintln!("  DEBUG: Shell verified with marker");
                            // Final drain
                            std::thread::sleep(Duration::from_millis(500));
                            while let Ok(l) = self.rx.try_recv() {
                                self.output_buffer.push(l);
                            }
                            return Ok(());
                        }
                        // If we see login: again, login failed - reset
                        if lower.contains("login:") {
                            eprintln!("  DEBUG: Saw login prompt again, login may have failed");
                            sent_username = false;
                            sent_password = false;
                            login_complete = false;
                            awaiting_shell_test = false;
                        }
                        continue;
                    }

                    // Check for login prompt (need to send username)
                    if lower.contains("login:") && !sent_username {
                        std::thread::sleep(Duration::from_millis(200));
                        self.stdin.write_all(format!("{}\n", username).as_bytes())?;
                        self.stdin.flush()?;
                        sent_username = true;
                        eprintln!("  DEBUG: Sent username: {}", username);
                    }
                    // Check for password prompt
                    else if lower.contains("password") && sent_username && !sent_password {
                        std::thread::sleep(Duration::from_millis(200));
                        self.stdin.write_all(format!("{}\n", password).as_bytes())?;
                        self.stdin.flush()?;
                        sent_password = true;
                        login_complete = true;
                        eprintln!("  DEBUG: Sent password");

                        // After sending password, wait briefly and then send shell test
                        std::thread::sleep(Duration::from_millis(1000));

                        // Drain any output from successful login
                        while let Ok(l) = self.rx.try_recv() {
                            self.output_buffer.push(l.clone());
                            last_lines.push(l);
                        }

                        // Now send the shell test command
                        self.stdin.write_all(b"echo ___LOGIN_OK___\n")?;
                        self.stdin.flush()?;
                        awaiting_shell_test = true;
                        eprintln!("  DEBUG: Sent shell test command");
                    }
                    // Check for login failure
                    else if lower.contains("login incorrect")
                        || lower.contains("authentication failure")
                    {
                        eprintln!("  DEBUG: Login failed, retrying...");
                        sent_username = false;
                        sent_password = false;
                        login_complete = false;
                    }
                    // Check for shell prompt patterns (in case there's no password required)
                    else if !login_complete && (trimmed.ends_with('#') || trimmed.ends_with('$')) {
                        // Might already be at a shell prompt (autologin or no password)
                        self.stdin.write_all(b"echo ___LOGIN_OK___\n")?;
                        self.stdin.flush()?;
                        awaiting_shell_test = true;
                        eprintln!("  DEBUG: Detected shell prompt, sent test command");
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // If we completed login but haven't sent the test command yet, do it now
                    if login_complete && !awaiting_shell_test {
                        std::thread::sleep(Duration::from_millis(500));
                        self.stdin.write_all(b"echo ___LOGIN_OK___\n")?;
                        self.stdin.flush()?;
                        awaiting_shell_test = true;
                        eprintln!("  DEBUG: Timeout after login, sent shell test command");
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
