//! Preflight verification for install tests.
//!
//! Runs BEFORE starting QEMU to catch issues early.
//! Verifies that ISO artifacts are correctly built and contain expected components.
//!
//! # Why Preflight?
//!
//! Starting QEMU, waiting for boot, and then discovering a broken initramfs
//! wastes significant time. Preflight catches:
//!
//! - Missing binaries (systemd, mount, switch_root, etc.)
//! - Missing systemd units
//! - Broken symlinks (especially library symlinks)
//! - Missing udev rules (critical for device discovery)
//!
//! If preflight fails, we know the ISO is broken WITHOUT waiting for QEMU.

use anyhow::{Context, Result};
use colored::Colorize;
use distro_builder::stages::s00_build::{
    check_kernel_installed_via_recipe, run_00build_evidence_script, S00BuildEvidenceSpec,
    S00BuildKernelSpec,
};
use distro_contract::{
    load_stage_00_contract_bundle_for_distro_from, require_valid_contract,
    validate_stage_00_runtime,
};
use fsdbg::checklist::{ChecklistType, VerificationReport};
use fsdbg::cpio::CpioReader;
use fsdbg::iso::IsoReader;
use leviso_cheat_guard::cheat_bail;
use std::path::Path;

/// Result of preflight verification
#[derive(Debug)]
pub struct PreflightResult {
    pub conformance: Option<PreflightCheck>,
    pub live_initramfs: Option<PreflightCheck>,
    pub install_initramfs: Option<PreflightCheck>,
    pub iso: Option<PreflightCheck>,
    pub overall_pass: bool,
}

/// Result of a single preflight check
#[derive(Debug)]
pub struct PreflightCheck {
    pub name: String,
    pub passed: bool,
    pub total_checks: usize,
    pub passed_checks: usize,
    pub failures: usize,
    pub details: Vec<String>,
}

impl PreflightCheck {
    fn from_report(name: &str, report: &VerificationReport) -> Self {
        let mut details = Vec::new();

        // Collect failures
        for result in &report.results {
            if !result.passed {
                let msg = result.message.as_deref().unwrap_or("Missing");
                details.push(format!("FAIL: {} - {}", result.item, msg));
            }
        }

        Self {
            name: name.to_string(),
            passed: report.is_success(),
            total_checks: report.total(),
            passed_checks: report.passed(),
            failures: report.failed(),
            details,
        }
    }
}

/// Run preflight verification on ISO artifacts.
///
/// This should be called BEFORE starting QEMU to catch issues early.
///
/// # Arguments
/// * `iso_dir` - Directory containing ISO artifacts (e.g. `.artifacts/out/levitate/`)
///
/// # Returns
/// * `Ok(PreflightResult)` - Verification completed (check `overall_pass`)
/// * `Err` - Could not run verification (missing files, etc.)
pub fn run_preflight(iso_dir: &Path) -> Result<PreflightResult> {
    run_preflight_with_iso(iso_dir, None)
}

/// Run preflight verification for a specific distro.
pub fn run_preflight_for_distro(iso_dir: &Path, distro_id: &str) -> Result<PreflightResult> {
    run_preflight_with_iso_distro(iso_dir, None, distro_id)
}

/// Run preflight verification with a specific ISO filename.
///
/// # Arguments
/// * `iso_dir` - Directory containing ISO artifacts
/// * `iso_filename` - Optional specific ISO filename. If None, searches for any .iso file.
///
/// # Returns
/// * `Ok(PreflightResult)` - Verification completed (check `overall_pass`)
/// * `Err` - Could not run verification (missing files, etc.)
pub fn run_preflight_with_iso(
    iso_dir: &Path,
    iso_filename: Option<&str>,
) -> Result<PreflightResult> {
    run_preflight_with_iso_distro(iso_dir, iso_filename, "levitate")
}

/// Run preflight verification with a specific ISO filename and distro context.
pub fn run_preflight_with_iso_distro(
    iso_dir: &Path,
    iso_filename: Option<&str>,
    distro_id: &str,
) -> Result<PreflightResult> {
    println!();
    println!("{}", "=== PREFLIGHT VERIFICATION ===".cyan().bold());
    println!(
        "Verifying contract + ISO artifacts for {} before starting QEMU...",
        distro_id
    );
    println!();

    let mut result = PreflightResult {
        conformance: None,
        live_initramfs: None,
        install_initramfs: None,
        iso: None,
        overall_pass: true,
    };

    result.conformance = Some(verify_conformance_contract(iso_dir, distro_id)?);
    if !result.conformance.as_ref().unwrap().passed {
        result.overall_pass = false;
    }

    // Check live initramfs
    let live_path = iso_dir.join("initramfs-live.cpio.gz");
    if live_path.exists() {
        result.live_initramfs = Some(verify_artifact(&live_path, ChecklistType::LiveInitramfs)?);
        if !result.live_initramfs.as_ref().unwrap().passed {
            result.overall_pass = false;
        }
    } else {
        println!(
            "  {} Live initramfs not found at {}",
            "SKIP".yellow(),
            live_path.display()
        );
    }

    // Check install initramfs (only LevitateOS builds this)
    if distro_id == "levitate" {
        let install_path = iso_dir.join("initramfs-installed.img");
        if install_path.exists() {
            result.install_initramfs = Some(verify_artifact(
                &install_path,
                ChecklistType::InstallInitramfs,
            )?);
            if !result.install_initramfs.as_ref().unwrap().passed {
                result.overall_pass = false;
            }
        } else {
            println!(
                "  {} Install initramfs not found at {}",
                "SKIP".yellow(),
                install_path.display()
            );
        }
    }

    // Check ISO with full content verification
    // Support multi-distro by trying specified filename first, then searching
    let iso_path = if let Some(filename) = iso_filename {
        iso_dir.join(filename)
    } else {
        // Fallback: try to find any .iso file in the directory
        match find_iso_file(iso_dir) {
            Some(path) => path,
            None => {
                println!(
                    "  {} No .iso file found in {}",
                    "✗".red(),
                    iso_dir.display()
                );
                result.overall_pass = false;
                println!();
                print_summary(&result);
                return Ok(result);
            }
        }
    };

    if iso_path.exists() {
        result.iso = Some(verify_iso_distro(&iso_path, distro_id)?);
        if !result.iso.as_ref().unwrap().passed {
            result.overall_pass = false;
        }
    } else {
        println!("  {} ISO not found at {}", "✗".red(), iso_path.display());
        result.overall_pass = false;
    }

    println!();

    // Print summary
    print_summary(&result);

    Ok(result)
}

fn verify_conformance_contract(iso_dir: &Path, distro_id: &str) -> Result<PreflightCheck> {
    let name = "Contract conformance";
    print!("  Checking {}... ", name);

    let bundle = match load_stage_00_contract_bundle_for_distro_from(iso_dir, distro_id) {
        Ok(bundle) => bundle,
        Err(err) => {
            println!("{}", "FAIL".red().bold());
            return Ok(PreflightCheck {
                name: name.to_string(),
                passed: false,
                total_checks: 1,
                passed_checks: 0,
                failures: 1,
                details: vec![err.to_string()],
            });
        }
    };

    let mut details = Vec::new();

    if let Err(err) = require_valid_contract(&bundle.contract) {
        details.extend(
            err.report
                .violations
                .into_iter()
                .map(|v| format!("{:?}.{} [{:?}] {}", v.stage, v.field, v.code, v.message)),
        );
    }

    let runtime_report = validate_stage_00_runtime(&bundle.contract, &bundle.variant_dir, iso_dir);
    details.extend(
        runtime_report
            .violations
            .into_iter()
            .map(|v| format!("{:?}.{} [{:?}] {}", v.stage, v.field, v.code, v.message)),
    );

    if let Err(err) = verify_kernel_recipe_is_installed(&bundle, iso_dir) {
        details.push(err);
    }
    if let Err(err) = verify_stage_00_evidence_script(&bundle, iso_dir) {
        details.push(err);
    }

    if details.is_empty() {
        println!("{} (declaration + runtime)", "PASS".green());
        Ok(PreflightCheck {
            name: name.to_string(),
            passed: true,
            total_checks: 2,
            passed_checks: 2,
            failures: 0,
            details: Vec::new(),
        })
    } else {
        println!("{} ({} violations)", "FAIL".red().bold(), details.len());
        for detail in &details {
            println!("    {}", detail.red());
        }
        Ok(PreflightCheck {
            name: name.to_string(),
            passed: false,
            total_checks: details.len(),
            passed_checks: 0,
            failures: details.len(),
            details,
        })
    }
}

fn verify_kernel_recipe_is_installed(
    bundle: &distro_contract::LoadedVariantContract,
    iso_dir: &Path,
) -> Result<(), String> {
    let stage_00 = &bundle.contract.stages.stage_00_build;
    let spec = S00BuildKernelSpec {
        recipe_kernel_script: stage_00.recipe_kernel_script.clone(),
        kernel_version: stage_00.kernel_version.clone(),
        kernel_sha256: stage_00.kernel_sha256.clone(),
        kernel_localversion: stage_00.kernel_localversion.clone(),
        module_install_path: stage_00.module_install_path.clone(),
    };

    check_kernel_installed_via_recipe(&bundle.repo_root, iso_dir, &spec).map_err(|e| {
        format!(
            "Stage00.recipe_isinstalled [RecipeKernelOrchestrationRequired] {}",
            e
        )
    })
}

fn verify_stage_00_evidence_script(
    bundle: &distro_contract::LoadedVariantContract,
    iso_dir: &Path,
) -> Result<(), String> {
    let stage_00 = &bundle.contract.stages.stage_00_build;
    let spec = S00BuildEvidenceSpec {
        script_path: stage_00.evidence.script_path.clone(),
        pass_marker: stage_00.evidence.pass_marker.clone(),
        kernel_release_path: stage_00.kernel_release_path.clone(),
        kernel_image_path: stage_00.kernel_image_path.clone(),
        iso_filename: bundle.contract.artifacts.iso_filename.clone(),
    };

    run_00build_evidence_script(&bundle.repo_root, &bundle.variant_dir, iso_dir, &spec)
        .map_err(|e| format!("Stage00.evidence [InvalidEvidenceDeclaration] {}", e))
}

/// Verify an ISO using distro-specific checklist.
fn verify_iso_distro(path: &Path, distro_id: &str) -> Result<PreflightCheck> {
    let name = "Live ISO";
    print!("  Checking {}... ", name);

    let reader = match IsoReader::open(path) {
        Ok(r) => r,
        Err(e) => {
            println!("{}", "FAIL".red().bold());
            println!("    Failed to read ISO: {}", e);
            return Ok(PreflightCheck {
                name: name.to_string(),
                passed: false,
                total_checks: 0,
                passed_checks: 0,
                failures: 1,
                details: vec![format!("Failed to read ISO: {}", e)],
            });
        }
    };
    // TODO: pass distro_id once verify_distro is implemented in fsdbg
    let _ = distro_id;
    let report = fsdbg::checklist::iso::verify(&reader);

    let check = PreflightCheck::from_report(name, &report);

    // Print inline result
    if check.passed {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let size_mb = size / (1024 * 1024);
        println!(
            "{} ({}/{} checks, {} MB)",
            "PASS".green(),
            check.passed_checks,
            check.total_checks,
            size_mb
        );
    } else {
        println!(
            "{} ({}/{} checks, {} failed)",
            "FAIL".red().bold(),
            check.passed_checks,
            check.total_checks,
            check.failures
        );
        for detail in &check.details {
            println!("    {}", detail.red());
        }
    }

    Ok(check)
}

/// Find any .iso file in the given directory.
///
/// Returns the first .iso file found (for multi-distro support).
fn find_iso_file(dir: &Path) -> Option<std::path::PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(path) = entry.path().canonicalize() {
                    if path.extension().and_then(|s| s.to_str()) == Some("iso") {
                        return Some(path);
                    }
                }
            }
            None
        }
        Err(_) => None,
    }
}

/// Verify an artifact against its checklist.
///
/// Handles CPIO (initramfs) and ISO formats based on the checklist type.
fn verify_artifact(path: &Path, checklist_type: ChecklistType) -> Result<PreflightCheck> {
    let name = checklist_type.name();
    print!("  Checking {}... ", name);

    let report = match checklist_type {
        ChecklistType::InstallInitramfs => {
            let reader = CpioReader::open(path)
                .with_context(|| format!("Failed to open {}: {}", name, path.display()))?;
            fsdbg::checklist::install_initramfs::verify(&reader)
        }
        ChecklistType::LiveInitramfs => {
            let reader = CpioReader::open(path)
                .with_context(|| format!("Failed to open {}: {}", name, path.display()))?;
            fsdbg::checklist::live_initramfs::verify(&reader)
        }
        ChecklistType::Rootfs => {
            let reader = CpioReader::open(path)
                .with_context(|| format!("Failed to open {}: {}", name, path.display()))?;
            fsdbg::checklist::rootfs::verify(&reader)
        }
        ChecklistType::Iso => {
            let reader = match IsoReader::open(path) {
                Ok(r) => r,
                Err(e) => {
                    println!("{}", "FAIL".red().bold());
                    println!("    Failed to read ISO: {}", e);
                    return Ok(PreflightCheck {
                        name: name.to_string(),
                        passed: false,
                        total_checks: 0,
                        passed_checks: 0,
                        failures: 1,
                        details: vec![format!("Failed to read ISO: {}", e)],
                    });
                }
            };
            fsdbg::checklist::iso::verify(&reader)
        }
        ChecklistType::AuthAudit | ChecklistType::Qcow2 => {
            // These checklist types are not used in preflight verification
            return Ok(PreflightCheck {
                name: name.to_string(),
                passed: true,
                total_checks: 0,
                passed_checks: 0,
                failures: 0,
                details: vec![format!(
                    "Checklist type {} not applicable for preflight",
                    name
                )],
            });
        }
    };

    let check = PreflightCheck::from_report(name, &report);

    // Print inline result
    if check.passed {
        if checklist_type == ChecklistType::Iso {
            // Also show size for ISO
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let size_mb = size / (1024 * 1024);
            println!(
                "{} ({}/{} checks, {} MB)",
                "PASS".green(),
                check.passed_checks,
                check.total_checks,
                size_mb
            );
        } else {
            println!(
                "{} ({}/{} checks)",
                "PASS".green(),
                check.passed_checks,
                check.total_checks
            );
        }
    } else {
        println!(
            "{} ({}/{} checks, {} failed)",
            "FAIL".red().bold(),
            check.passed_checks,
            check.total_checks,
            check.failures
        );
        // Show failures
        for detail in &check.details {
            println!("    {}", detail.red());
        }
    }

    Ok(check)
}

/// Print the overall summary.
fn print_summary(result: &PreflightResult) {
    println!("{}", "--- Preflight Summary ---".bold());

    let status = if result.overall_pass {
        "PASS".green().bold()
    } else {
        "FAIL".red().bold()
    };

    println!("Overall: {}", status);

    if !result.overall_pass {
        println!();
        println!(
            "{}",
            "Preflight verification failed. Fix the issues above before running tests.".red()
        );
        println!(
            "{}",
            "The ISO artifacts are broken and will not work correctly.".red()
        );
    }
}

/// Run preflight and fail if critical issues found.
///
/// This is a convenience function for tests that should abort on preflight failure.
pub fn require_preflight(iso_dir: &Path) -> Result<()> {
    require_preflight_for_distro(iso_dir, "levitate")
}

/// Run preflight for a distro and fail if critical issues found.
pub fn require_preflight_for_distro(iso_dir: &Path, distro_id: &str) -> Result<()> {
    require_preflight_with_iso_for_distro(iso_dir, None, distro_id)
}

/// Run preflight for a distro + explicit ISO filename and fail if critical issues found.
pub fn require_preflight_with_iso_for_distro(
    iso_dir: &Path,
    iso_filename: Option<&str>,
    distro_id: &str,
) -> Result<()> {
    let result = run_preflight_with_iso_distro(iso_dir, iso_filename, distro_id)?;

    if !result.overall_pass {
        // Collect all failures for the error message
        let mut all_failures = Vec::new();
        if let Some(ref check) = result.conformance {
            if !check.passed {
                all_failures.extend(check.details.iter().cloned());
            }
        }
        if let Some(ref check) = result.live_initramfs {
            if !check.passed {
                all_failures.extend(check.details.iter().cloned());
            }
        }
        if let Some(ref check) = result.install_initramfs {
            if !check.passed {
                all_failures.extend(check.details.iter().cloned());
            }
        }
        if let Some(ref check) = result.iso {
            if !check.passed {
                all_failures.extend(check.details.iter().cloned());
            }
        }

        cheat_bail!(
            protects = "Installation tests verify REAL artifacts, not broken/incomplete ones",
            severity = "CRITICAL",
            cheats = [
                "Skip preflight verification entirely",
                "Mark missing items as optional",
                "Remove items from required lists",
                "Return Ok() when overall_pass is false",
                "Lower severity of check failures"
            ],
            consequence = "Tests pass with broken artifacts. Users download and burn a non-functional ISO.",
            "Preflight verification failed. Cannot run installation tests with broken artifacts.\n\n\
             Failures:\n{}\n\n\
             Run 'cargo run -p distro-builder --bin distro-builder -- iso build {} 00Build' to rebuild the ISO.\n\
             ALL verification checks must pass before running tests.",
            all_failures.join("\n"),
            distro_id
        );
    }

    Ok(())
}
