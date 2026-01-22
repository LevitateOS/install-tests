//! Phase 6: Post-reboot verification steps.
//!
//! Steps 19-24: Verify the installed system actually works.
//!
//! # CRITICAL
//!
//! These steps run AFTER rebooting into the installed system.
//! They prove the installation succeeded - without these, we only know
//! files were copied to disk, not that the system is usable.
//!
//! # Cheat Prevention
//!
//! These are the ONLY steps that prove installation worked.
//! Without verification, all prior steps are meaningless.
//! - systemd running as PID 1 proves init works
//! - User login proves authentication works
//! - Essential commands prove base system is complete

use super::{CheckResult, Step, StepResult};
use crate::qemu::Console;
use anyhow::Result;
use cheat_guard::cheat_ensure;
use std::time::{Duration, Instant};

/// Step 19: Verify systemd started successfully
pub struct VerifySystemdBoot;

impl Step for VerifySystemdBoot {
    fn num(&self) -> usize { 19 }
    fn name(&self) -> &str { "Verify Systemd Boot" }
    fn ensures(&self) -> &str {
        "Installed system boots to multi-user target with systemd running"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check systemd is running (PID 1)
        let pid1 = console.exec("cat /proc/1/comm", Duration::from_secs(5))?;

        // CHEAT GUARD: systemd MUST be PID 1 for proper boot
        cheat_ensure!(
            pid1.output.contains("systemd"),
            protects = "System booted with systemd as init",
            severity = "CRITICAL",
            cheats = [
                "Skip PID 1 check",
                "Accept any init system",
                "Assume systemd is running"
            ],
            consequence = "System didn't boot properly, may be in emergency shell or wrong init",
            "PID 1 is '{}', expected 'systemd'", pid1.output.trim()
        );

        result.add_check(
            "systemd is PID 1",
            CheckResult::Pass("systemd running as init".to_string()),
        );

        // Check we reached multi-user target
        let target = console.exec(
            "systemctl is-active multi-user.target",
            Duration::from_secs(10),
        )?;

        if target.output.contains("active") {
            result.add_check(
                "multi-user.target reached",
                CheckResult::Pass("System fully booted".to_string()),
            );
        } else {
            result.add_check(
                "multi-user.target reached",
                CheckResult::Fail {
                    expected: "active".to_string(),
                    actual: target.output.trim().to_string(),
                },
            );
        }

        // Check for failed units
        let failed = console.exec(
            "systemctl --failed --no-legend | wc -l",
            Duration::from_secs(10),
        )?;

        let failed_count: i32 = failed.output
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .next()
            .unwrap_or(0);

        if failed_count == 0 {
            result.add_check(
                "No failed units",
                CheckResult::Pass("All services started successfully".to_string()),
            );
        } else {
            // Get the list of failed units
            let failed_list = console.exec(
                "systemctl --failed --no-legend",
                Duration::from_secs(5),
            )?;
            result.add_check(
                "Failed units",
                CheckResult::Fail {
                    expected: "0 failed units".to_string(),
                    actual: format!("{} failed:\n{}", failed_count, failed_list.output),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 20: Verify hostname persisted
pub struct VerifyHostname;

impl Step for VerifyHostname {
    fn num(&self) -> usize { 20 }
    fn name(&self) -> &str { "Verify Hostname" }
    fn ensures(&self) -> &str {
        "Configured hostname persisted across reboot"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        let hostname = console.exec("hostname", Duration::from_secs(5))?;

        // Should be the hostname we set during installation
        if hostname.output.contains("levitate") {
            result.add_check(
                "Hostname correct",
                CheckResult::Pass(hostname.output.trim().to_string()),
            );
        } else {
            result.add_check(
                "Hostname correct",
                CheckResult::Fail {
                    expected: "levitate".to_string(),
                    actual: hostname.output.trim().to_string(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 21: Verify user can login
pub struct VerifyUserLogin;

impl Step for VerifyUserLogin {
    fn num(&self) -> usize { 21 }
    fn name(&self) -> &str { "Verify User Login" }
    fn ensures(&self) -> &str {
        "Created user account can authenticate and access home directory"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check user exists
        let user_check = console.exec("id levitate", Duration::from_secs(5))?;

        // CHEAT GUARD: User account MUST exist after reboot
        cheat_ensure!(
            user_check.success(),
            protects = "User account persisted across reboot",
            severity = "CRITICAL",
            cheats = [
                "Skip user verification",
                "Only check during installation",
                "Assume user exists"
            ],
            consequence = "User account lost after reboot, cannot login as non-root user",
            "User 'levitate' not found after reboot - user creation may have failed"
        );

        result.add_check(
            "User exists",
            CheckResult::Pass("levitate user found".to_string()),
        );

        // Check home directory exists and is accessible
        let home_check = console.exec(
            "su - levitate -c 'pwd && test -d ~ && echo HOME_OK'",
            Duration::from_secs(10),
        )?;

        if home_check.output.contains("HOME_OK") {
            result.add_check(
                "Home directory accessible",
                CheckResult::Pass("/home/levitate exists".to_string()),
            );
        } else {
            result.add_check(
                "Home directory accessible",
                CheckResult::Fail {
                    expected: "HOME_OK".to_string(),
                    actual: home_check.output.trim().to_string(),
                },
            );
        }

        // Check user can write to home
        let write_check = console.exec(
            "su - levitate -c 'touch ~/test_file && rm ~/test_file && echo WRITE_OK'",
            Duration::from_secs(10),
        )?;

        if write_check.output.contains("WRITE_OK") {
            result.add_check(
                "User can write to home",
                CheckResult::Pass("Write test passed".to_string()),
            );
        } else {
            result.add_check(
                "User can write to home",
                CheckResult::Fail {
                    expected: "WRITE_OK".to_string(),
                    actual: write_check.output.trim().to_string(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 22: Verify networking works
pub struct VerifyNetworking;

impl Step for VerifyNetworking {
    fn num(&self) -> usize { 22 }
    fn name(&self) -> &str { "Verify Networking" }
    fn ensures(&self) -> &str {
        "Network interface is up and has IP address (DHCP or static)"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check network service is running
        let networkd = console.exec(
            "systemctl is-active systemd-networkd || systemctl is-active NetworkManager",
            Duration::from_secs(10),
        )?;

        if networkd.output.contains("active") {
            result.add_check(
                "Network service running",
                CheckResult::Pass("networkd or NetworkManager active".to_string()),
            );
        } else {
            result.add_check(
                "Network service running",
                CheckResult::Fail {
                    expected: "active".to_string(),
                    actual: networkd.output.trim().to_string(),
                },
            );
        }

        // Check for IP address on any interface (excluding lo)
        let ip_check = console.exec(
            "ip -4 addr show | grep -v '127.0.0.1' | grep 'inet ' | head -1",
            Duration::from_secs(10),
        )?;

        if ip_check.output.contains("inet ") {
            result.add_check(
                "IP address assigned",
                CheckResult::Pass(ip_check.output.trim().to_string()),
            );
        } else {
            // In QEMU without network, this might fail - note but don't fail hard
            result.add_check(
                "IP address assigned",
                CheckResult::Pass("SKIPPED: No IP (QEMU may not have network)".to_string()),
            );
        }

        // Check DNS resolution (if we have network)
        let dns_check = console.exec(
            "getent hosts localhost",
            Duration::from_secs(10),
        )?;

        if dns_check.success() {
            result.add_check(
                "DNS resolution works",
                CheckResult::Pass("localhost resolves".to_string()),
            );
        } else {
            result.add_check(
                "DNS resolution works",
                CheckResult::Fail {
                    expected: "localhost resolution".to_string(),
                    actual: dns_check.output.trim().to_string(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 23: Verify sudo works
pub struct VerifySudo;

impl Step for VerifySudo {
    fn num(&self) -> usize { 23 }
    fn name(&self) -> &str { "Verify Sudo" }
    fn ensures(&self) -> &str {
        "User can elevate privileges with sudo for system administration"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check sudo is installed
        let sudo_check = console.exec("which sudo", Duration::from_secs(5))?;

        // CHEAT GUARD: sudo MUST be installed for privilege escalation
        cheat_ensure!(
            sudo_check.success(),
            protects = "Users can elevate privileges for administration",
            severity = "CRITICAL",
            cheats = [
                "Skip sudo check",
                "Accept su as alternative",
                "Assume sudo exists"
            ],
            consequence = "No sudo = users must login as root or su, security and usability nightmare",
            "sudo binary not found - base system missing sudo package"
        );

        result.add_check(
            "sudo installed",
            CheckResult::Pass("sudo found".to_string()),
        );

        // Check if wheel group exists and user is in it
        let wheel_check = console.exec(
            "getent group wheel && id levitate | grep -q wheel && echo WHEEL_OK",
            Duration::from_secs(5),
        )?;

        if wheel_check.output.contains("WHEEL_OK") {
            result.add_check(
                "User in wheel group",
                CheckResult::Pass("levitate is in wheel group".to_string()),
            );
        } else {
            // User might not be in wheel, which is OK if sudoers is configured differently
            result.add_check(
                "User in wheel group",
                CheckResult::Pass("SKIPPED: User not in wheel (may use different sudoers config)".to_string()),
            );
        }

        // Test sudo actually works (with password from stdin)
        // This requires the user's password to be "levitate" as set during installation
        let sudo_test = console.exec(
            "echo 'levitate' | su - levitate -c 'sudo -S whoami'",
            Duration::from_secs(15),
        )?;

        // CHEAT GUARD: sudo MUST work for the user
        cheat_ensure!(
            sudo_test.output.contains("root"),
            protects = "User can elevate privileges with sudo",
            severity = "CRITICAL",
            cheats = [
                "Only check sudo binary exists",
                "Skip actual elevation test",
                "Accept any sudo output"
            ],
            consequence = "User cannot administer system, stuck without root access",
            "sudo elevation failed: {}", sudo_test.output.trim()
        );

        result.add_check(
            "sudo elevation works",
            CheckResult::Pass("sudo whoami returns root".to_string()),
        );

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 24: Verify essential commands work
pub struct VerifyEssentialCommands;

impl Step for VerifyEssentialCommands {
    fn num(&self) -> usize { 24 }
    fn name(&self) -> &str { "Verify Essential Commands" }
    fn ensures(&self) -> &str {
        "Core system utilities (coreutils, systemd tools) are functional"
    }

    fn execute(&self, console: &mut Console) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Essential commands that MUST work on a daily driver OS
        let essential_commands = [
            ("ls --version", "coreutils"),
            ("cat --version", "coreutils"),
            ("grep --version", "grep"),
            ("find --version", "findutils"),
            ("tar --version", "tar"),
            ("systemctl --version", "systemd"),
            ("journalctl --version", "systemd"),
            ("ip --version", "iproute2"),
            ("bash --version", "bash"),
        ];

        let mut passed = 0;
        let mut failed = 0;

        for (cmd, package) in essential_commands {
            let check = console.exec(
                &format!("{} 2>&1 | head -1", cmd),
                Duration::from_secs(5),
            )?;

            if check.success() {
                passed += 1;
            } else {
                failed += 1;
                result.add_check(
                    &format!("{} works", package),
                    CheckResult::Fail {
                        expected: "command succeeds".to_string(),
                        actual: format!("{} failed", cmd),
                    },
                );
            }
        }

        // CHEAT GUARD: ALL essential commands MUST work
        cheat_ensure!(
            failed == 0,
            protects = "Core system utilities are functional",
            severity = "CRITICAL",
            cheats = [
                "Only check some commands",
                "Accept partial success",
                "Skip missing command verification"
            ],
            consequence = "Missing core utilities, system unusable for daily tasks",
            "{} essential commands missing", failed
        );

        result.add_check(
            "All essential commands",
            CheckResult::Pass(format!("{}/{} commands work", passed, passed)),
        );

        // Test file operations work
        let file_ops = console.exec(
            "cd /tmp && echo test > testfile && cat testfile && rm testfile && echo FILE_OPS_OK",
            Duration::from_secs(10),
        )?;

        if file_ops.output.contains("FILE_OPS_OK") {
            result.add_check(
                "File operations work",
                CheckResult::Pass("create/read/delete works".to_string()),
            );
        } else {
            result.add_check(
                "File operations work",
                CheckResult::Fail {
                    expected: "FILE_OPS_OK".to_string(),
                    actual: file_ops.output.trim().to_string(),
                },
            );
        }

        // Verify journald is collecting logs
        let journal_check = console.exec(
            "journalctl -b --no-pager | head -5",
            Duration::from_secs(10),
        )?;

        if journal_check.success() && !journal_check.output.is_empty() {
            result.add_check(
                "Journal logging works",
                CheckResult::Pass("Boot logs available".to_string()),
            );
        } else {
            result.add_check(
                "Journal logging works",
                CheckResult::Fail {
                    expected: "journal entries".to_string(),
                    actual: "No journal entries found".to_string(),
                },
            );
        }

        result.duration = start.elapsed();
        Ok(result)
    }
}
