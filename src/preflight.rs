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

use crate::repo_root;
use anyhow::{Context, Result};
use colored::Colorize;
use distro_builder::kernel::{
    check_kernel_installed, KERNEL_IMAGE_PATH, KERNEL_MODULES_DIR, KERNEL_RELEASE_PATH,
};
use distro_builder::run_history::{self, RunMetadata};
use distro_contract::{
    validate_build_runtime, validate_live_boot_runtime, BuildRuntimeArtifacts,
    LiveBootRuntimeArtifacts,
};
use fsdbg::checklist::{ChecklistType, VerificationReport};
use fsdbg::cpio::CpioReader;
use fsdbg::iso::IsoReader;
use std::path::{Path, PathBuf};

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

fn verify_conformance_contract(
    distro_id: &str,
    runtime_artifacts: &ResolvedRuntimeArtifacts,
    validate_live_boot: bool,
) -> Result<PreflightCheck> {
    let name = "Contract conformance";
    print!("  Checking {}... ", name);

    let bundle = match distro_contract::load(&repo_root(), distro_id) {
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
    let kernel_output_dir = run_history::kernel_output_dir(&repo_root(), distro_id);

    let runtime_report = validate_build_runtime(
        &bundle,
        &BuildRuntimeArtifacts {
            rootfs_image: runtime_artifacts.rootfs_image.clone(),
            initramfs_live: runtime_artifacts.initramfs_live.clone(),
            overlay_image: runtime_artifacts.overlay_image.clone(),
            kernel_release: kernel_output_dir.join(KERNEL_RELEASE_PATH),
            kernel_image: kernel_output_dir.join(KERNEL_IMAGE_PATH),
            kernel_modules: kernel_output_dir.join(KERNEL_MODULES_DIR),
        },
    );
    details.extend(runtime_report.into_iter().map(|v| v.to_string()));

    if validate_live_boot {
        let live_boot_report = validate_live_boot_runtime(
            &bundle.contract.scenarios.live_boot,
            &LiveBootRuntimeArtifacts {
                rootfs_image: runtime_artifacts.rootfs_image.clone(),
                initramfs_live: runtime_artifacts.initramfs_live.clone(),
                overlay_image: runtime_artifacts.overlay_image.clone(),
                live_overlay_dir: runtime_artifacts.live_overlay_dir.clone(),
                rootfs_source_pointer: runtime_artifacts.rootfs_source_pointer.clone(),
            },
        );
        details.extend(live_boot_report.into_iter().map(|v| v.to_string()));
    }

    if let Err(err) = check_kernel_installed(&bundle, &kernel_output_dir) {
        details.push(format!("kernel: {err:#}"));
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

fn should_validate_live_boot_runtime(run_manifest: Option<&RunMetadata>) -> bool {
    if let Some(manifest) = run_manifest {
        return manifest.target_kind.as_deref() == Some(run_history::TARGET_KIND_RELEASE_PRODUCT)
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

/// Verify the contract and the ISO artifacts in `iso_dir` (the given ISO
/// file, else the first `.iso` found).
pub fn run_preflight(
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
    let run_manifest = run_history::read_run_manifest(iso_dir)?;
    let runtime_artifacts = resolve_runtime_artifacts(iso_dir, distro_id)?;
    let validate_live_boot = should_validate_live_boot_runtime(run_manifest.as_ref());

    result.conformance = Some(verify_conformance_contract(
        distro_id,
        &runtime_artifacts,
        validate_live_boot,
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
    let transforms = distro_contract::load(&repo_root(), distro_id)?.contract.transforms;
    Ok(CanonicalRuntimeArtifactNames {
        rootfs_image: transforms.rootfs_image.output().to_string(),
        initramfs_live: transforms.initramfs_live.output().to_string(),
        initramfs_installed: transforms
            .initramfs_installed
            .as_ref()
            .map(|transform| transform.output().to_string()),
        overlay_image: transforms.overlay_image.output().to_string(),
    })
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

/// Run preflight and fail if a check failed.
pub fn require_preflight(
    iso_dir: &Path,
    iso_filename: Option<&str>,
    distro_id: &str,
) -> Result<()> {
    let result = run_preflight(iso_dir, iso_filename, distro_id)?;

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

        anyhow::bail!("Preflight verification failed. Cannot run installation tests with broken artifacts.\n\n\
             Failures:\n{}\n\n\
             Rebuild the ISO: cargo xtask build base-rootfs {}\n\
             ALL verification checks must pass before running tests.",
            all_failures.join("\n"),
            distro_id);
    }

    Ok(())
}
