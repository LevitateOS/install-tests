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
use distro_builder::build_host::{
    check_kernel_preinstalled_via_recipe, run_build_host_evidence_script, BuildHostEvidenceSpec,
    BuildHostKernelSpec,
};
use distro_contract::{
    load_variant_contract_bundle_for_distro_from, require_valid_contract,
    validate_build_runtime_with_artifacts, validate_live_boot_runtime, BuildRuntimeArtifacts,
    LiveBootRuntimeArtifacts,
};
use fsdbg::checklist::{ChecklistType, VerificationReport};
use fsdbg::cpio::CpioReader;
use fsdbg::iso::IsoReader;
use leviso_cheat_guard::cheat_bail;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct RunManifest {
    status: String,
    created_at_utc: String,
    finished_at_utc: Option<String>,
    iso_path: Option<String>,
    target_kind: Option<String>,
    target_name: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedRuntimeArtifacts {
    rootfs_image: PathBuf,
    initramfs_live: PathBuf,
    initramfs_installed: Option<PathBuf>,
    overlay_image: PathBuf,
    live_overlay_dir: PathBuf,
    rootfs_source_pointer: PathBuf,
}

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
    run_preflight_with_iso_distro_layout(iso_dir, iso_filename, distro_id)
}

fn verify_conformance_contract(
    _iso_dir: &Path,
    _iso_filename: Option<&str>,
    distro_id: &str,
    runtime_artifacts: &ResolvedRuntimeArtifacts,
    validate_live_boot: bool,
    resolved_iso_path: Option<&Path>,
) -> Result<PreflightCheck> {
    let name = "Contract conformance";
    print!("  Checking {}... ", name);

    let workspace_root = workspace_root();
    let bundle = match load_variant_contract_bundle_for_distro_from(&workspace_root, distro_id) {
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
    let kernel_output_dir = kernel_output_dir_for_distro(distro_id);

    if let Err(err) = require_valid_contract(&bundle.contract) {
        details.extend(
            err.report
                .violations
                .into_iter()
                .map(|v| format!("{:?}.{} [{:?}] {}", v.stage, v.field, v.code, v.message)),
        );
    }

    let runtime_report = validate_build_runtime_with_artifacts(
        &bundle.contract,
        &bundle.variant_dir,
        &kernel_output_dir,
        &BuildRuntimeArtifacts {
            rootfs_image: runtime_artifacts.rootfs_image.clone(),
            initramfs_live: runtime_artifacts.initramfs_live.clone(),
            overlay_image: runtime_artifacts.overlay_image.clone(),
        },
    );
    details.extend(
        runtime_report
            .violations
            .into_iter()
            .map(|v| format!("{:?}.{} [{:?}] {}", v.stage, v.field, v.code, v.message)),
    );

    if validate_live_boot {
        let stage01_report = validate_live_boot_runtime(
            &bundle.contract,
            &LiveBootRuntimeArtifacts {
                rootfs_image: runtime_artifacts.rootfs_image.clone(),
                initramfs_live: runtime_artifacts.initramfs_live.clone(),
                overlay_image: runtime_artifacts.overlay_image.clone(),
                live_overlay_dir: runtime_artifacts.live_overlay_dir.clone(),
                rootfs_source_pointer: runtime_artifacts.rootfs_source_pointer.clone(),
            },
        );
        details.extend(
            stage01_report
                .violations
                .into_iter()
                .map(|v| format!("{:?}.{} [{:?}] {}", v.stage, v.field, v.code, v.message)),
        );
    }

    if let Err(err) = verify_kernel_recipe_is_installed(&bundle, &kernel_output_dir, distro_id) {
        details.push(err);
    }
    if let Err(err) =
        verify_stage_00_evidence_script(&bundle, &kernel_output_dir, distro_id, resolved_iso_path)
    {
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
    kernel_output_dir: &Path,
    distro_id: &str,
) -> Result<(), String> {
    let build = &bundle.contract.build;
    let spec = BuildHostKernelSpec {
        recipe_kernel_script: build.kernel.recipe_script.clone(),
        kernel_kconfig_path: build.kernel.kconfig_path.clone(),
    };

    check_kernel_preinstalled_via_recipe(
        &bundle.repo_root,
        &bundle.variant_dir,
        distro_id,
        kernel_output_dir,
        &spec,
    )
    .map_err(|e| {
        format!(
            "Stage00.recipe_isinstalled [RecipeKernelOrchestrationRequired] {}",
            e
        )
    })
}

fn verify_stage_00_evidence_script(
    bundle: &distro_contract::LoadedVariantContract,
    kernel_output_dir: &Path,
    distro_id: &str,
    resolved_iso_path: Option<&Path>,
) -> Result<(), String> {
    let build = &bundle.contract.build;
    let (run_output_dir, iso_filename) = if let Some(iso_path) = resolved_iso_path {
        let parent = iso_path.parent().ok_or_else(|| {
            format!(
                "Stage00.evidence [InvalidEvidenceDeclaration] ISO path has no parent directory: {}",
                iso_path.display()
            )
        })?;
        let filename = iso_path
            .file_name()
            .and_then(|part| part.to_str())
            .ok_or_else(|| {
                format!(
                    "Stage00.evidence [InvalidEvidenceDeclaration] ISO path has no valid filename: {}",
                    iso_path.display()
                )
            })?
            .to_string();
        (parent.to_path_buf(), filename)
    } else {
        let iso_filename = required_transform_output_name(
            &bundle.contract.transforms.iso.output_names,
            "contract.transforms.iso.output_names",
            distro_id,
        )
        .map_err(|e| format!("Stage00.evidence [InvalidEvidenceDeclaration] {}", e))?;
        let release_root = release_product_root_dir_for_distro(distro_id, "base-rootfs");
        let run_output_dir = resolve_latest_successful_run_dir(&release_root, &iso_filename)
            .map_err(|e| format!("Stage00.evidence [InvalidEvidenceDeclaration] {}", e))?
            .unwrap_or(release_root);
        (run_output_dir, iso_filename)
    };
    let spec = BuildHostEvidenceSpec {
        script_path: build.evidence.script_path.clone(),
        pass_marker: build.evidence.pass_marker.clone(),
        kernel_release_path: build.kernel.release_path.clone(),
        kernel_image_path: build.kernel.image_path.clone(),
        iso_filename,
    };

    run_build_host_evidence_script(
        &bundle.repo_root,
        &bundle.variant_dir,
        kernel_output_dir,
        &run_output_dir,
        &spec,
    )
    .map_err(|e| format!("Stage00.evidence [InvalidEvidenceDeclaration] {}", e))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn distro_output_dir_for_distro(distro_id: &str) -> PathBuf {
    workspace_root().join(".artifacts/out").join(distro_id)
}

fn release_product_root_dir_for_distro(distro_id: &str, product_name: &str) -> PathBuf {
    distro_output_dir_for_distro(distro_id)
        .join("releases")
        .join(product_name)
}

fn kernel_output_dir_for_distro(distro_id: &str) -> PathBuf {
    workspace_root()
        .join(".artifacts/kernel")
        .join(distro_id)
        .join("current")
}

fn resolve_latest_successful_run_dir(
    run_root: &Path,
    iso_filename: &str,
) -> Result<Option<PathBuf>> {
    if !run_root.is_dir() {
        return Ok(None);
    }

    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(run_root)
        .with_context(|| format!("reading run output directory '{}'", run_root.display()))?
    {
        let entry = entry
            .with_context(|| format!("iterating run output directory '{}'", run_root.display()))?;
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
        let manifest: RunManifest = serde_json::from_slice(&raw)
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
            .filter(|p| p.is_file())
            .unwrap_or_else(|| run_dir.join(iso_filename));
        if !iso_candidate.is_file() {
            continue;
        }
        candidates.push((sort_key, run_dir));
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(candidates.into_iter().next().map(|(_, run_dir)| run_dir))
}

fn load_run_manifest(run_dir: &Path) -> Result<Option<RunManifest>> {
    let manifest_path = run_dir.join("run-manifest.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let raw = fs::read(&manifest_path)
        .with_context(|| format!("reading stage run manifest '{}'", manifest_path.display()))?;
    let manifest: RunManifest = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing stage run manifest '{}'", manifest_path.display()))?;
    Ok(Some(manifest))
}

fn should_validate_live_boot_runtime(run_manifest: Option<&RunManifest>) -> bool {
    if let Some(manifest) = run_manifest {
        return manifest.target_kind.as_deref() == Some("release-product")
            && matches!(
                manifest.target_name.as_deref(),
                Some("live-boot") | Some("live-tools")
            );
    }
    false
}

fn resolve_runtime_artifacts(
    artifact_dir: &Path,
    distro_id: &str,
) -> Result<ResolvedRuntimeArtifacts> {
    let canonical_names = canonical_runtime_artifact_names_for_distro(distro_id)?;
    Ok(ResolvedRuntimeArtifacts {
        rootfs_image: resolve_required_file(artifact_dir, &canonical_names.rootfs_image)?,
        initramfs_live: resolve_required_file(artifact_dir, &canonical_names.initramfs_live)?,
        initramfs_installed: resolve_optional_file(
            artifact_dir,
            canonical_names
                .initramfs_installed
                .as_deref()
                .unwrap_or("initramfs-installed.img"),
        )?,
        overlay_image: resolve_required_file(artifact_dir, &canonical_names.overlay_image)?,
        live_overlay_dir: resolve_required_dir(artifact_dir, "live-overlay")?,
        rootfs_source_pointer: resolve_required_file(artifact_dir, ".live-rootfs-source.path")?,
    })
}

fn run_preflight_with_iso_distro_layout(
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
    let resolved_iso_path = iso_filename
        .map(|filename| iso_dir.join(filename))
        .filter(|path| path.is_file())
        .or_else(|| find_iso_file(iso_dir));
    let run_manifest = load_run_manifest(iso_dir)?;
    let runtime_artifacts = resolve_runtime_artifacts(iso_dir, distro_id)?;
    let validate_live_boot = should_validate_live_boot_runtime(run_manifest.as_ref());

    result.conformance = Some(verify_conformance_contract(
        iso_dir,
        iso_filename,
        distro_id,
        &runtime_artifacts,
        validate_live_boot,
        resolved_iso_path.as_deref(),
    )?);
    if !result.conformance.as_ref().unwrap().passed {
        result.overall_pass = false;
    }

    let live_path = runtime_artifacts.initramfs_live.clone();
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

    if distro_id == "levitate" {
        if let Some(install_path) = runtime_artifacts.initramfs_installed.as_ref() {
            if install_path.exists() {
                result.install_initramfs = Some(verify_artifact(
                    install_path,
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
        } else {
            let expected_install_path = canonical_runtime_artifact_names_for_distro(distro_id)?
                .initramfs_installed
                .map(|name| iso_dir.join(name))
                .unwrap_or_else(|| iso_dir.join("initramfs-installed.img"));
            println!(
                "  {} Install initramfs not found at {}",
                "SKIP".yellow(),
                expected_install_path.display()
            );
        }
    }

    let iso_path = if let Some(path) = resolved_iso_path {
        path
    } else {
        println!(
            "  {} No .iso file found in {}",
            "✗".red(),
            iso_dir.display()
        );
        result.overall_pass = false;
        println!();
        print_summary(&result);
        return Ok(result);
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
    print_summary(&result);
    Ok(result)
}

struct CanonicalRuntimeArtifactNames {
    rootfs_image: String,
    initramfs_live: String,
    initramfs_installed: Option<String>,
    overlay_image: String,
}

fn canonical_runtime_artifact_names_for_distro(
    distro_id: &str,
) -> Result<CanonicalRuntimeArtifactNames> {
    let bundle = load_variant_contract_bundle_for_distro_from(&workspace_root(), distro_id)
        .with_context(|| format!("loading variant contract for '{}'", distro_id))?;
    Ok(CanonicalRuntimeArtifactNames {
        rootfs_image: required_transform_output_name(
            &bundle.contract.transforms.rootfs_image.output_names,
            "contract.transforms.rootfs_image.output_names",
            distro_id,
        )?,
        initramfs_live: required_transform_output_name(
            &bundle.contract.transforms.initramfs_live.output_names,
            "contract.transforms.initramfs_live.output_names",
            distro_id,
        )?,
        initramfs_installed: optional_transform_output_name(
            bundle
                .contract
                .transforms
                .initramfs_installed
                .as_ref()
                .map(|transform| transform.output_names.as_slice()),
            "contract.transforms.initramfs_installed.output_names",
            distro_id,
        )?,
        overlay_image: required_transform_output_name(
            &bundle.contract.transforms.overlay_image.output_names,
            "contract.transforms.overlay_image.output_names",
            distro_id,
        )?,
    })
}

fn required_transform_output_name(
    outputs: &[String],
    field: &str,
    distro_id: &str,
) -> Result<String> {
    outputs.first().cloned().with_context(|| {
        format!(
            "missing canonical Ring 1 output '{}' for '{}'",
            field, distro_id
        )
    })
}

fn optional_transform_output_name(
    outputs: Option<&[String]>,
    field: &str,
    distro_id: &str,
) -> Result<Option<String>> {
    let Some(outputs) = outputs else {
        return Ok(None);
    };
    Ok(Some(required_transform_output_name(
        outputs, field, distro_id,
    )?))
}

fn resolve_required_file(artifact_dir: &Path, canonical: &str) -> Result<PathBuf> {
    let canonical_path = artifact_dir.join(canonical);
    if canonical_path.is_file() {
        return Ok(canonical_path);
    }
    Ok(artifact_dir.join(canonical))
}

fn resolve_optional_file(artifact_dir: &Path, canonical: &str) -> Result<Option<PathBuf>> {
    let canonical_path = artifact_dir.join(canonical);
    if canonical_path.is_file() {
        return Ok(Some(canonical_path));
    }
    Ok(None)
}

fn resolve_required_dir(artifact_dir: &Path, canonical: &str) -> Result<PathBuf> {
    let canonical_path = artifact_dir.join(canonical);
    if canonical_path.is_dir() {
        return Ok(canonical_path);
    }
    Ok(artifact_dir.join(canonical))
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
             Run 'cargo run -p distro-builder --bin distro-builder -- release build iso {} base-rootfs' to rebuild the ISO.\n\
             ALL verification checks must pass before running tests.",
            all_failures.join("\n"),
            distro_id
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "install-tests-preflight-{test_name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, content).expect("write file");
    }

    #[test]
    fn resolve_runtime_artifacts_prefers_product_native_names() {
        let dir = temp_dir("product-native");
        write_file(&dir.join("s00-filesystem.erofs"), "rootfs");
        write_file(&dir.join("s00-initramfs-live.cpio.gz"), "initramfs");
        write_file(
            &dir.join("s00-initramfs-installed.img"),
            "install-initramfs",
        );
        write_file(&dir.join("s00-overlayfs.erofs"), "overlay");
        write_file(&dir.join(".live-rootfs-source.path"), "./rootfs-source\n");
        fs::create_dir_all(dir.join("live-overlay")).expect("create live overlay");

        let resolved =
            resolve_runtime_artifacts(&dir, "levitate").expect("resolve runtime artifacts");
        assert_eq!(resolved.rootfs_image, dir.join("s00-filesystem.erofs"));
        assert_eq!(
            resolved.initramfs_live,
            dir.join("s00-initramfs-live.cpio.gz")
        );
        assert_eq!(
            resolved.initramfs_installed,
            Some(dir.join("s00-initramfs-installed.img"))
        );
        assert_eq!(resolved.overlay_image, dir.join("s00-overlayfs.erofs"));
        assert_eq!(
            resolved.rootfs_source_pointer,
            dir.join(".live-rootfs-source.path")
        );
        assert_eq!(resolved.live_overlay_dir, dir.join("live-overlay"));

        fs::remove_dir_all(dir).expect("cleanup temp dir");
    }

    #[test]
    fn live_boot_runtime_scope_uses_release_product_metadata() {
        let dir = temp_dir("scope");
        let manifest = RunManifest {
            status: "success".to_string(),
            created_at_utc: "20260313T120000Z".to_string(),
            finished_at_utc: Some("20260313T120100Z".to_string()),
            iso_path: Some(dir.join("levitate.iso").display().to_string()),
            target_kind: Some("release-product".to_string()),
            target_name: Some("live-boot".to_string()),
        };
        assert!(should_validate_live_boot_runtime(Some(&manifest)));

        let base_manifest = RunManifest {
            target_name: Some("base-rootfs".to_string()),
            ..manifest
        };
        assert!(!should_validate_live_boot_runtime(Some(&base_manifest)));

        fs::remove_dir_all(dir).expect("cleanup temp dir");
    }
}
