//! Phase 4: System configuration steps.
//!
//! Steps 11-15: Timezone, locale, hostname, root password, user creation.
//!
//! # Cheat Prevention
//!
//! Configuration MUST happen in chroot, not live environment.
//! User creation MUST include password - empty passwords = security hole.

use super::{CheckResult, Step, StepResult};
use crate::distro::DistroContext;
use crate::executor::Executor;
use anyhow::Result;
use leviso_cheat_guard::cheat_ensure;
use std::time::{Duration, Instant};

/// Escape a string for use in single-quoted shell context.
/// Handles single quotes by ending the quote, adding escaped quote, and resuming.
/// Example: "it's" -> "it'\''s" (in shell: 'it'\''s' = it's)
fn shell_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Escape a string for use in sed replacement pattern.
/// SHA-512 hashes contain base64 chars (A-Za-z0-9./) plus $ delimiters.
/// We use | as the sed delimiter, so we only need to escape $ and backslashes.
fn escape_for_sed(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('&', "\\&") // & has special meaning in sed replacement
}

/// Step 10: Set timezone
pub struct SetTimezone;

impl Step for SetTimezone {
    fn num(&self) -> usize {
        11
    }
    fn name(&self) -> &str {
        "Set Timezone"
    }
    fn ensures(&self) -> &str {
        "System timezone is configured for correct local time display"
    }

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Default to UTC for testing (can be parameterized later)
        let timezone = "UTC";

        // OPTIMIZATION: Check if timezone is already set correctly (rootfs default)
        let check =
            executor.exec_chroot("/mnt", "readlink /etc/localtime", Duration::from_secs(5))?;

        if check.success() && check.output.contains(timezone) {
            // Already correct, skip the write
            result.add_check(
                "Timezone already correct (skipped)",
                CheckResult::pass(format!("/etc/localtime → {}", timezone)),
            );
        } else {
            // Create symlink for timezone
            let cmd = format!("ln -sf /usr/share/zoneinfo/{} /etc/localtime", timezone);

            let tz_result = executor.exec_chroot("/mnt", &cmd, Duration::from_secs(5))?;

            if tz_result.success() {
                result.add_check(
                    "Timezone symlink created",
                    CheckResult::pass(format!("/etc/localtime → {}", timezone)),
                );
            } else {
                result.add_check(
                    "Timezone symlink created",
                    CheckResult::Fail {
                        expected: "symlink created".to_string(),
                        actual: format!("exit {}", tz_result.exit_code),
                    },
                );
            }
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 11: Configure locale
pub struct ConfigureLocale;

impl Step for ConfigureLocale {
    fn num(&self) -> usize {
        12
    }
    fn name(&self) -> &str {
        "Configure Locale"
    }
    fn ensures(&self) -> &str {
        "System locale is set for proper character encoding and language"
    }

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use en_US.UTF-8 as default
        let locale = "en_US.UTF-8";

        // OPTIMIZATION: Check if locale is already set correctly (rootfs default)
        let check = executor.exec("cat /mnt/etc/locale.conf", Duration::from_secs(5))?;

        if check.success() && check.output.contains(locale) {
            // Already correct, skip the write
            result.add_check(
                "locale.conf already correct (skipped)",
                CheckResult::pass(format!("LANG={}", locale)),
            );
        } else {
            // Write locale.conf
            executor.write_file("/mnt/etc/locale.conf", &format!("LANG={}\n", locale))?;

            // Verify
            let verify = executor.exec("cat /mnt/etc/locale.conf", Duration::from_secs(5))?;

            if verify.output.contains(locale) {
                result.add_check(
                    "locale.conf written",
                    CheckResult::pass(format!("LANG={}", locale)),
                );
            } else {
                result.add_check(
                    "locale.conf written",
                    CheckResult::Fail {
                        expected: format!("LANG={}", locale),
                        actual: verify.output.clone(),
                    },
                );
            }
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 12: Set hostname
pub struct SetHostname;

impl Step for SetHostname {
    fn num(&self) -> usize {
        13
    }
    fn name(&self) -> &str {
        "Set Hostname"
    }
    fn ensures(&self) -> &str {
        "System has a hostname configured for network identification"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use hostname from distro context
        let hostname = ctx.default_hostname();

        // Write hostname
        executor.write_file("/mnt/etc/hostname", &format!("{}\n", hostname))?;

        // Write hosts file
        let hosts = format!(
            "127.0.0.1   localhost
::1         localhost
127.0.1.1   {}.localdomain {}
",
            hostname, hostname
        );
        executor.write_file("/mnt/etc/hosts", &hosts)?;

        // Verify (use contains since output may include command echo)
        let verify_hostname = executor.exec("cat /mnt/etc/hostname", Duration::from_secs(5))?;
        let verify_hosts = executor.exec("cat /mnt/etc/hosts", Duration::from_secs(5))?;

        // Check if hostname appears as a separate line in output
        let hostname_found = verify_hostname
            .output
            .lines()
            .any(|line| line.trim() == hostname);

        if hostname_found {
            result.add_check("Hostname set", CheckResult::pass(hostname));
        } else {
            result.add_check(
                "Hostname set",
                CheckResult::Fail {
                    expected: hostname.to_string(),
                    actual: verify_hostname.output.trim().to_string(),
                },
            );
        }

        if verify_hosts.output.contains(hostname) {
            result.add_check(
                "Hosts file updated",
                CheckResult::pass(format!("127.0.1.1 → {}", hostname)),
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 13: Set root password
pub struct SetRootPassword;

impl Step for SetRootPassword {
    fn num(&self) -> usize {
        14
    }
    fn name(&self) -> &str {
        "Set Root Password"
    }
    fn ensures(&self) -> &str {
        "Root account has a password for emergency system recovery"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // WORKAROUND: chpasswd via PAM silently fails in chroot environments.
        // Instead, generate hash with openssl and edit /etc/shadow directly.
        // This was documented in KNOWLEDGE_install-test-debugging.md (W2) and
        // is now codified in the build system.
        //
        // See: https://github.com/systemd/systemd/issues/9197
        let password = ctx.default_password();

        // Generate SHA-512 password hash using openssl (available on all systems)
        // The -6 option uses SHA-512 (same as yescrypt in terms of security)
        // Use -stdin to avoid shell escaping issues with special characters in password
        let hash_cmd = format!(
            "printf '%s' '{}' | openssl passwd -6 -stdin",
            shell_escape(password)
        );
        let hash_result = executor.exec(&hash_cmd, Duration::from_secs(10))?;

        cheat_ensure!(
            hash_result.success(),
            protects = "Password hash can be generated",
            severity = "CRITICAL",
            cheats = [
                "Skip password hashing",
                "Use empty hash",
                "Hardcode a known hash"
            ],
            consequence = "No valid password hash = no login possible",
            "openssl passwd failed (exit {}): {}",
            hash_result.exit_code,
            hash_result.output
        );

        let hash = hash_result.output.trim();

        // Verify hash format (SHA-512 hashes start with $6$)
        cheat_ensure!(
            hash.starts_with("$6$"),
            protects = "Password hash is valid SHA-512 format",
            severity = "CRITICAL",
            cheats = ["Accept any string as hash", "Skip format validation"],
            consequence = "Invalid hash format = login will fail",
            "Invalid hash format: expected $6$..., got: {}",
            hash
        );

        // Edit /etc/shadow directly using sed to replace root's password field
        // The shadow format is: username:password:lastchanged:min:max:warn:inactive:expire:reserved
        // We replace the second field (password) with our hash
        // SHA-512 hashes contain only base64 chars (A-Za-z0-9./) plus $ delimiters
        let sed_cmd = format!(
            "sed -i 's|^root:[^:]*:|root:{}:|' /mnt/etc/shadow",
            escape_for_sed(hash)
        );
        let sed_result = executor.exec(&sed_cmd, Duration::from_secs(5))?;

        cheat_ensure!(
            sed_result.success(),
            protects = "Shadow file can be modified",
            severity = "CRITICAL",
            cheats = ["Skip shadow modification", "Accept sed failure"],
            consequence = "Password not written = no login possible",
            "sed failed (exit {}): {}",
            sed_result.exit_code,
            sed_result.output
        );

        // Verify password was actually set (not still locked with ! or *)
        let verify = executor.exec(
            "grep '^root:' /mnt/etc/shadow | grep -v ':!:' | grep -v ':\\*:'",
            Duration::from_secs(5),
        )?;

        cheat_ensure!(
            verify.success(),
            protects = "Root password is actually set in /etc/shadow",
            severity = "CRITICAL",
            cheats = [
                "Trust command exit code without verification",
                "Skip shadow file check",
                "Accept locked account as success"
            ],
            consequence = "Root account appears locked (! or *), login will fail",
            "Password not set in /etc/shadow - account still locked"
        );

        result.add_check(
            "Root password set",
            CheckResult::pass("root has SHA-512 hash in /etc/shadow"),
        );

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 14: Create user account
pub struct CreateUser;

impl Step for CreateUser {
    fn num(&self) -> usize {
        15
    }
    fn name(&self) -> &str {
        "Create User Account"
    }
    fn ensures(&self) -> &str {
        "Primary user account exists with proper groups for daily use"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Get user details from distro context
        let username = ctx.default_username();
        let user_shell = ctx.chroot_shell(); // Use chroot shell as default user shell

        // Default groups for sudo/admin access
        let default_groups = ["wheel", "audio", "video", "input"];

        // First, check which groups actually exist in the target system
        let mut available_groups = Vec::new();
        for group in default_groups {
            let check = executor.exec_chroot(
                "/mnt",
                &format!("getent group {}", group),
                Duration::from_secs(5),
            )?;
            if check.exit_code == 0 {
                available_groups.push(group);
            }
        }

        // Build useradd command with only available groups
        let groups_str = available_groups.join(",");
        let useradd_cmd = if available_groups.is_empty() {
            format!("useradd -m -s {} {}", user_shell, username)
        } else {
            format!(
                "useradd -m -s {} -G {} {}",
                user_shell, groups_str, username
            )
        };

        // Create user with home directory
        let useradd_result = executor.exec_chroot("/mnt", &useradd_cmd, Duration::from_secs(10))?;

        // CHEAT GUARD: User account MUST be created for daily use
        cheat_ensure!(
            useradd_result.success(),
            protects = "Primary user account exists for daily operation",
            severity = "CRITICAL",
            cheats = [
                "Skip user creation",
                "Ignore useradd errors",
                "Only check if command was attempted"
            ],
            consequence = "No user account, must login as root (dangerous), or locked out entirely",
            "useradd failed (exit {}): {}",
            useradd_result.exit_code,
            useradd_result.output
        );

        result.add_check(
            "User created",
            CheckResult::pass(format!("user '{}' with groups: {}", username, groups_str)),
        );

        // Set user password using direct shadow manipulation (same workaround as root password)
        // chpasswd via PAM silently fails in chroot environments
        let password = ctx.default_password();

        // Generate SHA-512 password hash using stdin to avoid shell escaping issues
        let hash_cmd = format!(
            "printf '%s' '{}' | openssl passwd -6 -stdin",
            shell_escape(password)
        );
        let hash_result = executor.exec(&hash_cmd, Duration::from_secs(10))?;

        cheat_ensure!(
            hash_result.success() && hash_result.output.trim().starts_with("$6$"),
            protects = "User password hash can be generated",
            severity = "CRITICAL",
            cheats = ["Skip password hashing", "Use invalid hash format"],
            consequence = "No valid password hash = user cannot login",
            "openssl passwd failed for user '{}' (exit {}): {}",
            username,
            hash_result.exit_code,
            hash_result.output
        );

        let hash = hash_result.output.trim();

        // Edit /etc/shadow to set user's password
        // Username is from distro context (safe), hash is base64 + $ (safe with proper escaping)
        let sed_cmd = format!(
            "sed -i 's|^{}:[^:]*:|{}:{}:|' /mnt/etc/shadow",
            username,
            username,
            escape_for_sed(hash)
        );
        let sed_result = executor.exec(&sed_cmd, Duration::from_secs(5))?;

        // CHEAT GUARD: User password MUST be set
        cheat_ensure!(
            sed_result.success(),
            protects = "User account has password for authentication",
            severity = "CRITICAL",
            cheats = [
                "Skip password setting",
                "Accept sed failure",
                "Leave user with empty password"
            ],
            consequence = "User cannot login, or security vulnerability with empty password",
            "Failed to set password for '{}' (exit {}): {}",
            username,
            sed_result.exit_code,
            sed_result.output
        );

        result.add_check(
            "User password set",
            CheckResult::pass(format!("'{}' has SHA-512 hash", username)),
        );

        // Verify user exists
        let verify =
            executor.exec_chroot("/mnt", &format!("id {}", username), Duration::from_secs(5))?;

        if verify.success() && verify.output.contains(username) {
            // Show actual id output as evidence
            result.add_check("User verified", CheckResult::pass(verify.output.trim()));
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
