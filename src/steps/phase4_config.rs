//! Phase 4: System configuration steps.
//!
//! Steps 11-15: Timezone, locale, hostname, root password, user creation.

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use distro_spec::levitate::{DEFAULT_HOSTNAME, default_user};
use std::time::{Duration, Instant};

/// Step 10: Set timezone
pub struct SetTimezone;

impl Step for SetTimezone {
    fn num(&self) -> usize { 11 }
    fn name(&self) -> &str { "Set Timezone" }
    fn ensures(&self) -> &str {
        "System timezone is configured for correct local time display"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Default to UTC for testing (can be parameterized later)
        let timezone = "UTC";

        // Create symlink for timezone
        let cmd = format!(
            "ln -sf /usr/share/zoneinfo/{} /etc/localtime",
            timezone
        );

        let tz_result = console.exec_chroot(&cmd, Duration::from_secs(5))?;

        if tz_result.success() {
            result.add_check(
                "Timezone symlink created",
                CheckResult::Pass(format!("Set to {}", timezone)),
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

        // Verify
        let verify = console.exec_chroot(
            "ls -la /etc/localtime",
            Duration::from_secs(5),
        )?;

        if verify.output.contains(timezone) {
            result.add_check(
                "Timezone verified",
                CheckResult::Pass(verify.output.trim().to_string()),
            );
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

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use en_US.UTF-8 as default
        let locale = "en_US.UTF-8";

        // Write locale.conf
        console.write_file("/mnt/etc/locale.conf", &format!("LANG={}\n", locale))?;

        // Verify
        let verify = console.exec("cat /mnt/etc/locale.conf", Duration::from_secs(5))?;

        if verify.output.contains(locale) {
            result.add_check(
                "locale.conf written",
                CheckResult::Pass(format!("LANG={}", locale)),
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

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use hostname from levitate-spec
        let hostname = DEFAULT_HOSTNAME;

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
            result.add_check(
                "Hostname set",
                CheckResult::Pass(hostname.to_string()),
            );
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
                CheckResult::Pass("Contains hostname".to_string()),
            );
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

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use chpasswd in chroot (non-interactive)
        // For testing, use a simple password (in production, this would be parameterized)
        let password_cmd = "echo 'root:levitate' | chpasswd";

        let passwd_result = console.exec_chroot(password_cmd, Duration::from_secs(10))?;

        if passwd_result.success() {
            result.add_check(
                "Root password set",
                CheckResult::Pass("Password configured".to_string()),
            );
        } else {
            result.add_check(
                "Root password set",
                CheckResult::Fail {
                    expected: "chpasswd exit 0".to_string(),
                    actual: format!("exit {}: {}", passwd_result.exit_code, passwd_result.output),
                },
            );
        }

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

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Use user spec from distro-spec (LevitateOS defaults)
        let user = default_user("levitate");
        let username = &user.username;

        // First, check which groups actually exist in the target system
        let mut available_groups = Vec::new();
        for group in user.groups.iter() {
            let check = console.exec_chroot(
                &format!("getent group {}", group),
                Duration::from_secs(5),
            )?;
            if check.exit_code == 0 {
                available_groups.push(group.as_str());
            }
        }

        // Build useradd command with only available groups
        let groups_str = available_groups.join(",");
        let useradd_cmd = if available_groups.is_empty() {
            format!("useradd -m -s {} {}", user.shell, username)
        } else {
            format!("useradd -m -s {} -G {} {}", user.shell, groups_str, username)
        };

        // Create user with home directory
        let useradd_result = console.exec_chroot(
            &useradd_cmd,
            Duration::from_secs(10),
        )?;

        if useradd_result.success() {
            result.add_check(
                "User created",
                CheckResult::Pass(format!("User '{}' created", username)),
            );
        } else {
            result.add_check(
                "User created",
                CheckResult::Fail {
                    expected: "useradd exit 0".to_string(),
                    actual: format!("exit {}: {}", useradd_result.exit_code, useradd_result.output),
                },
            );
            return Ok(result);
        }

        // Set user password
        let passwd_result = console.exec_chroot(
            &format!("echo '{}:levitate' | chpasswd", username),
            Duration::from_secs(10),
        )?;

        if passwd_result.success() {
            result.add_check(
                "User password set",
                CheckResult::Pass("Password configured".to_string()),
            );
        } else {
            result.add_check(
                "User password set",
                CheckResult::Fail {
                    expected: "chpasswd exit 0".to_string(),
                    actual: format!("exit {}", passwd_result.exit_code),
                },
            );
        }

        // Verify user exists
        let verify = console.exec_chroot(
            &format!("id {}", username),
            Duration::from_secs(5),
        )?;

        if verify.success() && verify.output.contains(username) {
            result.add_check(
                "User verified",
                CheckResult::Pass(verify.output.trim().to_string()),
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
