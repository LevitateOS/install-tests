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
use crate::distro::DistroContext;
use crate::executor::Executor;
use anyhow::Result;
use leviso_cheat_guard::cheat_ensure;
use std::time::{Duration, Instant};

/// Step 19: Verify systemd started successfully
pub struct VerifySystemdBoot;

impl Step for VerifySystemdBoot {
    fn num(&self) -> usize {
        19
    }
    fn name(&self) -> &str {
        "Verify Systemd Boot"
    }
    fn ensures(&self) -> &str {
        "Installed system boots to multi-user target with systemd running"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // ═══════════════════════════════════════════════════════════════════════
        // INFRASTRUCTURE CANARY: Verify we're on installed system, not live ISO
        // ═══════════════════════════════════════════════════════════════════════

        // Check root filesystem type - should NOT be overlay (live ISO uses overlay)
        let fstype = executor.exec("findmnt / -o FSTYPE -n", Duration::from_secs(5))?;

        // ANTI-CHEAT: Root must not be overlay - that would mean we're still on live ISO
        cheat_ensure!(
            !fstype.output.contains("overlay"),
            protects = "Verification runs on installed system, not live ISO",
            severity = "CRITICAL",
            cheats = [
                "Never reboot after install",
                "Skip Phase 6 entirely",
                "Verify on live ISO overlay"
            ],
            consequence = "Installation success faked, real rootfs never tested",
            "Root is overlay ({}) - still on live ISO, not installed system!",
            fstype.output.trim()
        );

        result.add_check(
            "Root is not overlay",
            CheckResult::pass(format!(
                "root fstype={} (installed, not live)",
                fstype.output.trim()
            )),
        );

        // Flush any pending output from login
        let _ = executor.exec("true", Duration::from_secs(2))?;

        // Check expected init is running (PID 1)
        let expected_pid1 = ctx.expected_pid1_name();
        let pid1 = executor.exec("cat /proc/1/comm", Duration::from_secs(5))?;

        // CHEAT GUARD: Expected init MUST be PID 1 for proper boot
        cheat_ensure!(
            pid1.output.contains(expected_pid1),
            protects = "System booted with expected init system",
            severity = "CRITICAL",
            cheats = [
                "Skip PID 1 check",
                "Accept any init system",
                "Assume init is running"
            ],
            consequence = "System didn't boot properly, may be in emergency shell or wrong init",
            "PID 1 is '{}', expected '{}'",
            pid1.output.trim(),
            expected_pid1
        );

        result.add_check(
            &format!("{} is PID 1", expected_pid1),
            CheckResult::pass(format!("/proc/1/comm = {}", expected_pid1)),
        );

        // Check we reached boot target using distro-specific command
        let target_cmd = ctx.check_target_reached_cmd();
        let target_expected = ctx.target_reached_expected();
        let target = executor.exec(target_cmd, Duration::from_secs(10))?;

        if target.output.contains(target_expected) {
            result.add_check(
                "boot target reached",
                CheckResult::pass(format!("{} target active", ctx.id())),
            );
        } else {
            result.add_check(
                "boot target reached",
                CheckResult::Fail {
                    expected: target_expected.to_string(),
                    actual: target.output.trim().to_string(),
                },
            );
        }

        // Check for failed units/services using distro-specific command
        let failed_cmd = ctx.count_failed_services_cmd();
        let failed = executor.exec(failed_cmd, Duration::from_secs(10))?;

        let failed_count: i32 = failed
            .output
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .next()
            .unwrap_or(0);

        if failed_count == 0 {
            result.add_check("No failed services", CheckResult::pass("0 failed services"));
        } else {
            // Get the list of failed services
            let failed_list_cmd = ctx.list_failed_services_cmd();
            let failed_list = executor.exec(&failed_list_cmd, Duration::from_secs(5))?;
            result.add_check(
                "Failed services",
                CheckResult::Fail {
                    expected: "0 failed services".to_string(),
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
    fn num(&self) -> usize {
        20
    }
    fn name(&self) -> &str {
        "Verify Hostname"
    }
    fn ensures(&self) -> &str {
        "Configured hostname persisted across reboot"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        let hostname = executor.exec("hostname", Duration::from_secs(5))?;
        let expected_pattern = ctx.hostname_check_pattern();

        // Should contain the hostname pattern we set during installation
        if hostname.output.contains(expected_pattern) {
            result.add_check(
                "Hostname correct",
                CheckResult::pass(hostname.output.trim()),
            );
        } else {
            result.add_check(
                "Hostname correct",
                CheckResult::Fail {
                    expected: format!("contains '{}'", expected_pattern),
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
    fn num(&self) -> usize {
        21
    }
    fn name(&self) -> &str {
        "Verify User Login"
    }
    fn ensures(&self) -> &str {
        "Created user account can authenticate and access home directory"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check user exists
        let username = ctx.default_username();
        let user_check = executor.exec(&format!("id {}", username), Duration::from_secs(5))?;

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
            "User '{}' not found after reboot - user creation may have failed",
            username
        );

        result.add_check("User exists", CheckResult::pass(user_check.output.trim()));

        // Check home directory exists and is accessible
        let home_check = executor.exec(
            &format!("su - {} -c 'pwd && test -d ~ && echo HOME_OK'", username),
            Duration::from_secs(10),
        )?;

        if home_check.output.contains("HOME_OK") {
            result.add_check(
                "Home directory accessible",
                CheckResult::pass(format!("/home/{} accessible", username)),
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
        let write_check = executor.exec(
            &format!(
                "su - {} -c 'touch ~/test_file && rm ~/test_file && echo WRITE_OK'",
                username
            ),
            Duration::from_secs(10),
        )?;

        if write_check.output.contains("WRITE_OK") {
            result.add_check(
                "User can write to home",
                CheckResult::pass("touch+rm ~/test_file succeeded"),
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
    fn num(&self) -> usize {
        22
    }
    fn name(&self) -> &str {
        "Verify Networking"
    }
    fn ensures(&self) -> &str {
        "Network interface is up and has IP address (DHCP or static)"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check network service is running using distro-specific command
        let network_cmd = ctx.check_network_service_cmd();
        let networkd = executor.exec(network_cmd, Duration::from_secs(10))?;

        if networkd.output.contains("active") {
            result.add_check(
                "Network service running",
                CheckResult::pass("network service active"),
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
        // QEMU user-mode networking is now enabled, so this MUST work
        let ip_check = executor.exec(
            "ip -4 addr show | grep -v '127.0.0.1' | grep 'inet ' | head -1",
            Duration::from_secs(10),
        )?;

        // ANTI-CHEAT: IP address is now required since we enable QEMU user network
        cheat_ensure!(
            ip_check.output.contains("inet "),
            protects = "Network interface has IP address",
            severity = "HIGH",
            cheats = [
                "Run without QEMU network",
                "Skip network verification",
                "Convert to optional Skip"
            ],
            consequence =
                "No network = can't install packages, can't reach internet on daily driver",
            "No IP address assigned. QEMU user network should provide DHCP. Output: {}",
            ip_check.output.trim()
        );

        result.add_check(
            "IP address assigned",
            CheckResult::pass(ip_check.output.trim()),
        );

        // Check DNS resolution (if we have network)
        let dns_check = executor.exec("getent hosts localhost", Duration::from_secs(10))?;

        if dns_check.success() {
            result.add_check(
                "DNS resolution works",
                CheckResult::pass(dns_check.output.trim()),
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
    fn num(&self) -> usize {
        23
    }
    fn name(&self) -> &str {
        "Verify Sudo"
    }
    fn ensures(&self) -> &str {
        "User can elevate privileges with sudo for system administration"
    }

    fn execute(&self, executor: &mut dyn Executor, ctx: &dyn DistroContext) -> Result<StepResult> {
        let start = Instant::now();
        let mut result = StepResult::new(self.num(), self.name());

        // Check sudo is installed
        let sudo_check = executor.exec("which sudo", Duration::from_secs(5))?;

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
            consequence =
                "No sudo = users must login as root or su, security and usability nightmare",
            "sudo binary not found - base system missing sudo package"
        );

        result.add_check(
            "sudo installed",
            CheckResult::pass(sudo_check.output.trim()),
        );

        // Check if wheel group exists and user is in it
        // This is the standard sudo configuration on most Linux systems
        let username = ctx.default_username();
        let wheel_check = executor.exec(
            &format!(
                "getent group wheel && id {} | grep -q wheel && echo WHEEL_OK",
                username
            ),
            Duration::from_secs(5),
        )?;

        // ANTI-CHEAT: User MUST be in wheel group for sudo to work
        cheat_ensure!(
            wheel_check.output.contains("WHEEL_OK"),
            protects = "User is in wheel group for sudo access",
            severity = "HIGH",
            cheats = [
                "Accept any sudoers configuration",
                "Skip wheel group check",
                "Convert to optional"
            ],
            consequence = "User not in wheel = sudo doesn't work = can't administer system",
            "User '{}' not in wheel group. Output: {}",
            username,
            wheel_check.output.trim()
        );

        result.add_check(
            "User in wheel group",
            CheckResult::pass(format!("{} in wheel group", username)),
        );

        // Test sudo actually works (with password from stdin)
        let password = ctx.default_password();
        let sudo_test = executor.exec(
            &format!(
                "echo '{}' | su - {} -c 'sudo -S whoami'",
                password, username
            ),
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
            "sudo elevation failed: {}",
            sudo_test.output.trim()
        );

        result.add_check(
            "sudo elevation works",
            CheckResult::pass("sudo whoami returned 'root'"),
        );

        result.duration = start.elapsed();
        Ok(result)
    }
}

/// Step 24: Verify essential commands work
pub struct VerifyEssentialCommands;

impl Step for VerifyEssentialCommands {
    fn num(&self) -> usize {
        24
    }
    fn name(&self) -> &str {
        "Verify Essential Commands"
    }
    fn ensures(&self) -> &str {
        "Core system utilities (coreutils, systemd tools) are functional"
    }

    fn execute(&self, executor: &mut dyn Executor, _ctx: &dyn DistroContext) -> Result<StepResult> {
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

        let mut failed = 0;

        for (cmd, package) in essential_commands {
            let check =
                executor.exec(&format!("{} 2>&1 | head -1", cmd), Duration::from_secs(5))?;

            if !check.success() {
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
            "{} essential commands missing",
            failed
        );

        result.add_check(
            "All essential commands",
            CheckResult::pass("9/9 commands working"),
        );

        // Test file operations work
        let file_ops = executor.exec(
            "cd /tmp && echo test > testfile && cat testfile && rm testfile && echo FILE_OPS_OK",
            Duration::from_secs(10),
        )?;

        if file_ops.output.contains("FILE_OPS_OK") {
            result.add_check(
                "File operations work",
                CheckResult::pass("echo+cat+rm in /tmp succeeded"),
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
        let journal_check = executor.exec(
            "journalctl -b --no-pager | head -5",
            Duration::from_secs(10),
        )?;

        if journal_check.success() && !journal_check.output.is_empty() {
            result.add_check(
                "Journal logging works",
                CheckResult::pass("journalctl -b shows entries"),
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
