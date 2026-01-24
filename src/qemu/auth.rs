//! Authentication subsystem for QEMU console.
//!
//! # Overview
//!
//! This module handles serial console login for the test harness.
//! It provides a clear state machine with retry logic and diagnostics.
//!
//! # Why This Module Exists
//!
//! Serial console authentication has been a persistent pain point:
//! - Race conditions between send and receive
//! - Line fragmentation and ANSI escape codes
//! - Shell prompt variations
//! - Bash startup time variability
//!
//! This module centralizes all auth logic with:
//! - Clear state machine with explicit transitions
//! - Retry logic with exponential backoff
//! - Comprehensive diagnostics
//!
//! # Usage
//!
//! ```ignore
//! // Simple login (backward compatible)
//! console.login("root", "levitate", Duration::from_secs(30))?;
//!
//! // With custom config
//! let config = AuthConfig { debug: true, ..Default::default() };
//! let result = console.authenticate_with_config(
//!     "root", "levitate",
//!     Duration::from_secs(30),
//!     &config,
//! )?;
//! ```

use anyhow::{bail, Result};
use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::ansi::strip_ansi_codes;
use super::console::Console;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for authentication attempts.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Base delay between retries (doubles each attempt).
    pub retry_delay: Duration,
    /// Timeout for receiving a single line.
    pub line_timeout: Duration,
    /// Enable verbose debug output.
    pub debug: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay: Duration::from_millis(500),
            line_timeout: Duration::from_millis(500),
            debug: false,
        }
    }
}

// ============================================================================
// Login State Machine
// ============================================================================

/// State machine for password-based login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginState {
    /// Waiting for "login:" prompt from getty.
    WaitingForLoginPrompt,
    /// Username sent, waiting for "Password:" prompt.
    WaitingForPasswordPrompt,
    /// Password sent, waiting for shell prompt.
    WaitingForShellPrompt,
    /// Shell prompt detected, verifying with test command.
    VerifyingShell,
    /// Authentication complete.
    Complete,
}

impl LoginState {
    /// Human-readable description of the state.
    fn description(&self) -> &'static str {
        match self {
            Self::WaitingForLoginPrompt => "waiting for login prompt",
            Self::WaitingForPasswordPrompt => "waiting for password prompt",
            Self::WaitingForShellPrompt => "waiting for shell prompt",
            Self::VerifyingShell => "verifying shell access",
            Self::Complete => "complete",
        }
    }
}

// ============================================================================
// Auth Result
// ============================================================================

/// Result of an authentication attempt.
#[derive(Debug)]
struct AuthResult {
    /// Whether authentication succeeded.
    success: bool,
    /// Final state of the state machine.
    final_state: LoginState,
    /// Number of attempts made.
    attempts: u32,
    /// Diagnostic information (last N lines of console output).
    diagnostics: Vec<String>,
    /// Error message if failed.
    error: Option<String>,
}

impl AuthResult {
    fn success(state: LoginState, attempts: u32) -> Self {
        Self {
            success: true,
            final_state: state,
            attempts,
            diagnostics: Vec::new(),
            error: None,
        }
    }

    fn failure(state: LoginState, attempts: u32, diagnostics: Vec<String>, error: String) -> Self {
        Self {
            success: false,
            final_state: state,
            attempts,
            diagnostics,
            error: Some(error),
        }
    }
}

// ============================================================================
// Constants
// ============================================================================

/// Marker used to verify shell is responding.
const LOGIN_OK_MARKER: &str = "___LOGIN_OK___";

// ============================================================================
// Auth Implementation
// ============================================================================

impl Console {
    /// Login to the console with username and password.
    ///
    /// This is the main entry point for authentication. It handles:
    /// - Password-based login with state machine
    /// - Retry logic with diagnostics
    ///
    /// # Arguments
    ///
    /// * `username` - Username to login as
    /// * `password` - Password for the user
    /// * `timeout` - Overall timeout for authentication
    ///
    /// # Returns
    ///
    /// `Ok(())` if authentication succeeds, `Err` with diagnostics if it fails.
    pub fn login(&mut self, username: &str, password: &str, timeout: Duration) -> Result<()> {
        let config = AuthConfig::default();
        self.authenticate_with_config(username, password, timeout, &config)
    }

    /// Authenticate with custom configuration.
    pub fn authenticate_with_config(
        &mut self,
        username: &str,
        password: &str,
        timeout: Duration,
        config: &AuthConfig,
    ) -> Result<()> {
        let result = self.auth_password(username, password, timeout, config)?;

        if result.success {
            Ok(())
        } else {
            let context = result
                .diagnostics
                .iter()
                .rev()
                .take(25)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");

            bail!(
                "Authentication failed: {}\nState: {:?}\nAttempts: {}\nLast output:\n{}",
                result.error.unwrap_or_else(|| "unknown error".to_string()),
                result.final_state,
                result.attempts,
                context
            );
        }
    }

    /// Password-based authentication via serial console.
    fn auth_password(
        &mut self,
        username: &str,
        password: &str,
        timeout: Duration,
        config: &AuthConfig,
    ) -> Result<AuthResult> {
        let start = Instant::now();
        let mut state = LoginState::WaitingForLoginPrompt;
        let mut retry_count: u32 = 0;
        let mut diagnostics: Vec<String> = Vec::new();

        // Brief settle time for getty to be ready
        std::thread::sleep(Duration::from_millis(300));

        // Drain pending output
        while let Ok(line) = self.rx.try_recv() {
            self.output_buffer.push(line.clone());
            diagnostics.push(line.clone());
            if diagnostics.len() > 50 {
                diagnostics.remove(0);
            }
        }

        // Wake up getty with newline
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;

        loop {
            // Check overall timeout
            if start.elapsed() > timeout {
                return Ok(AuthResult::failure(
                    state,
                    retry_count + 1,
                    diagnostics,
                    format!(
                        "Timeout after {:?} while {}",
                        timeout,
                        state.description()
                    ),
                ));
            }

            // Check retry limit
            if retry_count >= config.max_retries {
                return Ok(AuthResult::failure(
                    state,
                    retry_count,
                    diagnostics,
                    format!(
                        "Max retries ({}) exceeded while {}",
                        config.max_retries,
                        state.description()
                    ),
                ));
            }

            match self.rx.recv_timeout(config.line_timeout) {
                Ok(line) => {
                    self.output_buffer.push(line.clone());
                    diagnostics.push(line.clone());
                    if diagnostics.len() > 50 {
                        diagnostics.remove(0);
                    }

                    let clean = strip_ansi_codes(&line);
                    let trimmed = clean.trim();
                    let lower = clean.to_lowercase();

                    if config.debug {
                        eprintln!("  AUTH[{:?}]: {:?}", state, trimmed);
                    }

                    // Process based on current state
                    match state {
                        LoginState::WaitingForLoginPrompt => {
                            if lower.contains("login:") {
                                self.stdin
                                    .write_all(format!("{}\n", username).as_bytes())?;
                                self.stdin.flush()?;
                                state = LoginState::WaitingForPasswordPrompt;
                            }
                        }

                        LoginState::WaitingForPasswordPrompt => {
                            if lower.contains("password") {
                                self.stdin
                                    .write_all(format!("{}\n", password).as_bytes())?;
                                self.stdin.flush()?;
                                state = LoginState::WaitingForShellPrompt;
                            } else if lower.contains("login incorrect")
                                || lower.contains("authentication failure")
                            {
                                // Login failed, retry
                                retry_count += 1;
                                state = LoginState::WaitingForLoginPrompt;
                                std::thread::sleep(config.retry_delay * retry_count);
                            } else if lower.contains("login:") && !trimmed.contains(username) {
                                // Login prompt returned without password prompt
                                // Username may have been rejected
                                retry_count += 1;
                                state = LoginState::WaitingForLoginPrompt;
                            }
                        }

                        LoginState::WaitingForShellPrompt => {
                            // Check for login failure
                            if lower.contains("login incorrect")
                                || lower.contains("authentication failure")
                            {
                                retry_count += 1;
                                state = LoginState::WaitingForLoginPrompt;
                                std::thread::sleep(config.retry_delay * retry_count);
                                continue;
                            }

                            // Check for login prompt (failure without message)
                            if lower.contains("login:") {
                                retry_count += 1;
                                state = LoginState::WaitingForLoginPrompt;
                                continue;
                            }

                            // Check for shell prompt (successful login)
                            if trimmed.ends_with('#') || trimmed.ends_with('$') {
                                // Verify shell is responding
                                self.stdin
                                    .write_all(format!("echo {}\n", LOGIN_OK_MARKER).as_bytes())?;
                                self.stdin.flush()?;
                                state = LoginState::VerifyingShell;
                            }
                        }

                        LoginState::VerifyingShell => {
                            // Check for our verification marker
                            if trimmed.contains(LOGIN_OK_MARKER)
                                && !trimmed.starts_with("echo ")
                            {
                                // Brief drain for trailing output
                                std::thread::sleep(Duration::from_millis(100));
                                while let Ok(l) = self.rx.try_recv() {
                                    self.output_buffer.push(l);
                                }
                                return Ok(AuthResult::success(LoginState::Complete, retry_count + 1));
                            }

                            // Login prompt means verification failed
                            if lower.contains("login:") {
                                retry_count += 1;
                                state = LoginState::WaitingForLoginPrompt;
                            }
                        }

                        LoginState::Complete => {
                            // Should not reach here - we return earlier
                            break;
                        }
                    }
                }

                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // On timeout in WaitingForShellPrompt, try sending newline
                    if state == LoginState::WaitingForShellPrompt {
                        self.stdin.write_all(b"\n")?;
                        self.stdin.flush()?;
                    }
                    continue;
                }

                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok(AuthResult::failure(
                        state,
                        retry_count + 1,
                        diagnostics,
                        "Console disconnected during authentication".to_string(),
                    ));
                }
            }
        }

        Ok(AuthResult::failure(
            state,
            retry_count + 1,
            diagnostics,
            "Authentication loop exited unexpectedly".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_state_descriptions() {
        assert_eq!(
            LoginState::WaitingForLoginPrompt.description(),
            "waiting for login prompt"
        );
        assert_eq!(
            LoginState::WaitingForPasswordPrompt.description(),
            "waiting for password prompt"
        );
        assert_eq!(LoginState::Complete.description(), "complete");
    }

    #[test]
    fn test_auth_config_defaults() {
        let config = AuthConfig::default();
        assert_eq!(config.max_retries, 3);
        assert!(!config.debug);
    }
}
