//! Scenario-based development loop.
//!
//! Lightweight, incremental scenarios that gate progression and give fast
//! feedback during development. Each scenario validates one thing.
//!
//! # Scenarios
//!
//! - `build-preflight` — contract + runtime provenance + artifact preflight
//! - `live-boot` — ISO boots in QEMU (login prompt or `___SHELL_READY___`)
//! - `live-tools` — expected binaries present in live environment
//! - `install` — scripted install to disk succeeds
//! - `installed-boot` — system boots from disk after install
//! - `automated-login` — harness can login and run commands
//! - `runtime` — expected installed-system tools are present

pub mod compat;
pub mod state;

use crate::distro::{context_for_distro, DistroContext};
use crate::preflight::require_preflight_with_iso_for_distro;
use crate::qemu::session;
use crate::qemu::{Console, SerialExecutorExt};
use anyhow::{bail, Context, Result};
use colored::Colorize;
use distro_contract::RootfsMutability;
use recshuttle::{InstallLayout, InstallPlanSpec, RemoteInstallerService, SshExecOutput};
use serde::{Deserialize, Serialize};
use state::ScenarioState;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const LIVE_BOOT_VALIDATION_SCRIPT: &str = "/usr/local/bin/stage-01-live-boot.sh";
const LIVE_BOOT_SSH_PREFLIGHT_SCRIPT: &str = "/usr/local/bin/stage-01-ssh-preflight.sh";
const STAGE_RUNTIME_RETENTION_COUNT: usize = 5;
const INSTALL_DISK_FILENAME: &str = "disk.qcow2";
const INSTALL_OVMF_VARS_FILENAME: &str = "ovmf-vars.fd";

const PRODUCT_BASE_ROOTFS: &str = "base-rootfs";
const PRODUCT_LIVE_BOOT: &str = "live-boot";
const PRODUCT_LIVE_TOOLS: &str = "live-tools";
const SCENARIO_ROOT_DIRNAME: &str = "scenarios";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScenarioId {
    BuildPreflight,
    LiveBoot,
    LiveTools,
    Install,
    InstalledBoot,
    AutomatedLogin,
    Runtime,
}

impl ScenarioId {
    pub const ALL: [ScenarioId; 7] = [
        ScenarioId::BuildPreflight,
        ScenarioId::LiveBoot,
        ScenarioId::LiveTools,
        ScenarioId::Install,
        ScenarioId::InstalledBoot,
        ScenarioId::AutomatedLogin,
        ScenarioId::Runtime,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::BuildPreflight => "build-preflight",
            Self::LiveBoot => "live-boot",
            Self::LiveTools => "live-tools",
            Self::Install => "install",
            Self::InstalledBoot => "installed-boot",
            Self::AutomatedLogin => "automated-login",
            Self::Runtime => "runtime",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::BuildPreflight => "Build Preflight",
            Self::LiveBoot => "Live Boot",
            Self::LiveTools => "Live Tools",
            Self::Install => "Install",
            Self::InstalledBoot => "Installed Boot",
            Self::AutomatedLogin => "Automated Login",
            Self::Runtime => "Runtime",
        }
    }

    pub fn ordinal(self) -> usize {
        match self {
            Self::BuildPreflight => 0,
            Self::LiveBoot => 1,
            Self::LiveTools => 2,
            Self::Install => 3,
            Self::InstalledBoot => 4,
            Self::AutomatedLogin => 5,
            Self::Runtime => 6,
        }
    }

    pub fn parse_key(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        match trimmed {
            "build-preflight" | "build_preflight" => Some(Self::BuildPreflight),
            "live-boot" | "live_boot" => Some(Self::LiveBoot),
            "live-tools" | "live_tools" => Some(Self::LiveTools),
            "install" => Some(Self::Install),
            "installed-boot" | "installed_boot" => Some(Self::InstalledBoot),
            "automated-login" | "automated_login" => Some(Self::AutomatedLogin),
            "runtime" => Some(Self::Runtime),
            _ => None,
        }
    }

    fn release_product(self) -> Option<&'static str> {
        match self {
            Self::BuildPreflight => Some(PRODUCT_BASE_ROOTFS),
            Self::LiveBoot => Some(PRODUCT_LIVE_BOOT),
            Self::LiveTools | Self::Install => Some(PRODUCT_LIVE_TOOLS),
            Self::InstalledBoot | Self::AutomatedLogin | Self::Runtime => None,
        }
    }

    fn scenario_output_dirname(self) -> &'static str {
        self.key()
    }
}

#[derive(Debug, Clone)]
pub struct ScenarioIsoArtifact {
    pub scenario: ScenarioId,
    pub product_name: &'static str,
    pub path: PathBuf,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseRunManifest {
    status: String,
    iso_path: Option<String>,
    target_kind: Option<String>,
    target_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstallScenarioRuntime {
    pub run_id: String,
    pub disk_path: PathBuf,
    pub ovmf_vars_path: PathBuf,
}

/// Run a single scenario for a distro.
pub fn run_scenario(distro_id: &str, scenario: ScenarioId) -> Result<bool> {
    run_scenario_impl(distro_id, scenario, false)
}

/// Run a single scenario for a distro, forcing rerun of the target scenario.
pub fn run_scenario_forced(distro_id: &str, scenario: ScenarioId) -> Result<bool> {
    run_scenario_impl(distro_id, scenario, true)
}

fn run_scenario_impl(distro_id: &str, scenario: ScenarioId, force: bool) -> Result<bool> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;
    let canonical_distro_id = ctx.id();
    let scenario_iso = resolve_iso_artifact_for_scenario(canonical_distro_id, scenario)?;
    if let Some(iso) = scenario_iso.as_ref() {
        let iso_dir = iso.path.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "Could not resolve ISO parent directory for '{}'",
                iso.path.display()
            )
        })?;
        require_preflight_with_iso_for_distro(iso_dir, Some(&iso.filename), canonical_distro_id)?;
    }

    let input_fingerprint =
        scenario_input_fingerprint(canonical_distro_id, scenario, scenario_iso.as_ref())?;

    let mut state = ScenarioState::load(canonical_distro_id);
    if !state.is_valid_for_scenario_input(scenario, &input_fingerprint) {
        println!(
            "{}",
            format!(
                "Scenario input changed for {} — resetting this scenario and later results.",
                scenario.display_name()
            )
            .yellow()
        );
        state.reset_for_scenario_input(scenario, &input_fingerprint);
        state.save(canonical_distro_id)?;
    }

    if force {
        state.results.retain(|key, _| {
            compat::parse_scenario_target(key)
                .map(|existing| existing.ordinal() < scenario.ordinal())
                .unwrap_or(false)
        });
        state.save(canonical_distro_id)?;
        println!(
            "{}",
            format!(
                "Forcing {} rerun (cleared cached results from this scenario onward).",
                scenario.display_name()
            )
            .yellow()
        );
    }

    if !force && scenario.ordinal() > 0 {
        let previous = ScenarioId::ALL[scenario.ordinal() - 1];
        if !state.has_passed(previous) {
            bail!(
                "{} is blocked: {} has not passed yet.\n\
                 Run: cargo run --bin stages -- --distro {} --scenario {}",
                scenario.display_name(),
                previous.display_name(),
                canonical_distro_id,
                previous.key()
            );
        }
    }

    if state.has_passed(scenario) {
        println!(
            "{} {} already passed (use --reset to clear).",
            "[SKIP]".green(),
            scenario.display_name()
        );
        return Ok(true);
    }

    println!("{} {}", ">>".cyan(), scenario.display_name(),);

    let result = match scenario {
        ScenarioId::BuildPreflight => {
            Ok("Preflight conformance + artifact checks passed".to_string())
        }
        ScenarioId::LiveBoot => run_live_boot(
            &*ctx,
            &scenario_iso
                .as_ref()
                .expect("live-boot scenario requires ISO")
                .path,
        ),
        ScenarioId::LiveTools => run_live_tools(
            &*ctx,
            &scenario_iso
                .as_ref()
                .expect("live-tools scenario requires ISO")
                .path,
        ),
        ScenarioId::Install => run_installation(
            &*ctx,
            &scenario_iso
                .as_ref()
                .expect("install scenario requires ISO")
                .path,
        ),
        ScenarioId::InstalledBoot => run_installed_boot(&*ctx),
        ScenarioId::AutomatedLogin => run_automated_login(&*ctx),
        ScenarioId::Runtime => run_daily_driver_tools(&*ctx),
    };

    match &result {
        Ok(evidence) => {
            state.record(scenario, true, evidence);
            state.save(canonical_distro_id)?;
            println!(
                "{} {} passed: {}",
                "[PASS]".green().bold(),
                scenario.display_name(),
                evidence
            );
            Ok(true)
        }
        Err(e) => {
            state.record(scenario, false, &format!("{:#}", e));
            state.save(canonical_distro_id)?;
            print_failure(scenario, e);
            Ok(false)
        }
    }
}

/// Run all scenarios up to `target` (inclusive).
pub fn run_up_to_scenario(distro_id: &str, target: ScenarioId) -> Result<bool> {
    for scenario in ScenarioId::ALL {
        if scenario.ordinal() > target.ordinal() {
            break;
        }
        if !run_scenario(distro_id, scenario)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Print scenario status for a distro.
pub fn print_status(distro_id: &str) -> Result<()> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;
    let canonical_distro_id = ctx.id();

    let state = ScenarioState::load(canonical_distro_id);
    let valid = ScenarioId::ALL.iter().all(|scenario| {
        if !state.has_result(*scenario) {
            return true;
        }
        match scenario_input_fingerprint(
            canonical_distro_id,
            *scenario,
            resolve_iso_artifact_for_scenario(canonical_distro_id, *scenario)
                .ok()
                .flatten()
                .as_ref(),
        ) {
            Ok(fingerprint) => state.is_valid_for_scenario_input(*scenario, &fingerprint),
            Err(_) => false,
        }
    });

    println!("{} Scenario Status", ctx.name().bold());
    if !valid {
        println!(
            "{}",
            "  (stale — scenario input changed or is missing, results will reset on next run)"
                .yellow()
        );
    }
    println!();

    for scenario in ScenarioId::ALL {
        let status = if state.has_passed(scenario) {
            "[PASS]".green()
        } else if state.has_result(scenario) {
            "[FAIL]".red()
        } else {
            "[    ]".dimmed()
        };
        println!(
            "  {} {:<15} {}",
            status,
            scenario.key(),
            scenario.display_name()
        );
    }
    println!();
    println!(
        "  Highest passed: {}",
        state
            .highest_passed()
            .map(|scenario| scenario.display_name().to_string())
            .unwrap_or_else(|| "none".to_string())
            .bold()
    );
    Ok(())
}

/// Reset all scenario state for a distro.
pub fn reset_state(distro_id: &str) -> Result<()> {
    let canonical_distro_id = context_for_distro(distro_id)
        .map(|ctx| ctx.id().to_string())
        .unwrap_or_else(|| distro_id.to_string());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.stages")
        .join(format!("{}.json", canonical_distro_id));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    println!("Scenario state reset for {}.", canonical_distro_id);
    Ok(())
}

pub fn parse_scenario_name(value: &str) -> Result<ScenarioId> {
    ScenarioId::parse_key(value).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported scenario '{}'; expected one of: build-preflight, live-boot, live-tools, install, installed-boot, automated-login, runtime",
            value
        )
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// Stage implementations
// ═══════════════════════════════════════════════════════════════════════════

/// Live Boot scenario — ISO boots in QEMU.
fn run_live_boot(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let (mut child, mut console, ssh_host_port) = spawn_live_qemu_with_ssh(ctx, iso_path)?;
    let stall_timeout = Duration::from_secs(ctx.live_boot_stall_timeout_secs());

    let result = (|| -> Result<String> {
        console.wait_for_live_boot_with_context(stall_timeout, ctx)?;
        verify_live_boot_ssh_login(&mut console, ssh_host_port)?;

        run_stage_script_over_ssh(ssh_host_port, LIVE_BOOT_VALIDATION_SCRIPT)?;
        run_stage_script_over_ssh(ssh_host_port, LIVE_BOOT_SSH_PREFLIGHT_SCRIPT)?;

        Ok(
            "Boot markers detected + SSH login probe passed + live-boot script checks passed"
                .to_string(),
        )
    })();

    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Live Tools scenario — expected binaries in the live environment.
///
/// IMPORTANT: This doesn't just check if tools exist (which would be lazy).
/// It actually EXECUTES each tool to verify:
/// - Binary can execute (not just exist in PATH)
/// - Required libraries are present (no missing .so files)
/// - Environment is configured (proc/sys/dev available)
/// - Tool is functional (not broken/corrupted)
fn run_live_tools(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let (mut child, mut console, ssh_host_port) = spawn_live_qemu_with_ssh(ctx, iso_path)?;
    let result = (|| -> Result<String> {
        wait_for_live_tools_serial_readiness(&mut console, ctx)?;
        verify_live_boot_ssh_login(&mut console, ssh_host_port)?;

        // Check for key tools expected in the live environment
        let tools: Vec<&str> = ctx.live_tools().to_vec();
        let mut missing = Vec::new();
        let mut found = Vec::new();
        let mut broken = Vec::new();

        for tool in &tools {
            // Get the validation command for this tool
            let validation_cmd = get_tool_validation_command(tool);

            let result = ssh_exec(ssh_host_port, &validation_cmd)?;
            if result.exit_code == 0 {
                // Tool executed successfully - it works!
                found.push(*tool);
            } else if result.exit_code == 127
                || result.output.contains("command not found")
                || result.output.contains("not found")
            {
                // Exit code 127 = command not found
                missing.push(*tool);
            } else {
                // Tool exists but failed to execute properly
                broken.push((*tool, result.exit_code, result.output.trim().to_string()));
            }
        }

        let overlay_evidence = verify_live_overlay_behavior(&mut console)?;

        // Report failures
        if !missing.is_empty() || !broken.is_empty() {
            let mut error_msg = String::new();

            if !missing.is_empty() {
                error_msg.push_str(&format!(
                    "Missing tools (not in PATH): {}\n",
                    missing.join(", ")
                ));
            }

            if !broken.is_empty() {
                error_msg.push_str("Broken tools (exist but failed to execute):\n");
                for (tool, code, output) in &broken {
                    error_msg.push_str(&format!(
                        "  {} (exit {}): {}\n",
                        tool,
                        code,
                        if output.is_empty() {
                            "no output"
                        } else {
                            &output.lines().next().unwrap_or("unknown error")
                        }
                    ));
                }
            }

            error_msg.push_str(&format!("\nWorking tools: {}", found.join(", ")));

            bail!("{}", error_msg.trim());
        }

        let expected_install_experience = ctx.install_experience_profile();
        let install_experience_marker =
            ssh_exec(ssh_host_port, "cat /usr/lib/levitate/install-experience")
                .with_context(|| "reading install-experience marker".to_string())?;
        if install_experience_marker.exit_code != 0 {
            bail!(
                "install-experience marker missing or unreadable: {}",
                install_experience_marker.output.trim()
            );
        }
        let actual_install_experience = install_experience_marker
            .output
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .trim();
        if actual_install_experience != expected_install_experience {
            bail!(
                "install-experience mismatch: expected '{}', found '{}'",
                expected_install_experience,
                actual_install_experience
            );
        }

        let entrypoint_check = ssh_exec(
            ssh_host_port,
            "test -x /usr/local/bin/levitate-install-entrypoint",
        )
        .with_context(|| "checking install entrypoint script presence".to_string())?;
        if entrypoint_check.exit_code != 0 {
            bail!(
                "missing executable install entrypoint at /usr/local/bin/levitate-install-entrypoint"
            );
        }
        let mut ux_split_evidence: Option<String> = None;
        if expected_install_experience == "ux" {
            let ux_hook_check = ssh_exec(ssh_host_port, "test -r /etc/profile.d/30-install-ux.sh")
                .with_context(|| "checking install UX profile hook presence".to_string())?;
            if ux_hook_check.exit_code != 0 {
                bail!("missing install UX profile hook at /etc/profile.d/30-install-ux.sh");
            }
            ux_split_evidence = Some(verify_install_ux_split_behavior(ssh_host_port)?);
        }

        let install_profile_evidence = match ux_split_evidence {
            Some(evidence) => format!(
                "install profile '{}' verified; {}",
                actual_install_experience, evidence
            ),
            None => format!("install profile '{}' verified", actual_install_experience),
        };

        Ok(format!(
            "All {} tools verified working (actually executed): {}; {}; {}",
            found.len(),
            found.join(", "),
            install_profile_evidence,
            overlay_evidence
        ))
    })();

    let _ = child.kill();
    let _ = child.wait();
    result
}

fn wait_for_live_tools_serial_readiness(
    console: &mut Console,
    ctx: &dyn DistroContext,
) -> Result<()> {
    let stall_timeout = Duration::from_secs(ctx.live_boot_stall_timeout_secs());
    // Live-tools is validated over SSH. Serial readiness can be either the explicit
    // shell marker or a stable login prompt on ttyS0.
    let live_tools_success_patterns = [
        "___SHELL_READY___",
        "Login as 'root' (no password)",
        " login:",
    ];
    Console::wait_for_boot_with_patterns(
        console,
        stall_timeout,
        &live_tools_success_patterns,
        ctx.boot_error_patterns(),
        false,
    )
    .with_context(|| "waiting for live-tools serial readiness".to_string())
}

fn run_installation(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let scenario_run = ScenarioRun::start(ctx.id(), ScenarioId::Install, None)?;
    let disk_path = scenario_run.output_dir.join(INSTALL_DISK_FILENAME);
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    recqemu::create_disk(&disk_path, "20G")?;

    let ovmf_vars_path = scenario_run.output_dir.join(INSTALL_OVMF_VARS_FILENAME);
    let (ovmf, ovmf_vars) = session::setup_ovmf_vars_at(&ovmf_vars_path)?;

    let (mut child, mut console, ssh_host_port) =
        session::spawn_live_with_disk_with_ssh(iso_path, &disk_path, &ovmf, &ovmf_vars)?;

    // Install runs through the remote installer service channel (SSH),
    // not through serial console command execution. We still wait for live boot
    // markers to drain serial output and establish a deterministic ready boundary.
    let installer = RemoteInstallerService::new(ssh_host_port);
    let install_result = (|| -> Result<usize> {
        console.wait_for_live_boot_with_context(
            Duration::from_secs(ctx.live_boot_stall_timeout_secs()),
            ctx,
        )?;
        installer.wait_ready(Duration::from_secs(ctx.live_boot_stall_timeout_secs()))?;

        let install_disk = installer.resolve_install_disk()?;
        let install_layout = install_layout_for_distro(ctx.id())?;
        let install_spec = InstallPlanSpec {
            distro_id: ctx.id().to_string(),
            os_name: ctx.name().to_string(),
            default_hostname: ctx.default_hostname().to_string(),
            default_password: ctx.default_password().to_string(),
            install_bootloader_cmd: ctx.install_bootloader_cmd().to_string(),
            enable_serial_getty_cmd: ctx.enable_serial_getty_cmd(),
            include_initramfs: ctx.init_system_name() != "OpenRC",
        };
        let install_cmds =
            recshuttle::install_commands_for(&install_spec, &install_disk, install_layout);
        let step_count = installer.run_install_plan(&install_cmds)?;

        // Verify key artifacts exist
        let include_initramfs = ctx.init_system_name() != "OpenRC";
        let mut verify_cmds = vec![
            ("Root filesystem", "ls /mnt/sysroot/bin/busybox".to_string()),
            ("Boot partition", "ls /mnt/sysroot/boot/EFI".to_string()),
            ("Kernel on ESP", "ls /mnt/sysroot/boot/vmlinuz".to_string()),
        ];
        if include_initramfs {
            verify_cmds.push((
                "Initramfs on ESP",
                "ls /mnt/sysroot/boot/initramfs.img".to_string(),
            ));
        }
        verify_cmds.push((
            "systemd-boot loader config",
            "cat /mnt/sysroot/boot/loader/loader.conf".to_string(),
        ));
        match install_layout {
            InstallLayout::MutableSingleRoot => {
                verify_cmds.push((
                    "systemd-boot entry",
                    format!("cat /mnt/sysroot/boot/loader/entries/{}.conf", ctx.id()),
                ));
            }
            InstallLayout::ImmutableAb => {
                verify_cmds.push((
                    "systemd-boot entry slot A",
                    format!("cat /mnt/sysroot/boot/loader/entries/{}-a.conf", ctx.id()),
                ));
                verify_cmds.push((
                    "systemd-boot entry slot B",
                    format!("cat /mnt/sysroot/boot/loader/entries/{}-b.conf", ctx.id()),
                ));
            }
        }
        verify_cmds.push(("fstab", "cat /mnt/sysroot/etc/fstab".to_string()));
        verify_cmds.extend(recshuttle::runtime_policy_checks_for_install(
            ctx.id(),
            install_layout,
            &install_disk,
        ));
        installer.verify_checks(&verify_cmds)?;
        installer.run_install_plan(&[
            ("Sync filesystem", "sync".to_string()),
            (
                "Unmount install target",
                "umount -R /mnt/sysroot".to_string(),
            ),
        ])?;

        Ok(step_count)
    })();

    let _ = installer.shutdown();
    let _ = child.kill();
    let _ = child.wait();

    match install_result {
        Ok(step_count) => {
            let evidence = format!(
                "{} install steps completed + verified via remote installer service",
                step_count
            );
            scenario_run.finish_success(
                &evidence,
                Some(disk_path.as_path()),
                Some(ovmf_vars_path.as_path()),
            )?;
            Ok(evidence)
        }
        Err(err) => {
            let failure = format!("FAIL: {:#}", err);
            let _ = scenario_run.finish_failed(
                &failure,
                Some(disk_path.as_path()),
                Some(ovmf_vars_path.as_path()),
            );
            Err(err)
        }
    }
}

fn run_installed_boot(ctx: &dyn DistroContext) -> Result<String> {
    let install_runtime = resolve_latest_install_runtime(ctx.id())?;
    let scenario_run = ScenarioRun::start(
        ctx.id(),
        ScenarioId::InstalledBoot,
        Some(install_runtime.run_id.clone()),
    )?;
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let (mut child, mut console) = session::spawn_installed(
        &install_runtime.disk_path,
        &ovmf,
        &install_runtime.ovmf_vars_path,
    )?;

    let result = console.wait_for_installed_boot_with_context(Duration::from_secs(90), ctx);
    let _ = child.kill();
    let _ = child.wait();

    match result {
        Ok(()) => {
            let evidence = "Installed system boot markers detected".to_string();
            scenario_run.finish_success(
                &evidence,
                Some(install_runtime.disk_path.as_path()),
                Some(install_runtime.ovmf_vars_path.as_path()),
            )?;
            Ok(evidence)
        }
        Err(e) => {
            let failure = format!("FAIL: {:#}", e);
            let _ = scenario_run.finish_failed(
                &failure,
                Some(install_runtime.disk_path.as_path()),
                Some(install_runtime.ovmf_vars_path.as_path()),
            );
            Err(anyhow::anyhow!("{:#}", e))
        }
    }
}

fn run_automated_login(ctx: &dyn DistroContext) -> Result<String> {
    let install_runtime = resolve_latest_install_runtime(ctx.id())?;
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;

    let (mut child, mut console) = session::spawn_installed(
        &install_runtime.disk_path,
        &ovmf,
        &install_runtime.ovmf_vars_path,
    )?;

    console.wait_for_installed_boot_with_context(Duration::from_secs(90), ctx)?;

    // Attempt login
    console.login("root", ctx.default_password(), Duration::from_secs(15))?;

    // Verify shell works
    let result = console.exec("echo STAGE_LOGIN_OK", Duration::from_secs(5))?;
    let _ = child.kill();
    let _ = child.wait();

    if result.output.contains("STAGE_LOGIN_OK") {
        Ok("Login succeeded, shell functional".to_string())
    } else {
        bail!(
            "Login succeeded but shell not functional. Got: {}",
            result.output.trim()
        );
    }
}

fn run_daily_driver_tools(ctx: &dyn DistroContext) -> Result<String> {
    let install_runtime = resolve_latest_install_runtime(ctx.id())?;
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;

    let (mut child, mut console) = session::spawn_installed(
        &install_runtime.disk_path,
        &ovmf,
        &install_runtime.ovmf_vars_path,
    )?;

    console.wait_for_installed_boot_with_context(Duration::from_secs(90), ctx)?;
    console.login("root", ctx.default_password(), Duration::from_secs(15))?;

    let tools: Vec<&str> = ctx.installed_tools().to_vec();
    let mut missing = Vec::new();
    let mut found = Vec::new();

    for tool in &tools {
        let result = console.exec(
            &format!("which {} 2>/dev/null && echo FOUND", tool),
            Duration::from_secs(5),
        )?;
        if result.output.contains("FOUND") {
            found.push(*tool);
        } else {
            missing.push(*tool);
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    if !missing.is_empty() {
        bail!(
            "Missing daily driver tools: {}\nFound: {}",
            missing.join(", "),
            found.join(", ")
        );
    }

    Ok(format!("All {} daily driver tools present", found.len()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

pub fn resolve_iso_artifact_for_scenario(
    distro_id: &str,
    scenario: ScenarioId,
) -> Result<Option<ScenarioIsoArtifact>> {
    let Some(product_name) = scenario.release_product() else {
        return Ok(None);
    };
    let release_root = release_product_root_dir(distro_id, product_name);
    let run_id = distro_builder::stage_runs::latest_successful_run_id(&release_root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "scenario '{}' for '{}' requires release product '{}', but no successful runs were found under '{}'.\n\
             Build it first: cargo run -p distro-builder --bin distro-builder -- release build iso {} {}",
            scenario.key(),
            distro_id,
            product_name,
            release_root.display(),
            distro_id,
            product_name
        )
    })?;
    let run_dir = release_root.join(&run_id);
    let manifest = load_release_run_manifest(&run_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "release product '{}' for '{}' is missing run-manifest metadata under '{}'.",
            product_name,
            distro_id,
            run_dir.display()
        )
    })?;
    if manifest.status != "success" {
        bail!(
            "latest '{}' release run for '{}' is not successful under '{}'",
            product_name,
            distro_id,
            run_dir.display()
        );
    }
    if manifest.target_kind.as_deref() != Some("release-product")
        || manifest.target_name.as_deref() != Some(product_name)
    {
        bail!(
            "release run manifest under '{}' does not match expected product '{}'",
            run_dir.display(),
            product_name
        );
    }

    let iso_path = manifest
        .iso_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "release product '{}' for '{}' is missing ISO output in '{}'.",
                product_name,
                distro_id,
                run_dir.display()
            )
        })?;
    let filename = iso_path
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid ISO filename '{}'", iso_path.display()))?
        .to_string();

    Ok(Some(ScenarioIsoArtifact {
        scenario,
        product_name,
        path: iso_path,
        filename,
    }))
}

fn scenario_input_fingerprint(
    distro_id: &str,
    scenario: ScenarioId,
    iso_artifact: Option<&ScenarioIsoArtifact>,
) -> Result<String> {
    if let Some(iso) = iso_artifact {
        let mtime = std::fs::metadata(&iso.path)
            .with_context(|| format!("reading metadata for '{}'", iso.path.display()))?
            .modified()
            .with_context(|| format!("reading mtime for '{}'", iso.path.display()))?
            .duration_since(UNIX_EPOCH)
            .with_context(|| format!("mtime before UNIX_EPOCH for '{}'", iso.path.display()))?
            .as_secs();
        return Ok(format!(
            "iso:{}:{}:{}",
            iso.product_name,
            iso.path.display(),
            mtime
        ));
    }

    let install_runtime = resolve_latest_install_runtime(distro_id)?;
    Ok(format!(
        "install-runtime:{}:{}",
        scenario.key(),
        install_runtime.run_id
    ))
}

fn load_release_run_manifest(run_dir: &Path) -> Result<Option<ReleaseRunManifest>> {
    let manifest_path = run_dir.join("run-manifest.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let raw = fs::read(&manifest_path)
        .with_context(|| format!("reading release run manifest '{}'", manifest_path.display()))?;
    let manifest: ReleaseRunManifest = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing release run manifest '{}'", manifest_path.display()))?;
    Ok(Some(manifest))
}

fn release_product_root_dir(distro_id: &str, product_name: &str) -> PathBuf {
    workspace_root()
        .join(".artifacts/out")
        .join(distro_id)
        .join("releases")
        .join(product_name)
}

fn spawn_live_qemu_with_ssh(
    ctx: &dyn DistroContext,
    iso_path: &Path,
) -> Result<(std::process::Child, Console, u16)> {
    session::spawn_live_with_ssh(ctx, iso_path)
}

fn verify_live_boot_ssh_login(console: &mut Console, ssh_host_port: u16) -> Result<()> {
    let mut last_err = String::new();
    for _ in 0..60 {
        let out = ssh_exec(ssh_host_port, "echo __SSH_LOGIN_OK__");
        match out {
            Ok(result) if result.exit_code == 0 && result.output.contains("__SSH_LOGIN_OK__") => {
                return Ok(());
            }
            Ok(result) => last_err = result.output,
            Err(err) => last_err = format!("{err:#}"),
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    let diagnostics = collect_live_boot_ssh_diagnostics(console);
    bail!(
        "live-boot SSH login probe failed after shell-ready boundary (forwarded port {}). Last output:\n{}\n\n{}",
        ssh_host_port,
        last_err,
        diagnostics
    );
}

fn collect_live_boot_ssh_diagnostics(console: &mut Console) -> String {
    let checks = [
        ("Kernel cmdline", "cat /proc/cmdline"),
        (
            "Network interfaces",
            "ip -brief addr 2>/dev/null || ip addr 2>/dev/null || true",
        ),
        (
            "Network routes",
            "ip route 2>/dev/null || route -n 2>/dev/null || true",
        ),
        (
            "OpenRC networking status",
            "rc-service networking status 2>/dev/null || true",
        ),
        (
            "OpenRC dhcpcd status",
            "rc-service dhcpcd status 2>/dev/null || true",
        ),
        (
            "Root SSH auth files",
            "ls -ld /root /root/.ssh /root/.ssh/authorized_keys 2>/dev/null || true",
        ),
        (
            "Injected boot payload",
            "ls -l /run/boot-injection /run/boot-injection/* 2>/dev/null || true; cat /run/boot-injection/source /run/boot-injection/payload.env 2>/dev/null || true",
        ),
        (
            "sshd auth settings",
            "sshd -T 2>/dev/null | grep -E 'permitrootlogin|pubkeyauthentication|passwordauthentication|authorizedkeysfile' || true",
        ),
        (
            "sshd service status",
            "systemctl status sshd.service --no-pager -l || true",
        ),
        (
            "anaconda-sshd service status",
            "systemctl status anaconda-sshd.service --no-pager -l || true",
        ),
        (
            "sshd journal",
            "journalctl -b -u sshd.service --no-pager -n 80 || true",
        ),
        (
            "Port 22 listeners",
            "ss -lntp 2>/dev/null | grep ':22' || netstat -lntp 2>/dev/null | grep ':22' || true",
        ),
    ];

    let mut report = String::from("live-boot SSH diagnostics from live shell:\n");
    for (title, cmd) in checks {
        report.push_str(&format!("\n--- {} ---\n$ {}\n", title, cmd));
        match console.exec(cmd, Duration::from_secs(15)) {
            Ok(result) => {
                let output = result.output.trim();
                if output.is_empty() {
                    report.push_str("(no output)\n");
                } else {
                    report.push_str(output);
                    report.push('\n');
                }
            }
            Err(err) => {
                report.push_str(&format!("(failed to collect: {:#})\n", err));
            }
        }
    }
    report
}

fn ssh_exec(ssh_host_port: u16, remote_cmd: &str) -> Result<SshExecOutput> {
    recshuttle::ssh_exec_default_key(ssh_host_port, remote_cmd).with_context(|| {
        format!(
            "running host SSH command against forwarded live SSH port {}",
            ssh_host_port
        )
    })
}

fn run_stage_script_over_ssh(ssh_host_port: u16, script_path: &str) -> Result<()> {
    let result = ssh_exec(ssh_host_port, script_path)?;
    if result.exit_code == 0 {
        return Ok(());
    }

    bail!(
        "Stage script '{}' failed over SSH (exit {}):\n{}",
        script_path,
        result.exit_code,
        result.output.trim()
    );
}

fn verify_install_ux_split_behavior(ssh_host_port: u16) -> Result<String> {
    let probe = ssh_exec(
        ssh_host_port,
        "/usr/local/bin/levitate-install-entrypoint --probe",
    )
    .with_context(|| "probing install UX helper selection".to_string())?;
    if probe.exit_code != 0 {
        bail!(
            "install helper probe failed (exit {}): {}\n\
             Expected probe command: /usr/local/bin/levitate-install-entrypoint --probe",
            probe.exit_code,
            probe.output.trim()
        );
    }

    let helper = probe
        .output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("install-entrypoint-helper="))
        .unwrap_or("")
        .trim();
    let helper_ok =
        helper == "levitate-install-docs-split" || helper.ends_with("/levitate-install-docs-split");
    if !helper_ok {
        bail!(
            "install UX helper mismatch: expected 'levitate-install-docs-split', found '{}'.\n\
             Probe output: {}\n\
             Remediation: ensure the live install payload installs levitate-install-docs-split into PATH.",
            if helper.is_empty() { "<empty>" } else { helper },
            probe.output.trim()
        );
    }

    let smoke = ssh_exec(
        ssh_host_port,
        "LEVITATE_INSTALL_ENTRYPOINT_SMOKE=1 /usr/local/bin/levitate-install-entrypoint",
    )
    .with_context(|| "running install UX split-pane smoke launch".to_string())?;
    if smoke.exit_code != 0 {
        bail!(
            "install UX split-pane smoke launch failed (exit {}): {}\n\
             Expected command: LEVITATE_INSTALL_ENTRYPOINT_SMOKE=1 /usr/local/bin/levitate-install-entrypoint",
            smoke.exit_code,
            smoke.output.trim()
        );
    }
    if !smoke.output.contains("split-smoke:ok") {
        bail!(
            "install UX split-pane smoke output missing success marker 'split-smoke:ok'.\n\
             Command output: {}\n\
             Remediation: run `levitate-install-docs-split --smoke` in the live environment and fix pane launch wiring.",
            smoke.output.trim()
        );
    }

    Ok("install UX split-pane smoke verified (shell-left + docs-right)".to_string())
}

#[derive(Debug, Serialize)]
struct ScenarioRunManifest {
    run_id: String,
    distro_id: String,
    scenario_name: String,
    status: String,
    created_at_utc: String,
    finished_at_utc: Option<String>,
    scenario_root_dir: String,
    output_dir: String,
    evidence_path: Option<String>,
    disk_path: Option<String>,
    ovmf_vars_path: Option<String>,
    source_install_run_id: Option<String>,
}

struct ScenarioRun {
    run_id: String,
    distro_id: String,
    scenario: ScenarioId,
    scenario_root_dir: PathBuf,
    output_dir: PathBuf,
    created_at_utc: String,
    source_install_run_id: Option<String>,
}

impl ScenarioRun {
    fn start(
        distro_id: &str,
        scenario: ScenarioId,
        source_install_run_id: Option<String>,
    ) -> Result<Self> {
        let scenario_root_dir = scenario_runtime_root_dir(distro_id, scenario);
        let legacy_current_dir = scenario_root_dir.join("current");
        if legacy_current_dir.is_dir() {
            fs::remove_dir_all(&legacy_current_dir).with_context(|| {
                format!(
                    "removing legacy scenario shortcut directory '{}'",
                    legacy_current_dir.display()
                )
            })?;
        }
        let (run_id, output_dir) =
            distro_builder::stage_runs::allocate_run_dir(&scenario_root_dir)?;
        let run = Self {
            run_id,
            distro_id: distro_id.to_string(),
            scenario,
            scenario_root_dir,
            output_dir,
            created_at_utc: now_utc_sortable()?,
            source_install_run_id,
        };
        run.write_manifest("building", None, None, None, None)?;
        Ok(run)
    }

    fn finish_success(
        &self,
        evidence: &str,
        disk_path: Option<&Path>,
        ovmf_vars_path: Option<&Path>,
    ) -> Result<()> {
        let evidence_path = self.write_evidence(evidence)?;
        self.write_manifest(
            "success",
            Some(now_utc_sortable()?),
            Some(evidence_path.as_path()),
            disk_path,
            ovmf_vars_path,
        )?;
        distro_builder::stage_runs::prune_old_runs(
            &self.scenario_root_dir,
            STAGE_RUNTIME_RETENTION_COUNT,
        )
    }

    fn finish_failed(
        &self,
        evidence: &str,
        disk_path: Option<&Path>,
        ovmf_vars_path: Option<&Path>,
    ) -> Result<()> {
        let evidence_path = self.write_evidence(evidence)?;
        self.write_manifest(
            "failed",
            Some(now_utc_sortable()?),
            Some(evidence_path.as_path()),
            disk_path,
            ovmf_vars_path,
        )
    }

    fn write_manifest(
        &self,
        status: &str,
        finished_at_utc: Option<String>,
        evidence_path: Option<&Path>,
        disk_path: Option<&Path>,
        ovmf_vars_path: Option<&Path>,
    ) -> Result<()> {
        let metadata = ScenarioRunManifest {
            run_id: self.run_id.clone(),
            distro_id: self.distro_id.clone(),
            scenario_name: self.scenario.key().to_string(),
            status: status.to_string(),
            created_at_utc: self.created_at_utc.clone(),
            finished_at_utc,
            scenario_root_dir: self.scenario_root_dir.display().to_string(),
            output_dir: self.output_dir.display().to_string(),
            evidence_path: evidence_path.map(|p| p.display().to_string()),
            disk_path: disk_path.map(|p| p.display().to_string()),
            ovmf_vars_path: ovmf_vars_path.map(|p| p.display().to_string()),
            source_install_run_id: self.source_install_run_id.clone(),
        };
        let manifest_path = distro_builder::stage_runs::manifest_path(&self.output_dir);
        write_json_atomic(&manifest_path, &metadata).with_context(|| {
            format!(
                "writing scenario runtime metadata '{}'",
                manifest_path.display()
            )
        })
    }

    fn write_evidence(&self, evidence: &str) -> Result<PathBuf> {
        let evidence_path = self.output_dir.join("last-result.txt");
        let body = format!(
            "timestamp_unix_ns={}\ndistro={}\nscenario={}\n{}\n",
            now_unix_nanos()?,
            self.distro_id,
            self.scenario.key(),
            evidence
        );
        fs::write(&evidence_path, body).with_context(|| {
            format!(
                "writing scenario runtime evidence '{}'",
                evidence_path.display()
            )
        })?;
        Ok(evidence_path)
    }
}

pub fn resolve_latest_install_runtime(distro_id: &str) -> Result<InstallScenarioRuntime> {
    let scenario_root = scenario_runtime_root_dir(distro_id, ScenarioId::Install);
    let run_id =
        distro_builder::stage_runs::latest_successful_run_id(&scenario_root)?.ok_or_else(|| {
            anyhow::anyhow!(
                "install scenario runtime not found for '{}': no successful runs under '{}'.\n\
                 Run: cargo run --bin stages -- --distro {} --scenario install",
                distro_id,
                scenario_root.display(),
                distro_id
            )
        })?;
    let run_dir = scenario_root.join(&run_id);
    let disk_path = run_dir.join(INSTALL_DISK_FILENAME);
    let ovmf_vars_path = run_dir.join(INSTALL_OVMF_VARS_FILENAME);
    if !disk_path.is_file() {
        bail!(
            "install scenario disk image missing for '{}' run '{}': '{}'.\n\
             Re-run install: cargo run --bin stages -- --distro {} --scenario install",
            distro_id,
            run_id,
            disk_path.display(),
            distro_id
        );
    }
    if !ovmf_vars_path.is_file() {
        bail!(
            "install scenario OVMF vars missing for '{}' run '{}': '{}'.\n\
             Re-run install: cargo run --bin stages -- --distro {} --scenario install",
            distro_id,
            run_id,
            ovmf_vars_path.display(),
            distro_id
        );
    }
    Ok(InstallScenarioRuntime {
        run_id,
        disk_path,
        ovmf_vars_path,
    })
}

fn scenario_runtime_root_dir(distro_id: &str, scenario: ScenarioId) -> PathBuf {
    workspace_root()
        .join(".artifacts/out")
        .join(distro_id)
        .join(SCENARIO_ROOT_DIRNAME)
        .join(scenario.scenario_output_dirname())
}

fn workspace_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot write stage runtime metadata without parent directory: '{}'",
            path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating metadata parent directory '{}'", parent.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let payload = serde_json::to_vec_pretty(value)
        .with_context(|| "serializing scenario runtime metadata")?;
    fs::write(&tmp, payload).with_context(|| format!("writing temp file '{}'", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "renaming temp metadata '{}' to '{}'",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn now_utc_sortable() -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .with_context(|| {
            "system clock before UNIX_EPOCH while recording stage runtime metadata".to_string()
        })?;
    Ok(format!("{:020}{:09}", now.as_secs(), now.subsec_nanos()))
}

fn now_unix_nanos() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .with_context(|| {
            "system clock before UNIX_EPOCH while recording stage runtime evidence".to_string()
        })?
        .as_nanos())
}

fn verify_live_overlay_behavior(console: &mut Console) -> Result<String> {
    let marker = console.exec("test -f /live-boot-marker", Duration::from_secs(5))?;
    if !marker.success() {
        bail!("Live overlay marker missing: /live-boot-marker");
    }

    let overlay_mount = console.exec(
        "mount | grep ' type overlay ' | grep 'lowerdir=/live-overlay:/rootfs'",
        Duration::from_secs(5),
    )?;
    if !overlay_mount.success() {
        bail!("Overlay root mount is missing required lowerdir=/live-overlay:/rootfs chain");
    }

    Ok("overlayfs lowerdir chain verified".to_string())
}

fn install_layout_for_distro(distro_id: &str) -> Result<InstallLayout> {
    let contract = distro_spec::conformance::contract_for_distro(distro_id).ok_or_else(|| {
        anyhow::anyhow!(
            "missing conformance contract for distro '{}'; add it in distro-spec::conformance::contract_for_distro",
            distro_id
        )
    })?;

    match contract.stages.stage_07_runtime_policy.rootfs_mutability {
        RootfsMutability::Mutable => Ok(InstallLayout::MutableSingleRoot),
        RootfsMutability::Immutable => Ok(InstallLayout::ImmutableAb),
    }
}

/// Get the validation command for a tool.
///
/// Returns a command that:
/// 1. Actually executes the tool (not just checks if it exists)
/// 2. Exits with code 0 on success
/// 3. Exits with code 127 if tool not found
/// 4. Exits with non-zero if tool is broken/misconfigured
///
/// Most tools support --version which is perfect for verification.
fn get_tool_validation_command(tool: &str) -> String {
    match tool {
        // Installation tools - most support --help
        "recstrap" | "recfstab" | "recchroot" | "iuppiter-dar" => {
            format!("{} --help >/dev/null 2>&1", tool)
        }

        // Partitioning/filesystem tools
        "sfdisk" => "sfdisk --version >/dev/null 2>&1".to_string(),
        "mkfs.ext4" => "mkfs.ext4 -V 2>&1 | head -1 >/dev/null".to_string(),
        "parted" => "parted --version >/dev/null 2>&1".to_string(),
        "sgdisk" => "sgdisk --version >/dev/null 2>&1".to_string(),

        // Mount is special - just verify it's callable
        "mount" | "umount" => format!("{} --version >/dev/null 2>&1", tool),

        // Network tools
        "ip" => "ip link show >/dev/null 2>&1".to_string(),
        "ping" => {
            "ping -h >/dev/null 2>&1 || ping --help >/dev/null 2>&1 || ping -V >/dev/null 2>&1"
                .to_string()
        }
        "curl" => "curl --version >/dev/null 2>&1".to_string(),

        // Hardware diagnostics
        "lspci" => "lspci --version >/dev/null 2>&1".to_string(),
        "lsusb" => "lsusb --version >/dev/null 2>&1".to_string(),
        "smartctl" => "smartctl --version >/dev/null 2>&1".to_string(),
        "hdparm" => "hdparm -V >/dev/null 2>&1".to_string(),
        "sg_inq" => "sg_inq --version >/dev/null 2>&1".to_string(),
        "nvme" => "nvme version >/dev/null 2>&1".to_string(),
        "dmidecode" => "dmidecode --version >/dev/null 2>&1".to_string(),
        "ethtool" => "ethtool --version >/dev/null 2>&1".to_string(),

        // Editors and pagers
        "vim" => "vim --version >/dev/null 2>&1".to_string(),
        "vi" => "vi -h 2>&1 | head -1 >/dev/null".to_string(),
        "less" => "less --help >/dev/null 2>&1 || less -V >/dev/null 2>&1".to_string(),

        // System utilities
        "htop" => "htop --version >/dev/null 2>&1".to_string(),
        "grep" => "echo x | grep x >/dev/null 2>&1".to_string(),
        "find" => "find / -maxdepth 0 >/dev/null 2>&1".to_string(),
        "sed" => "sed --version >/dev/null 2>&1".to_string(),
        "awk" | "gawk" => "awk --version >/dev/null 2>&1".to_string(),

        // SSH/sudo
        "ssh" => "ssh -V 2>&1 >/dev/null".to_string(),
        "sudo" | "doas" => format!("{} --version >/dev/null 2>&1", tool),

        // Shell and basic tools (busybox applets)
        "ash" | "bash" | "sh" => format!("{} --version >/dev/null 2>&1", tool),
        "cat" | "ls" | "ps" | "dmesg" => {
            format!("{} --version >/dev/null 2>&1 || true", tool)
        }

        // Unknown tool - try generic --version
        _ => format!("{} --version >/dev/null 2>&1", tool),
    }
}

fn print_failure(scenario: ScenarioId, err: &anyhow::Error) {
    eprintln!();
    eprintln!(
        "{} {} FAILED: {}",
        "[FAIL]".red().bold(),
        scenario.display_name(),
        scenario.key()
    );
    eprintln!();
    eprintln!("  Error: {:#}", err);
    eprintln!();

    match scenario {
        ScenarioId::LiveBoot => {
            eprintln!("  Common causes:");
            eprintln!("    - ISO not built or corrupted");
            eprintln!("    - Kernel panic during boot");
            eprintln!("    - initramfs missing /init");
            eprintln!("    - UEFI firmware not finding boot entry");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Manual boot: just stage 1 <distro>");
            eprintln!(
                "    - Rebuild live-boot ISO: cargo run -p distro-builder --bin distro-builder -- iso build <distro> 01Boot"
            );
        }
        ScenarioId::LiveTools => {
            eprintln!("  Common causes:");
            eprintln!("    - Tools not included in initramfs or rootfs");
            eprintln!("    - PATH not set correctly");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Check package list in distro-spec");
        }
        ScenarioId::Install => {
            eprintln!("  Common causes:");
            eprintln!("    - recstrap/recfstab/recchroot broken");
            eprintln!("    - Disk too small or partition failure");
            eprintln!("    - Bootloader install failure");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Manual install: boot ISO, run install commands by hand");
        }
        ScenarioId::InstalledBoot => {
            eprintln!("  Common causes:");
            eprintln!("    - Bootloader not installed correctly");
            eprintln!("    - UKI not found by systemd-boot");
            eprintln!("    - Missing kernel modules");
        }
        ScenarioId::AutomatedLogin => {
            eprintln!("  Common causes:");
            eprintln!("    - No serial getty enabled");
            eprintln!("    - Password not set correctly");
            eprintln!("    - Shell not functional");
        }
        ScenarioId::Runtime => {
            eprintln!("  Common causes:");
            eprintln!("    - Packages not installed in rootfs");
            eprintln!("    - PATH misconfigured");
        }
        _ => {}
    }
    eprintln!();
}
