//! Scenario-based development loop.
//!
//! Independent named scenarios give focused feedback during development.
//! Each scenario consumes existing artifacts and validates one behavior.
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

pub mod state;

mod install;
use crate::distro::{context_for_distro, load_installed_scenario_facts, load_scenarios, DistroContext};
use crate::preflight::require_preflight;
use crate::qemu::session;
use crate::qemu::{Console, SerialExecutorExt};
use anyhow::{bail, Context, Result};
use colored::Colorize;
use distro_contract::RootfsMutability;
use recpart::InstallMode;
use recshuttle::Remote;
use distro_builder::live_ux::{
    INSTALL_ENTRYPOINT, INSTALL_EXPERIENCE_MARKER, INSTALL_UX_PROFILE, PROBE_PREFIX, SMOKE_ENV,
    SPLIT_HELPER, SPLIT_SMOKE_OK,
};
use distro_contract::InstallExperience;
use crate::qemu::session::VmSpec;
use crate::repo_root;
use distro_builder::run_history;
use serde::Serialize;
use state::ScenarioState;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const LIVE_BOOT_SSH_PREFLIGHT_SCRIPT: &str = "/usr/local/bin/live-boot-ssh-preflight.sh";
const SCENARIO_RUNTIME_RETENTION_COUNT: usize = 5;
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
        require_preflight(iso_dir, Some(&iso.filename), canonical_distro_id)?;
    }

    let input_fingerprint =
        scenario_input_fingerprint(canonical_distro_id, scenario, scenario_iso.as_ref())?;

    let mut state = ScenarioState::load(canonical_distro_id)?;
    if !state.is_valid_for_scenario_input(scenario, &input_fingerprint) {
        println!(
            "{}",
            format!(
                "Scenario input changed for {} — resetting its cached result.",
                scenario.display_name()
            )
            .yellow()
        );
        state.reset_for_scenario_input(scenario, &input_fingerprint);
        state.save(canonical_distro_id)?;
    }

    if force {
        state.results.remove(scenario.key());
        state.save(canonical_distro_id)?;
        println!("Forcing {} rerun.", scenario.display_name());
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
        ScenarioId::BuildPreflight => Ok(format!(
            "Preflight conformance + artifact checks passed for {}",
            scenario_iso
                .as_ref()
                .map(|iso| iso.path.display().to_string())
                .unwrap_or_default()
        )),
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

/// Print scenario status for a distro.
pub fn print_status(distro_id: &str) -> Result<()> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;
    let canonical_distro_id = ctx.id();

    let state = ScenarioState::load(canonical_distro_id)?;
    println!("{} Scenario Status", ctx.name().bold());
    println!();
    for scenario in ScenarioId::ALL {
        let current = if state.has_result(scenario) {
            resolve_iso_artifact_for_scenario(canonical_distro_id, scenario)
                .and_then(|iso| {
                    scenario_input_fingerprint(canonical_distro_id, scenario, iso.as_ref())
                })
                .map(|fingerprint| state.is_valid_for_scenario_input(scenario, &fingerprint))
                .unwrap_or(false)
        } else {
            true
        };
        let status = if !current {
            "[STALE]".yellow()
        } else if state.has_passed(scenario) {
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
    Ok(())
}

/// Reset all scenario state for a distro.
pub fn reset_state(distro_id: &str) -> Result<()> {
    let canonical_distro_id = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?
        .id()
        .to_string();
    let path = state::state_path(&canonical_distro_id);
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

/// Live Boot scenario — ISO boots in QEMU.
fn run_live_boot(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let live_boot_evidence = load_scenarios(ctx.id())?.live_boot.evidence;
    let mut vm = session::spawn(VmSpec {
        iso: Some(iso_path),
        ssh: true,
        ..Default::default()
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = &mut vm.console;
    let stall_timeout = Duration::from_secs(ctx.live_boot_stall_timeout_secs());

    console.wait_for_live_boot_with_context(stall_timeout, ctx)?;
    verify_live_boot_ssh_login(console, ssh_host_port)?;

    evidence_over_ssh(ssh_host_port, &live_boot_evidence, &[])?;
    Remote::new(ssh_host_port).run(
        "live-boot SSH preflight",
        &install::shell_quote(LIVE_BOOT_SSH_PREFLIGHT_SCRIPT),
    )?;

    Ok(
        "Boot markers detected + SSH login probe passed + live-boot script checks passed"
            .to_string(),
    )
}

/// Live Tools scenario — expected binaries in the live environment.
///
/// Each tool is run with a harmless invocation where it has one (see
/// [`check_tool`]), so missing libraries or a broken binary fail, not just
/// absence from PATH. The live overlay's behavior is checked too.
fn run_live_tools(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let mut vm = session::spawn(VmSpec {
        iso: Some(iso_path),
        ssh: true,
        ..Default::default()
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = &mut vm.console;
    wait_for_live_tools_serial_readiness(console, ctx)?;
    verify_live_boot_ssh_login(console, ssh_host_port)?;

    let required_tools = load_scenarios(ctx.id())?.live_tools.required_tools;
    let tools: Vec<&str> = required_tools.iter().map(String::as_str).collect();
    let mut missing = Vec::new();
    let mut found = Vec::new();
    let mut broken = Vec::new();

    let remote = Remote::new(ssh_host_port);
    for tool in &tools {
        match check_tool(tool, |cmd| Ok(remote.exec(cmd)?.exit_code))? {
            ToolCheck::Works => found.push(*tool),
            ToolCheck::Missing => missing.push(*tool),
            ToolCheck::Broken(code) => broken.push((*tool, code)),
        }
    }

    let overlay_evidence = verify_live_overlay_behavior(console)?;

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
            for (tool, code) in &broken {
                error_msg.push_str(&format!("  {tool} (exit {code})\n"));
            }
        }

        error_msg.push_str(&format!("\nWorking tools: {}", found.join(", ")));

        bail!("{}", error_msg.trim());
    }

    let expected = load_scenarios(ctx.id())?.live_tools.install_experience;
    let remote = Remote::new(ssh_host_port);
    let marker = remote.run(
        "reading the install-experience marker",
        &format!("cat /{INSTALL_EXPERIENCE_MARKER}"),
    )?;
    let actual_install_experience = marker.lines().next().unwrap_or("").trim().to_string();
    if actual_install_experience != expected.as_str() {
        bail!(
            "install-experience mismatch: expected '{}', found '{actual_install_experience}'",
            expected.as_str()
        );
    }
    remote.run(
        "checking the install entrypoint",
        &format!("test -x /{INSTALL_ENTRYPOINT}"),
    )?;
    let mut ux_split_evidence: Option<String> = None;
    if expected == InstallExperience::Ux {
        remote.run(
            "checking the install UX profile hook",
            &format!("test -r /{INSTALL_UX_PROFILE}"),
        )?;
        ux_split_evidence = Some(verify_install_ux_split_behavior(&remote)?);
    }

    let script_evidence = evidence_over_ssh(
        ssh_host_port,
        &load_scenarios(ctx.id())?.live_tools.evidence,
        &[],
    )?;
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
    ) + &format!("; {script_evidence}"))
}

fn wait_for_live_tools_serial_readiness(
    console: &mut Console,
    ctx: &dyn DistroContext,
) -> Result<()> {
    let stall_timeout = Duration::from_secs(ctx.live_boot_stall_timeout_secs());
    // Live-tools is validated over SSH; the serial console may run the
    // install UX rather than a shell.
    console
        .wait_for_live_tools_with_context(stall_timeout, ctx)
        .context("waiting for live-tools serial readiness")
}

fn run_installation(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let scenario_run = ScenarioRun::start(ctx.id(), ScenarioId::Install, None)?;
    let disk_path = scenario_run.output_dir.join(INSTALL_DISK_FILENAME);
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    recqemu::create_disk(&disk_path, "20G")?;

    let ovmf_vars_path = session::setup_ovmf_vars_at(
        &scenario_run.output_dir.join(INSTALL_OVMF_VARS_FILENAME),
    )?;
    let mut vm = session::spawn(VmSpec {
        iso: Some(iso_path),
        disk: Some(&disk_path),
        ovmf_vars: Some(&ovmf_vars_path),
        ssh: true,
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = &mut vm.console;

    // The installation runs over SSH; the serial console only marks the
    // boot boundary.
    let remote = Remote::new(ssh_host_port);
    let install_result = (|| -> Result<usize> {
        console.wait_for_live_boot_with_context(
            Duration::from_secs(ctx.live_boot_stall_timeout_secs()),
            ctx,
        )?;
        remote.wait_ready(Duration::from_secs(ctx.live_boot_stall_timeout_secs()))?;

        let root_password = default_password(ctx)?;
        let system = install::TestSystem {
            mode: install_mode_for_distro(ctx.id())?,
            root_password: &root_password,
            uses_initramfs: ctx.uses_initramfs(),
        };
        let (target, step_count) = install::install(&remote, ctx, &system)?;
        evidence_over_ssh(
            ssh_host_port,
            &load_scenarios(ctx.id())?.install.evidence,
            &[("SCENARIO_INSTALL_TARGET", target.as_str())],
        )?;

        let mut verify = vec![
            ("Root filesystem", format!("test -f {target}/etc/os-release")),
            ("Boot partition", format!("ls {target}/boot/EFI")),
            ("Kernel on ESP", format!("ls {target}/boot/vmlinuz")),
            ("systemd-boot loader config", format!("cat {target}/boot/loader/loader.conf")),
            ("fstab root entry", format!("awk '$2==\"/\" {{ok=1}} END{{exit(ok?0:1)}}' {target}/etc/fstab")),
        ];
        if system.uses_initramfs {
            verify.push(("Initramfs on ESP", format!("ls {target}/boot/initramfs.img")));
        }
        match system.mode {
            InstallMode::Mutable => verify.push((
                "systemd-boot entry",
                format!("cat {target}/boot/loader/entries/{}.conf", ctx.id()),
            )),
            InstallMode::Ab => {
                verify.push((
                    "systemd-boot entry slot A",
                    format!("cat {target}/boot/loader/entries/{}-a.conf", ctx.id()),
                ));
                verify.push((
                    "systemd-boot entry slot B",
                    format!("cat {target}/boot/loader/entries/{}-b.conf", ctx.id()),
                ));
            }
        }
        verify.push(("Sync filesystem", "sync".to_string()));
        verify.push(("Unmount install target", format!("umount -R {target}")));
        for (what, cmd) in &verify {
            remote.run(what, cmd)?;
        }

        Ok(step_count)
    })();

    remote.power_off();
    drop(vm);

    match install_result {
        Ok(step_count) => {
            let evidence = format!(
                "{} install steps completed + verified over SSH",
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
    let mut vm = installed_vm(&install_runtime)?;
    let console = &mut vm.console;

    let result = console.wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, ctx);
    drop(vm);

    match result {
        Ok(failed_services) => {
            let evidence = if failed_services.is_empty() {
                "Installed system boot markers detected".to_string()
            } else {
                format!(
                    "Installed system boot markers detected; failed services seen: {}",
                    failed_services.join(" | ")
                )
            };
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
    let mut vm = installed_vm(&install_runtime)?;
    let console = &mut vm.console;

    console.wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, ctx)?;

    console.login("root", &default_password(ctx)?, LOGIN_TIMEOUT)?;

    let result = console.exec("echo SCENARIO_LOGIN_OK", COMMAND_TIMEOUT)?;
    let script_evidence = evidence_on_console(
        console,
        &load_scenarios(ctx.id())?.automated_login.evidence,
    )?;

    if result.output.contains("SCENARIO_LOGIN_OK") {
        Ok(format!("Login succeeded, shell functional; {script_evidence}"))
    } else {
        bail!(
            "Login succeeded but shell not functional. Got: {}",
            result.output.trim()
        );
    }
}

fn run_daily_driver_tools(ctx: &dyn DistroContext) -> Result<String> {
    let install_runtime = resolve_latest_install_runtime(ctx.id())?;
    let mut vm = installed_vm(&install_runtime)?;
    let console = &mut vm.console;

    console.wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, ctx)?;
    let facts = load_installed_scenario_facts(ctx.id())?;
    console.login("root", &default_password(ctx)?, LOGIN_TIMEOUT)?;

    let tools: Vec<&str> = facts
        .installed_tools
        .required_tools
        .iter()
        .map(String::as_str)
        .collect();
    let mut missing = Vec::new();
    let mut found = Vec::new();

    let mut broken = Vec::new();
    for tool in &tools {
        match check_tool(tool, |cmd| Ok(console.exec(cmd, COMMAND_TIMEOUT)?.exit_code))? {
            ToolCheck::Works => found.push(*tool),
            ToolCheck::Missing => missing.push(*tool),
            ToolCheck::Broken(code) => broken.push(format!("{tool} (exit {code})")),
        }
    }


    if !missing.is_empty() || !broken.is_empty() {
        bail!(
            "Missing daily driver tools: {}\nBroken: {}\nFound: {}",
            missing.join(", "),
            broken.join(", "),
            found.join(", ")
        );
    }

    let script_evidence = evidence_on_console(console, &facts.installed_tools.evidence)?;
    Ok(format!(
        "All {} daily driver tools present; {script_evidence}",
        found.len()
    ))
}

pub fn resolve_iso_artifact_for_scenario(
    distro_id: &str,
    scenario: ScenarioId,
) -> Result<Option<ScenarioIsoArtifact>> {
    let Some(product_name) = scenario.release_product() else {
        return Ok(None);
    };
    let release_root = run_history::release_product_dir(&repo_root(), distro_id, product_name);
    let run_id = run_history::latest_successful_run_id(&release_root)?.ok_or_else(|| {
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
    let manifest = run_history::read_run_manifest(&run_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "release product '{}' for '{}' is missing run-manifest metadata under '{}'.",
            product_name,
            distro_id,
            run_dir.display()
        )
    })?;
    if manifest.status != run_history::STATUS_SUCCESS {
        bail!(
            "latest '{}' release run for '{}' is not successful under '{}'",
            product_name,
            distro_id,
            run_dir.display()
        );
    }
    if manifest.target_kind.as_deref() != Some(run_history::TARGET_KIND_RELEASE_PRODUCT)
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

/// What a cached pass depends on: the scenario's input (ISO or install run)
/// and the harness binary, which links recqemu, recshuttle and recpart, so a
/// change to the harness or those tools reruns the scenario.
fn scenario_input_fingerprint(
    distro_id: &str,
    scenario: ScenarioId,
    iso_artifact: Option<&ScenarioIsoArtifact>,
) -> Result<String> {
    let mtime = |path: &Path| -> Result<u64> {
        Ok(fs::metadata(path)
            .with_context(|| format!("reading metadata for '{}'", path.display()))?
            .modified()
            .with_context(|| format!("reading mtime for '{}'", path.display()))?
            .duration_since(UNIX_EPOCH)
            .with_context(|| format!("mtime before UNIX_EPOCH for '{}'", path.display()))?
            .as_secs())
    };
    let harness_path = std::env::current_exe().context("locating the scenario harness binary")?;
    let harness = format!(
        "harness:{}:{}",
        mtime(&harness_path)?,
        fs::metadata(&harness_path)?.len()
    );
    let input = match iso_artifact {
        Some(iso) => format!(
            "iso:{}:{}:{}",
            iso.product_name,
            iso.path.display(),
            mtime(&iso.path)?
        ),
        None => format!(
            "install-runtime:{}:{}",
            scenario.key(),
            resolve_latest_install_runtime(distro_id)?.run_id
        ),
    };
    Ok(format!("{input};{harness}"))
}

fn verify_live_boot_ssh_login(console: &mut Console, ssh_host_port: u16) -> Result<()> {
    Remote::new(ssh_host_port)
        .wait_ready(Duration::from_secs(60))
        .map_err(|err| {
            let diagnostics = collect_live_boot_ssh_diagnostics(console);
            anyhow::anyhow!("live-boot SSH login probe failed after shell-ready boundary: {err:#}\n\n{diagnostics}")
        })
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
            &format!(
                "ls -l {d} {d}/* 2>/dev/null || true; cat {d}/source {e} 2>/dev/null || true",
                d = distro_spec::shared::live_boot::BOOT_INJECTION_DIR,
                e = distro_spec::shared::live_boot::BOOT_INJECTION_ENV
            ),
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

/// Run a scenario's evidence script (installed in the image) through `exec`,
/// which runs a guest command and returns its exit status and output; the
/// script must exit 0 and print its pass marker.
fn run_evidence(
    evidence: &distro_contract::ScriptEvidence,
    env: &[(&str, &str)],
    mut exec: impl FnMut(&str) -> Result<(i32, String)>,
) -> Result<String> {
    let path = installed_script_path(&evidence.script_path);
    let mut cmd = format!("SCENARIO_PASS_MARKER={}", install::shell_quote(&evidence.pass_marker));
    for (key, value) in env {
        cmd.push_str(&format!(" {key}={}", install::shell_quote(value)));
    }
    let (code, output) = exec(&format!("{cmd} {}", install::shell_quote(&path)))?;
    if code != 0 || !output.contains(&evidence.pass_marker) {
        bail!(
            "evidence script {path} failed (exit {code}, pass marker {}):\n{}",
            if output.contains(&evidence.pass_marker) { "printed" } else { "missing" },
            output.trim()
        );
    }
    Ok(format!("{} passed", evidence.script_path))
}

/// `run_evidence` over SSH.
fn evidence_over_ssh(
    ssh_host_port: u16,
    evidence: &distro_contract::ScriptEvidence,
    env: &[(&str, &str)],
) -> Result<String> {
    let remote = Remote::new(ssh_host_port);
    run_evidence(evidence, env, |cmd| {
        let out = remote.exec(cmd)?;
        Ok((out.exit_code, out.output))
    })
}

/// `run_evidence` on the serial console.
fn evidence_on_console(console: &mut Console, evidence: &distro_contract::ScriptEvidence) -> Result<String> {
    run_evidence(evidence, &[], |cmd| {
        let out = console.exec(cmd, EVIDENCE_TIMEOUT)?;
        Ok((out.exit_code, out.output))
    })
}

fn verify_install_ux_split_behavior(remote: &Remote) -> Result<String> {
    let probe = remote.run(
        "probing the install helper",
        &format!("/{INSTALL_ENTRYPOINT} --probe"),
    )?;
    let helper = probe
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(PROBE_PREFIX))
        .unwrap_or("")
        .trim();
    if helper != SPLIT_HELPER && !helper.ends_with(&format!("/{SPLIT_HELPER}")) {
        bail!(
            "install UX helper mismatch: expected '{SPLIT_HELPER}', found '{}'.\nProbe output: {}",
            if helper.is_empty() { "<empty>" } else { helper },
            probe.trim()
        );
    }
    let smoke = remote.run(
        "running the split-pane smoke launch",
        &format!("{SMOKE_ENV}=1 /{INSTALL_ENTRYPOINT}"),
    )?;
    if !smoke.contains(SPLIT_SMOKE_OK) {
        bail!(
            "split-pane smoke output lacks '{SPLIT_SMOKE_OK}':\n{}",
            smoke.trim()
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
        let (run_id, output_dir) =
            run_history::allocate_run_dir(&scenario_root_dir)?;
        let run = Self {
            run_id,
            distro_id: distro_id.to_string(),
            scenario,
            scenario_root_dir,
            output_dir,
            created_at_utc: now_utc_sortable()?,
            source_install_run_id,
        };
        run.write_manifest(run_history::STATUS_BUILDING, None, None, None, None)?;
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
            run_history::STATUS_SUCCESS,
            Some(now_utc_sortable()?),
            Some(evidence_path.as_path()),
            disk_path,
            ovmf_vars_path,
        )?;
        run_history::prune_old_runs(
            &self.scenario_root_dir,
            SCENARIO_RUNTIME_RETENTION_COUNT,
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
            run_history::STATUS_FAILED,
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
        let manifest_path = run_history::run_manifest_path(&self.output_dir);
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
    let run_id = run_history::latest_successful_run_id(&scenario_root)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "install scenario runtime not found for '{}': no successful runs under '{}'.\n\
                 Run: cargo run --bin scenarios -- --distro {} --scenario install",
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
             Re-run install: cargo run --bin scenarios -- --distro {} --scenario install",
            distro_id,
            run_id,
            disk_path.display(),
            distro_id
        );
    }
    if !ovmf_vars_path.is_file() {
        bail!(
            "install scenario OVMF vars missing for '{}' run '{}': '{}'.\n\
             Re-run install: cargo run --bin scenarios -- --distro {} --scenario install",
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
    run_history::distro_output_dir(&repo_root(), distro_id)
        .join(SCENARIO_ROOT_DIRNAME)
        .join(scenario.scenario_output_dirname())
}

fn installed_script_path(script_filename: &str) -> String {
    format!("/usr/local/bin/{script_filename}")
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot write scenario runtime metadata without parent directory: '{}'",
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
            "system clock before UNIX_EPOCH while recording scenario runtime metadata".to_string()
        })?;
    Ok(format!("{:020}{:09}", now.as_secs(), now.subsec_nanos()))
}

fn now_unix_nanos() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .with_context(|| {
            "system clock before UNIX_EPOCH while recording scenario runtime evidence".to_string()
        })?
        .as_nanos())
}

fn verify_live_overlay_behavior(console: &mut Console) -> Result<String> {
    let marker = console.exec("test -f /live-boot-marker", COMMAND_TIMEOUT)?;
    if !marker.success() {
        bail!("Live overlay marker missing: /live-boot-marker");
    }

    let overlay_mount = console.exec(
        "mount | grep ' type overlay ' | grep 'lowerdir=/live-overlay:/rootfs'",
        COMMAND_TIMEOUT,
    )?;
    if !overlay_mount.success() {
        bail!("Overlay root mount is missing required lowerdir=/live-overlay:/rootfs chain");
    }

    Ok("overlayfs lowerdir chain verified".to_string())
}

/// How long the installed system may stay silent while booting.
const INSTALLED_BOOT_STALL: Duration = Duration::from_secs(90);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const EVIDENCE_TIMEOUT: Duration = Duration::from_secs(120);

/// The variant's default root password for automated logins.
fn default_password(ctx: &dyn DistroContext) -> Result<String> {
    load_installed_scenario_facts(ctx.id())?
        .automated_login
        .default_password
        .with_context(|| {
            format!(
                "missing canonical automated-login default_password for '{}'",
                ctx.id()
            )
        })
}

/// Boot the latest install run's disk.
fn installed_vm(install_runtime: &InstallScenarioRuntime) -> Result<session::Vm> {
    session::spawn(VmSpec {
        disk: Some(&install_runtime.disk_path),
        ovmf_vars: Some(&install_runtime.ovmf_vars_path),
        ..Default::default()
    })
}

fn install_mode_for_distro(distro_id: &str) -> Result<InstallMode> {
    match load_installed_scenario_facts(distro_id)?
        .runtime_policy
        .rootfs_mutability
    {
        RootfsMutability::Mutable => Ok(InstallMode::Mutable),
        RootfsMutability::Immutable => Ok(InstallMode::Ab),
    }
}

enum ToolCheck {
    Works,
    Missing,
    Broken(i32),
}

/// Check that `tool` is on PATH (`command -v`) and runs: its known harmless
/// invocation must exit 0. `exec` runs a command in the guest and returns
/// its exit status.
fn check_tool(tool: &str, mut exec: impl FnMut(&str) -> Result<i32>) -> Result<ToolCheck> {
    if exec(&format!("command -v {tool} >/dev/null"))? != 0 {
        return Ok(ToolCheck::Missing);
    }
    let Some(run) = tool_run_command(tool) else {
        return Ok(ToolCheck::Works);
    };
    Ok(match exec(&format!("{{ {run}; }} >/dev/null 2>&1"))? {
        0 => ToolCheck::Works,
        code => ToolCheck::Broken(code),
    })
}

/// A harmless invocation of `tool` that exits 0 when it works (missing
/// libraries, broken binaries fail), or `None` when the tool has no such
/// invocation and presence on PATH is the check (busybox `vi`).
fn tool_run_command(tool: &str) -> Option<String> {
    Some(match tool {
        "recpart" | "recstrap" | "recfstab" | "recchroot" => format!("{tool} --help"),
        "mkfs.ext4" => "mkfs.ext4 -V".to_string(),
        "nvme" => "nvme version".to_string(),
        "hdparm" => "hdparm -V".to_string(),
        "ip" => "ip link show".to_string(),
        "ping" => "ping -V || ping -h || ping --help".to_string(),
        "less" => "less --help || less -V".to_string(),
        "grep" => "echo x | grep x".to_string(),
        "find" => "find / -maxdepth 0".to_string(),
        "ssh" => "ssh -V".to_string(),
        // OpenDoas has no --version option.
        "doas" => "doas -C /etc/doas.conf".to_string(),
        "sh" | "ash" => format!("{tool} -c true"),
        "cat" => "cat /dev/null".to_string(),
        "ls" => "ls /".to_string(),
        "ps" => "ps".to_string(),
        "dmesg" => "dmesg".to_string(),
        "vi" => return None,
        _ => format!("{tool} --version"),
    })
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
            eprintln!(
                "    - Rebuild live-boot ISO: cargo run -p distro-builder --bin distro-builder -- release build iso <distro> live-boot"
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
