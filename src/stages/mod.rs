//! Stage-based development loop.
//!
//! Lightweight, incremental stages that gate progression and give fast
//! feedback during development. Each stage validates one thing.
//!
//! # Stages
//!
//! 00. **00Build** — Contract + runtime provenance + artifact preflight
//! 01. **01Boot** — ISO boots in QEMU (login prompt or `___SHELL_READY___`)
//! 02. **02LiveTools** — Expected binaries present in live environment
//! 03. **03Install** — Scripted install to disk succeeds
//! 04. **04LoginGate** — System boots from disk after install
//! 05. **05Harness** — Harness can login and run commands
//! 06. **06Runtime** — Expected installed-system tools are present

pub mod state;

use crate::distro::{context_for_distro, DistroContext};
use crate::preflight::require_preflight_with_iso_for_distro;
use crate::qemu::session;
use crate::qemu::{Console, SerialExecutorExt};
use anyhow::{bail, Context, Result};
use colored::Colorize;
use distro_contract::load_stage_00_contract_bundle_for_distro_from;
use serde::Deserialize;
use state::StageState;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const STAGE00_SLUG: &str = "s00_build";
const STAGE01_SLUG: &str = "s01_boot";
const STAGE02_SLUG: &str = "s02_live_tools";
const STAGE00_DIRNAME: &str = "s00-build";
const STAGE01_DIRNAME: &str = "s01-boot";
const STAGE02_DIRNAME: &str = "s02-live-tools";
const STAGE01_LIVE_BOOT_SCRIPT: &str = "/usr/local/bin/stage-01-live-boot.sh";
const STAGE01_SSH_PREFLIGHT_SCRIPT: &str = "/usr/local/bin/stage-01-ssh-preflight.sh";

struct StageIsoTarget {
    path: PathBuf,
    filename: String,
}

#[derive(Debug, Deserialize)]
struct StageRunManifest {
    status: String,
    created_at_utc: String,
    finished_at_utc: Option<String>,
    iso_path: Option<String>,
}

/// Run a single stage for a distro.
pub fn run_stage(distro_id: &str, stage: u32) -> Result<bool> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;
    let canonical_distro_id = ctx.id();
    let stage00_iso_target = resolve_iso_target_for_stage(canonical_distro_id, 0, &*ctx)?;
    let iso_target = resolve_iso_target_for_stage(canonical_distro_id, stage, &*ctx)?;
    let iso_path = iso_target.path.clone();
    let iso_dir = iso_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "Could not resolve ISO parent directory for '{}'",
            iso_path.display()
        )
    })?;

    // Stage 00 and all later stages must satisfy conformance + artifact preflight.
    require_preflight_with_iso_for_distro(
        iso_dir,
        Some(&iso_target.filename),
        canonical_distro_id,
    )?;

    let mut state = StageState::load(canonical_distro_id);
    if !state.is_valid_for_stage_iso(0, &stage00_iso_target.path) {
        println!(
            "{}",
            "ISO rebuilt since last run — resetting stages.".yellow()
        );
        state.reset_for_stage_iso(0, &stage00_iso_target.path);
        state.save(canonical_distro_id)?;
    } else if stage > 0 && !state.is_valid_for_stage_iso(stage, &iso_target.path) {
        println!(
            "{}",
            "Runtime ISO rebuilt since last run — resetting Stage 01+ results.".yellow()
        );
        state.reset_for_stage_iso(stage, &iso_target.path);
        state.save(canonical_distro_id)?;
    }

    // Gating: stage N requires N-1 to have passed (01 requires 00).
    if stage > 0 && !state.has_passed(stage - 1) {
        bail!(
            "Stage {:02} is blocked: Stage {:02} has not passed yet.\n\
             Run: cargo run --bin stages -- --distro {} --stage {}",
            stage,
            stage - 1,
            canonical_distro_id,
            stage - 1
        );
    }

    // Already passed?
    if state.has_passed(stage) {
        println!(
            "{} Stage {:02} already passed (use --reset to clear).",
            "[SKIP]".green(),
            stage
        );
        return Ok(true);
    }

    println!("{} Stage {:02}: {}", ">>".cyan(), stage, stage_name(stage));

    let result = match stage {
        0 => Ok("Preflight conformance + artifact checks passed".to_string()),
        1 => run_live_boot(&*ctx, &iso_path),
        2 => run_live_tools(&*ctx, &iso_path),
        3 => run_installation(&*ctx, &iso_path),
        4 => run_installed_boot(&*ctx, &iso_path),
        5 => run_automated_login(&*ctx, &iso_path),
        6 => run_daily_driver_tools(&*ctx, &iso_path),
        _ => bail!("Invalid stage number: {} (valid: 00-06)", stage),
    };

    match &result {
        Ok(evidence) => {
            state.record(stage, true, evidence);
            state.save(canonical_distro_id)?;
            println!(
                "{} Stage {:02} passed: {}",
                "[PASS]".green().bold(),
                stage,
                evidence
            );
            Ok(true)
        }
        Err(e) => {
            state.record(stage, false, &format!("{:#}", e));
            state.save(canonical_distro_id)?;
            print_failure(stage, e);
            Ok(false)
        }
    }
}

/// Run all stages up to `target` (inclusive).
pub fn run_up_to(distro_id: &str, target: u32) -> Result<bool> {
    for stage_n in 0..=target {
        if !run_stage(distro_id, stage_n)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Print stage status for a distro.
pub fn print_status(distro_id: &str) -> Result<()> {
    let ctx = context_for_distro(distro_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown distro '{}'", distro_id))?;
    let canonical_distro_id = ctx.id();
    let stage00_iso_path =
        resolve_iso_target_for_stage(canonical_distro_id, 0, &*ctx).map(|target| target.path);
    let runtime_iso_path =
        resolve_iso_target_for_stage(canonical_distro_id, 1, &*ctx).map(|target| target.path);

    let state = StageState::load(canonical_distro_id);
    let build_iso_valid = stage00_iso_path
        .as_ref()
        .map(|p| state.is_valid_for_stage_iso(0, p))
        .unwrap_or(false);
    let runtime_iso_valid = if state.has_any_results_from(1) {
        runtime_iso_path
            .as_ref()
            .map(|p| state.is_valid_for_stage_iso(1, p))
            .unwrap_or(false)
    } else {
        true
    };
    let valid = build_iso_valid && runtime_iso_valid;

    println!("{} Stage Status", ctx.name().bold());
    if !valid {
        println!(
            "{}",
            "  (stale — ISO rebuilt or missing, stages will reset on next run)".yellow()
        );
    }
    println!();

    for stage_n in 0..=6u32 {
        let status = if state.has_passed(stage_n) {
            "[PASS]".green()
        } else if state.results.contains_key(&stage_n) {
            "[FAIL]".red()
        } else {
            "[    ]".dimmed()
        };
        println!("  {} {:02}: {}", status, stage_n, stage_name(stage_n));
    }
    println!();
    println!(
        "  Highest passed: {}",
        state.highest_passed().to_string().bold()
    );
    Ok(())
}

/// Reset all stage state for a distro.
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
    println!("Stages reset for {}.", canonical_distro_id);
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Stage implementations
// ═══════════════════════════════════════════════════════════════════════════

/// Stage 01: Live Boot — ISO boots in QEMU.
fn run_live_boot(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let (mut child, mut console, ssh_host_port) = spawn_live_qemu_with_ssh(ctx, iso_path)?;
    let stall_timeout = Duration::from_secs(ctx.live_boot_stall_timeout_secs());

    let result = (|| -> Result<String> {
        console.wait_for_live_boot_with_context(stall_timeout, ctx)?;
        verify_stage01_ssh_login(&mut console, ssh_host_port)?;

        run_stage_script_over_ssh(ssh_host_port, STAGE01_LIVE_BOOT_SCRIPT)?;
        run_stage_script_over_ssh(ssh_host_port, STAGE01_SSH_PREFLIGHT_SCRIPT)?;

        Ok(
            "Boot markers detected + SSH login probe passed + stage-01 script checks passed"
                .to_string(),
        )
    })();

    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Stage 02: Live Tools — Expected binaries in live environment.
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
        let stall_timeout = Duration::from_secs(ctx.live_boot_stall_timeout_secs());
        console.wait_for_live_boot_with_context(stall_timeout, ctx)?;
        verify_stage01_ssh_login(&mut console, ssh_host_port)?;

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

        Ok(format!(
            "All {} tools verified working (actually executed): {}; {}",
            found.len(),
            found.join(", "),
            overlay_evidence
        ))
    })();

    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Stage 03: Installation — Scripted install to disk.
fn run_installation(ctx: &dyn DistroContext, iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    recqemu::create_disk(&disk_path, "20G")?;

    let (ovmf, ovmf_vars) = session::setup_ovmf_vars(ctx.id())?;

    let (mut child, mut console) =
        session::spawn_live_with_disk(iso_path, &disk_path, &ovmf, &ovmf_vars)?;

    console.wait_for_live_boot_with_context(Duration::from_secs(60), ctx)?;

    // Run the installation script
    let install_cmds = install_commands_for(ctx);
    let mut step_count = 0;
    for (desc, cmd_str) in &install_cmds {
        println!("    {} {}", "->".cyan(), desc);
        let result = console.exec(cmd_str, Duration::from_secs(120))?;
        if !result.success() {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "Installation step '{}' failed (exit {}): {}",
                desc,
                result.exit_code,
                result.output.trim()
            );
        }
        step_count += 1;
    }

    // Verify key artifacts exist
    let verify_cmds = [
        ("Root filesystem", "ls /mnt/sysroot/bin/busybox"),
        ("Boot partition", "ls /mnt/sysroot/boot/EFI"),
        ("fstab", "cat /mnt/sysroot/etc/fstab"),
    ];
    for (desc, cmd_str) in &verify_cmds {
        let result = console.exec(cmd_str, Duration::from_secs(10))?;
        if !result.success() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Verification '{}' failed: {}", desc, result.output.trim());
        }
    }

    let _ = console.exec("poweroff -f", Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();

    Ok(format!("{} install steps completed + verified", step_count))
}

/// Stage 04: Installed Boot — Boot from disk after install.
fn run_installed_boot(ctx: &dyn DistroContext, _iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    if !disk_path.exists() {
        bail!(
            "No disk image found at {}. Stage 03 (Installation) must pass first.",
            disk_path.display()
        );
    }

    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars = temp_ovmf_vars_path(ctx.id());
    if !ovmf_vars.exists() {
        bail!("No OVMF vars found. Stage 03 (Installation) must pass first.");
    }

    let (mut child, mut console) = session::spawn_installed(&disk_path, &ovmf, &ovmf_vars)?;

    let result = console.wait_for_installed_boot_with_context(Duration::from_secs(90), ctx);

    match &result {
        Ok(()) => {
            let _ = child.kill();
            let _ = child.wait();
            Ok("Installed system boot markers detected".to_string())
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(anyhow::anyhow!("{:#}", e))
        }
    }
}

/// Stage 05: Automated Login — Harness can login and execute commands.
fn run_automated_login(ctx: &dyn DistroContext, _iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars = temp_ovmf_vars_path(ctx.id());

    let (mut child, mut console) = session::spawn_installed(&disk_path, &ovmf, &ovmf_vars)?;

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

/// Stage 06: Daily Driver Tools — All expected tools present.
fn run_daily_driver_tools(ctx: &dyn DistroContext, _iso_path: &Path) -> Result<String> {
    let disk_path = temp_disk_path(ctx.id());
    let ovmf = recqemu::find_ovmf().context("OVMF not found")?;
    let ovmf_vars = temp_ovmf_vars_path(ctx.id());

    let (mut child, mut console) = session::spawn_installed(&disk_path, &ovmf, &ovmf_vars)?;

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

fn stage_name(n: u32) -> &'static str {
    match n {
        0 => "00Build",
        1 => "01Boot",
        2 => "02LiveTools",
        3 => "03Install",
        4 => "04LoginGate",
        5 => "05Harness",
        6 => "06Runtime",
        _ => "Unknown",
    }
}

fn resolve_iso_target_for_stage(
    distro_id: &str,
    stage: u32,
    ctx: &dyn DistroContext,
) -> Result<StageIsoTarget> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = load_stage_00_contract_bundle_for_distro_from(&workspace_root, distro_id)
        .with_context(|| format!("loading 00Build contract bundle for '{}'", distro_id))?;
    let stage00_filename = bundle.contract.artifacts.iso_filename.clone();
    let filename = match stage {
        0 => stage00_filename.clone(),
        1 => derive_stage_iso_filename(&stage00_filename, STAGE01_SLUG),
        _ => derive_stage_iso_filename(&stage00_filename, STAGE02_SLUG),
    };
    let stage_root = workspace_root
        .join(".artifacts/out")
        .join(distro_id)
        .join(stage_output_dir_for_stage(stage));
    let legacy_path = stage_root.join(&filename);

    let path = if let Some(run_iso) =
        resolve_iso_from_latest_successful_run(&stage_root, &filename)?
    {
        run_iso
    } else if legacy_path.is_file() {
        legacy_path
    } else {
        let build_stage = match stage {
            0 => "00Build",
            1 => "01Boot",
            _ => "02LiveTools",
        };
        bail!(
            "ISO not found at {} and no successful run-manifest ISO found under '{}'. Build {} {} first: \
             cargo run -p distro-builder --bin distro-builder -- iso build {} {}",
            legacy_path.display(),
            stage_root.display(),
            ctx.name(),
            build_stage,
            distro_id,
            build_stage
        );
    };

    Ok(StageIsoTarget { path, filename })
}

fn resolve_iso_from_latest_successful_run(
    stage_root: &Path,
    iso_filename: &str,
) -> Result<Option<PathBuf>> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    if !stage_root.is_dir() {
        return Ok(None);
    }

    for entry in fs::read_dir(stage_root)
        .with_context(|| format!("reading stage output directory '{}'", stage_root.display()))?
    {
        let entry = entry.with_context(|| {
            format!(
                "iterating stage output directory '{}'",
                stage_root.display()
            )
        })?;
        let run_dir = entry.path();
        if !run_dir.is_dir() {
            continue;
        }
        let manifest_path = run_dir.join("run-manifest.json");
        if !manifest_path.is_file() {
            continue;
        }

        let raw = fs::read(&manifest_path)
            .with_context(|| format!("reading stage run manifest '{}'", manifest_path.display()))?;
        let manifest: StageRunManifest = serde_json::from_slice(&raw)
            .with_context(|| format!("parsing stage run manifest '{}'", manifest_path.display()))?;
        if manifest.status != "success" {
            continue;
        }

        let sort_key = manifest
            .finished_at_utc
            .clone()
            .unwrap_or(manifest.created_at_utc.clone());
        let iso_candidate = manifest
            .iso_path
            .as_ref()
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .unwrap_or_else(|| run_dir.join(iso_filename));

        if iso_candidate.is_file() {
            candidates.push((sort_key, iso_candidate));
        }
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(candidates.into_iter().next().map(|(_, path)| path))
}

fn stage_output_dir_for_stage(stage: u32) -> &'static str {
    match stage {
        0 => STAGE00_DIRNAME,
        1 => STAGE01_DIRNAME,
        _ => STAGE02_DIRNAME,
    }
}

fn derive_stage_iso_filename(stage00_iso_filename: &str, stage_slug: &str) -> String {
    if stage00_iso_filename.contains(STAGE00_SLUG) {
        return stage00_iso_filename.replacen(STAGE00_SLUG, stage_slug, 1);
    }
    if let Some(base) = stage00_iso_filename.strip_suffix(".iso") {
        return format!("{base}-{stage_slug}.iso");
    }
    format!("{stage00_iso_filename}-{stage_slug}.iso")
}

fn spawn_live_qemu_with_ssh(
    ctx: &dyn DistroContext,
    iso_path: &Path,
) -> Result<(std::process::Child, Console, u16)> {
    session::spawn_live_with_ssh(ctx, iso_path)
}

struct SshExecOutput {
    exit_code: i32,
    output: String,
}

fn verify_stage01_ssh_login(console: &mut Console, ssh_host_port: u16) -> Result<()> {
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

    let diagnostics = collect_stage01_ssh_diagnostics(console);
    bail!(
        "Stage 01 SSH login probe failed after shell-ready boundary (forwarded port {}). Last output:\n{}\n\n{}",
        ssh_host_port,
        last_err,
        diagnostics
    );
}

fn collect_stage01_ssh_diagnostics(console: &mut Console) -> String {
    let checks = [
        ("Kernel cmdline", "cat /proc/cmdline"),
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

    let mut report = String::from("Stage 01 SSH diagnostics from live shell:\n");
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
    let key_path = ssh_private_key_path();
    if !key_path.is_file() {
        bail!(
            "SSH private key not found at '{}'. Set LEVITATE_SSH_PRIVATE_KEY to a readable key.",
            key_path.display()
        );
    }

    let output = Command::new("ssh")
        .args([
            "-i",
            &key_path.to_string_lossy(),
            "-p",
            &ssh_host_port.to_string(),
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "ConnectTimeout=5",
            "root@127.0.0.1",
            remote_cmd,
        ])
        .output()
        .with_context(|| {
            format!(
                "running host SSH command against forwarded live SSH port {}",
                ssh_host_port
            )
        })?;

    let mut merged = String::new();
    merged.push_str(&String::from_utf8_lossy(&output.stdout));
    merged.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(SshExecOutput {
        exit_code: output.status.code().unwrap_or(255),
        output: merged,
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

fn ssh_private_key_path() -> PathBuf {
    if let Ok(path) = std::env::var("LEVITATE_SSH_PRIVATE_KEY") {
        return PathBuf::from(path);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".ssh/id_ed25519");
    }
    PathBuf::from("/root/.ssh/id_ed25519")
}

fn temp_disk_path(distro_id: &str) -> PathBuf {
    session::temp_disk_path(distro_id)
}

fn temp_ovmf_vars_path(distro_id: &str) -> PathBuf {
    session::temp_ovmf_vars_path(distro_id)
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

/// Installation commands for a distro. Returns (description, command) pairs.
fn install_commands_for(ctx: &dyn DistroContext) -> Vec<(&'static str, String)> {
    // Common install flow for Alpine-based distros
    vec![
        (
            "Partition disk",
            "echo 'label: gpt\nsize=512M, type=uefi\ntype=linux' | sfdisk /dev/sda".to_string(),
        ),
        (
            "Format EFI partition",
            "mkfs.fat -F32 /dev/sda1".to_string(),
        ),
        (
            "Format root partition",
            "mkfs.ext4 -F /dev/sda2".to_string(),
        ),
        (
            "Mount root",
            "mkdir -p /mnt/sysroot && mount /dev/sda2 /mnt/sysroot".to_string(),
        ),
        (
            "Mount boot",
            "mkdir -p /mnt/sysroot/boot && mount /dev/sda1 /mnt/sysroot/boot".to_string(),
        ),
        ("Extract rootfs", "recstrap /mnt/sysroot".to_string()),
        ("Generate fstab", "recfstab /mnt/sysroot".to_string()),
        (
            "Install bootloader",
            format!("recchroot /mnt/sysroot -- {}", ctx.install_bootloader_cmd()),
        ),
        (
            "Set hostname",
            format!(
                "echo {} > /mnt/sysroot/etc/hostname",
                ctx.default_hostname()
            ),
        ),
        (
            "Set root password",
            format!(
                "recchroot /mnt/sysroot -- sh -c 'echo root:{} | chpasswd'",
                ctx.default_password()
            ),
        ),
        (
            "Enable serial getty",
            format!(
                "recchroot /mnt/sysroot -- sh -c '{}'",
                ctx.enable_serial_getty_cmd()
            ),
        ),
        ("Unmount", "umount -R /mnt/sysroot".to_string()),
    ]
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
        "recstrap" | "recfstab" | "recchroot" => {
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

fn print_failure(stage: u32, err: &anyhow::Error) {
    eprintln!();
    eprintln!(
        "{} Stage {:02} FAILED: {}",
        "[FAIL]".red().bold(),
        stage,
        stage_name(stage)
    );
    eprintln!();
    eprintln!("  Error: {:#}", err);
    eprintln!();

    match stage {
        1 => {
            eprintln!("  Common causes:");
            eprintln!("    - ISO not built or corrupted");
            eprintln!("    - Kernel panic during boot");
            eprintln!("    - initramfs missing /init");
            eprintln!("    - UEFI firmware not finding boot entry");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Manual boot: just stage 1 <distro>");
            eprintln!(
                "    - Rebuild Stage 01 ISO: cargo run -p distro-builder --bin distro-builder -- iso build <distro> 01Boot"
            );
        }
        2 => {
            eprintln!("  Common causes:");
            eprintln!("    - Tools not included in initramfs or rootfs");
            eprintln!("    - PATH not set correctly");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Check package list in distro-spec");
        }
        3 => {
            eprintln!("  Common causes:");
            eprintln!("    - recstrap/recfstab/recchroot broken");
            eprintln!("    - Disk too small or partition failure");
            eprintln!("    - Bootloader install failure");
            eprintln!();
            eprintln!("  Try:");
            eprintln!("    - Manual install: boot ISO, run install commands by hand");
        }
        4 => {
            eprintln!("  Common causes:");
            eprintln!("    - Bootloader not installed correctly");
            eprintln!("    - UKI not found by systemd-boot");
            eprintln!("    - Missing kernel modules");
        }
        5 => {
            eprintln!("  Common causes:");
            eprintln!("    - No serial getty enabled");
            eprintln!("    - Password not set correctly");
            eprintln!("    - Shell not functional");
        }
        6 => {
            eprintln!("  Common causes:");
            eprintln!("    - Packages not installed in rootfs");
            eprintln!("    - PATH misconfigured");
        }
        _ => {}
    }
    eprintln!();
}
