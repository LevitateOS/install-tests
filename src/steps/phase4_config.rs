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
use crate::qemu::Console;
use anyhow::Result;
use leviso_cheat_guard::cheat_ensure;
use std::time::{Duration, Instant};

/// Step 10: Set timezone
pub struct SetTimezone;

impl Step for SetTimezone {
    fn num(&self) -> usize { 11 }
    fn name(&self) -> &str { "Set Timezone" }
    fn ensures(&self) -> &str {
        "System timezone is configured for correct local time display"
    }

    fn execute(&self, console: &mut Console, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Default to UTC for testing (can be parameterized later)
        let timezone = "UTC";

        // OPTIMIZATION: Check if timezone is already set correctly (squashfs default)
        let check = console.exec_chroot(
            "/mnt",
            "readlink /etc/localtime",
            Duration::from_secs(5),
        )?;

        if check.success() && check.output.contains(timezone) {
            // Already correct, skip the write
            result.add_check("Timezone already correct (skipped)", CheckResult::pass(format!("/etc/localtime → {}", timezone)));
        } else {
            // Create symlink for timezone
            let cmd = format!(
                "ln -sf /usr/share/zoneinfo/{} /etc/localtime",
                timezone
            );

            let tz_result = console.exec_chroot("/mnt", &cmd, Duration::from_secs(5))?;

            if tz_result.success() {
                result.add_check("Timezone symlink created", CheckResult::pass(format!("/etc/localtime → {}", timezone)));
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
    fn num(&self) -> usize { 12 }
    fn name(&self) -> &str { "Configure Locale" }
    fn ensures(&self) -> &str {
        "System locale is set for proper character encoding and language"
    }

    fn execute(&self, console: &mut Console, _ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use en_US.UTF-8 as default
        let locale = "en_US.UTF-8";

        // OPTIMIZATION: Check if locale is already set correctly (squashfs default)
        let check = console.exec("cat /mnt/etc/locale.conf", Duration::from_secs(5))?;

        if check.success() && check.output.contains(locale) {
            // Already correct, skip the write
            result.add_check("locale.conf already correct (skipped)", CheckResult::pass(format!("LANG={}", locale)));
        } else {
            // Write locale.conf
            console.write_file("/mnt/etc/locale.conf", &format!("LANG={}\n", locale))?;

            // Verify
            let verify = console.exec("cat /mnt/etc/locale.conf", Duration::from_secs(5))?;

            if verify.output.contains(locale) {
                result.add_check("locale.conf written", CheckResult::pass(format!("LANG={}", locale)));
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
    fn num(&self) -> usize { 13 }
    fn name(&self) -> &str { "Set Hostname" }
    fn ensures(&self) -> &str {
        "System has a hostname configured for network identification"
    }

    fn execute(&self, console: &mut Console, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use hostname from distro context
        let hostname = ctx.default_hostname();

        // Write hostname
        console.write_file("/mnt/etc/hostname", &format!("{}\n", hostname))?;

        // Write hosts file
        let hosts = format!(
            "127.0.0.1   localhost
::1         localhost
127.0.1.1   {}.localdomain {}
",
            hostname, hostname
        );
        console.write_file("/mnt/etc/hosts", &hosts)?;

        // Verify (use contains since output may include command echo)
        let verify_hostname = console.exec("cat /mnt/etc/hostname", Duration::from_secs(5))?;
        let verify_hosts = console.exec("cat /mnt/etc/hosts", Duration::from_secs(5))?;

        // Check if hostname appears as a separate line in output
        let hostname_found = verify_hostname.output
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
            result.add_check("Hosts file updated", CheckResult::pass(format!("127.0.1.1 → {}", hostname)));
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 13: Set root password
pub struct SetRootPassword;

impl Step for SetRootPassword {
    fn num(&self) -> usize { 14 }
    fn name(&self) -> &str { "Set Root Password" }
    fn ensures(&self) -> &str {
        "Root account has a password for emergency system recovery"
    }

    fn execute(&self, console: &mut Console, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use chpasswd in chroot (non-interactive)
        // NOTE: This requires unix_chkpwd in /usr/sbin (added to AUTH_SBIN in definitions.rs)
        let password_cmd = format!("echo 'root:{}' | chpasswd", ctx.default_password());

        let passwd_result = console.exec_chroot("/mnt", &password_cmd, Duration::from_secs(10))?;

        // CHEAT GUARD: Root password MUST be set for emergency access
        cheat_ensure!(
            passwd_result.success(),
            protects = "Root account has password for emergency recovery",
            severity = "CRITICAL",
            cheats = [
                "Skip password setting",
                "Accept failure silently",
                "Leave root with empty password"
            ],
            consequence = "No root password = locked out of system recovery, or security vulnerability",
            "chpasswd failed (exit {}): {}", passwd_result.exit_code, passwd_result.output
        );

        // Verify password was actually set (not still locked with ! or *)
        let verify = console.exec(
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
            "Password not set in /etc/shadow - account still locked. Is unix_chkpwd in the squashfs?"
        );

        result.add_check("Root password set", CheckResult::pass("root has password hash in /etc/shadow"));

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 14: Create user account
pub struct CreateUser;

impl Step for CreateUser {
    fn num(&self) -> usize { 15 }
    fn name(&self) -> &str { "Create User Account" }
    fn ensures(&self) -> &str {
        "Primary user account exists with proper groups for daily use"
    }

    fn execute(&self, console: &mut Console, ctx: &dyn DistroContext) -> Result<StepResult> {
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
            let check = console.exec_chroot(
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
            format!("useradd -m -s {} -G {} {}", user_shell, groups_str, username)
        };

        // Create user with home directory
        let useradd_result = console.exec_chroot(
            "/mnt",
            &useradd_cmd,
            Duration::from_secs(10),
        )?;

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
            "useradd failed (exit {}): {}", useradd_result.exit_code, useradd_result.output
        );

        result.add_check("User created", CheckResult::pass(format!("user '{}' with groups: {}", username, groups_str)));

        // Set user password (same as root password for testing)
        let passwd_result = console.exec_chroot(
            "/mnt",
            &format!("echo '{}:{}' | chpasswd", username, ctx.default_password()),
            Duration::from_secs(10),
        )?;

        // CHEAT GUARD: User password MUST be set
        cheat_ensure!(
            passwd_result.success(),
            protects = "User account has password for authentication",
            severity = "CRITICAL",
            cheats = [
                "Skip password setting",
                "Accept chpasswd failure",
                "Leave user with empty password"
            ],
            consequence = "User cannot login, or security vulnerability with empty password",
            "Failed to set password for '{}' (exit {})", username, passwd_result.exit_code
        );

        result.add_check("User password set", CheckResult::pass(format!("'{}' has password hash", username)));

        // Verify user exists
        let verify = console.exec_chroot(
            "/mnt",
            &format!("id {}", username),
            Duration::from_secs(5),
        )?;

        if verify.success() && verify.output.contains(username) {
            // Show actual id output as evidence
            result.add_check("User verified", CheckResult::pass(verify.output.trim()));
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
