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
use distro_builder::kernel::{
    check_kernel_installed, KERNEL_IMAGE_PATH, KERNEL_MODULES_DIR, KERNEL_RELEASE_PATH,
};
use distro_contract::runs::{self, RunManifest};
use distro_contract::{
    validate_build_runtime, validate_live_boot_runtime, BuildRuntimeArtifacts,
    LiveBootRuntimeArtifacts, ProductId, Variant,
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

/// The built artifacts against the (already loaded and validated) variant.
fn verify_conformance_contract(
    bundle: &Variant,
    runtime_artifacts: &ResolvedRuntimeArtifacts,
    validate_live_boot: bool,
) -> Result<PreflightCheck> {
    let name = "Contract conformance";
    eprint!("  Checking {}... ", name);

    let mut details = Vec::new();
    let kernel_output_dir = runs::kernel_dir(&bundle.repo_root, &bundle.id);

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
            &bundle.contract,
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

    if let Err(err) = check_kernel_installed(bundle, &kernel_output_dir) {
        details.push(format!("kernel: {err:#}"));
    }

    if details.is_empty() {
        eprintln!("{} (declaration + runtime)", "PASS".green());
        Ok(PreflightCheck {
            name: name.to_string(),
            passed: true,
            total_checks: 2,
            passed_checks: 2,
            failures: 0,
            details: Vec::new(),
        })
    } else {
        eprintln!("{} ({} violations)", "FAIL".red().bold(), details.len());
        for detail in &details {
            eprintln!("    {}", detail.red());
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

/// The release product the run in `iso_dir` built, by its run manifest.
fn built_product(run_manifest: Option<&RunManifest>) -> Option<ProductId> {
    run_manifest?.product()
}

fn resolve_runtime_artifacts(
    artifact_dir: &Path,
    variant: &Variant,
    product: ProductId,
) -> Result<ResolvedRuntimeArtifacts> {
    let artifacts = &variant.contract.artifacts;
    let build = distro_builder::product::build(product);
    Ok(ResolvedRuntimeArtifacts {
        rootfs_image: resolve_required_file(artifact_dir, &artifacts.rootfs)?,
        initramfs_live: resolve_required_file(artifact_dir, &artifacts.initramfs_live)?,
        initramfs_installed: match &artifacts.initramfs_installed {
            Some(name) => resolve_optional_file(artifact_dir, name)?,
            None => None,
        },
        overlay_image: resolve_required_file(artifact_dir, &artifacts.overlay)?,
        live_overlay_dir: resolve_required_dir(artifact_dir, build.overlay_dir)?,
        rootfs_source_pointer: resolve_required_file(artifact_dir, build.rootfs_source_pointer)?,
    })
}

/// Verify the contract and the ISO artifacts in `iso_dir` (the given ISO
/// file, else the first `.iso` found), built as `product`.
pub fn run_preflight(
    iso_dir: &Path,
    iso_filename: Option<&str>,
    product: ProductId,
    variant: &Variant,
) -> Result<PreflightResult> {
    eprintln!();
    eprintln!("{}", "=== PREFLIGHT VERIFICATION ===".cyan().bold());
    eprintln!(
        "Verifying contract + ISO artifacts for {} before starting QEMU...",
        variant.id
    );
    eprintln!();

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
    let run_manifest = runs::read(iso_dir).ok();
    let runtime_artifacts = resolve_runtime_artifacts(iso_dir, variant, product)?;
    // A product whose overlay sets up the live session is checked for it.
    let validate_live_boot = built_product(run_manifest.as_ref())
        .is_some_and(|built| distro_builder::product::build(built).live_session);

    result.conformance = Some(verify_conformance_contract(
        variant,
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
        eprintln!(
            "  {} Live initramfs not found at {}",
            "SKIP".yellow(),
            live_path.display()
        );
    }

    // The installed system's initramfs, when the variant declares one.
    if let Some(declared) = &variant.contract.artifacts.initramfs_installed {
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
                eprintln!(
                    "  {} Install initramfs not found at {}",
                    "SKIP".yellow(),
                    install_path.display()
                );
            }
        } else {
            eprintln!(
                "  {} Install initramfs not found at {}",
                "SKIP".yellow(),
                iso_dir.join(declared).display()
            );
        }
    }

    let iso_path = if let Some(path) = resolved_iso_path {
        path
    } else {
        eprintln!(
            "  {} No .iso file found in {}",
            "✗".red(),
            iso_dir.display()
        );
        result.overall_pass = false;
        eprintln!();
        print_summary(&result);
        return Ok(result);
    };

    if iso_path.exists() {
        result.iso = Some(verify_iso_distro(&iso_path)?);
        if !result.iso.as_ref().unwrap().passed {
            result.overall_pass = false;
        }
    } else {
        eprintln!("  {} ISO not found at {}", "✗".red(), iso_path.display());
        result.overall_pass = false;
    }

    eprintln!();
    print_summary(&result);
    Ok(result)
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

/// Verify an ISO against the ISO checklist.
fn verify_iso_distro(path: &Path) -> Result<PreflightCheck> {
    let name = "Live ISO";
    eprint!("  Checking {}... ", name);

    let reader = match IsoReader::open(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", "FAIL".red().bold());
            eprintln!("    Failed to read ISO: {}", e);
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
    let report = fsdbg::checklist::iso::verify(&reader);

    let check = PreflightCheck::from_report(name, &report);

    // Print inline result
    if check.passed {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let size_mb = size / (1024 * 1024);
        eprintln!(
            "{} ({}/{} checks, {} MB)",
            "PASS".green(),
            check.passed_checks,
            check.total_checks,
            size_mb
        );
    } else {
        eprintln!(
            "{} ({}/{} checks, {} failed)",
            "FAIL".red().bold(),
            check.passed_checks,
            check.total_checks,
            check.failures
        );
        for detail in &check.details {
            eprintln!("    {}", detail.red());
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
    eprint!("  Checking {}... ", name);

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
                    eprintln!("{}", "FAIL".red().bold());
                    eprintln!("    Failed to read ISO: {}", e);
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
            eprintln!(
                "{} ({}/{} checks, {} MB)",
                "PASS".green(),
                check.passed_checks,
                check.total_checks,
                size_mb
            );
        } else {
            eprintln!(
                "{} ({}/{} checks)",
                "PASS".green(),
                check.passed_checks,
                check.total_checks
            );
        }
    } else {
        eprintln!(
            "{} ({}/{} checks, {} failed)",
            "FAIL".red().bold(),
            check.passed_checks,
            check.total_checks,
            check.failures
        );
        // Show failures
        for detail in &check.details {
            eprintln!("    {}", detail.red());
        }
    }

    Ok(check)
}

/// Print the overall summary.
fn print_summary(result: &PreflightResult) {
    eprintln!("{}", "--- Preflight Summary ---".bold());

    let status = if result.overall_pass {
        "PASS".green().bold()
    } else {
        "FAIL".red().bold()
    };

    eprintln!("Overall: {}", status);

    if !result.overall_pass {
        eprintln!();
        eprintln!(
            "{}",
            "Preflight verification failed. Fix the issues above before running tests.".red()
        );
        eprintln!(
            "{}",
            "The ISO artifacts are broken and will not work correctly.".red()
        );
    }
}

/// Run preflight on `product`'s ISO at `iso` and fail if a check failed.
pub fn require_preflight(iso: &Path, product: ProductId, variant: &Variant) -> Result<()> {
    let iso_dir = iso
        .parent()
        .with_context(|| format!("'{}' has no parent directory", iso.display()))?;
    let iso_filename = iso.file_name().and_then(|name| name.to_str());
    let result = run_preflight(iso_dir, iso_filename, product, variant)?;

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
             Rebuild the ISO: cargo xtask build {} {}\n\
             ALL verification checks must pass before running tests.",
            all_failures.join("\n"),
            product,
            variant.id);
    }

    Ok(())
}
