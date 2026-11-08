//! Preflight: before a VM boots a release ISO, check the release against the
//! variant's contract, the same way for every variant. Every artifact the
//! contract declares for the product must be in the release's run directory
//! (the ISO the run manifest names, the rootfs and overlay images, the live
//! initramfs, the installed system's initramfs when the variant declares
//! one, and for a live-session product its overlay and rootfs trees), and
//! the contract's runtime checks must pass
//! (`distro_contract::validate_build_runtime`, `validate_live_boot_runtime`).
//!
//! It reads files only: the builder verifies the kernel recipe's output and
//! each producer its own artifacts when it builds them.

use anyhow::{bail, Context, Result};
use distro_builder::kernel::{KERNEL_IMAGE_PATH, KERNEL_MODULES_DIR, KERNEL_RELEASE_PATH};
use distro_contract::runs;
use distro_contract::{
    validate_build_runtime, validate_live_boot_runtime, BuildRuntimeArtifacts,
    LiveBootRuntimeArtifacts, ProductId, Variant, Violation,
};
use std::path::Path;

/// Check `product`'s release ISO at `iso` and the artifacts beside it; fails
/// listing every violation. Returns what was checked.
pub fn require_preflight(iso: &Path, product: ProductId, variant: &Variant) -> Result<String> {
    let run_dir = iso
        .parent()
        .with_context(|| format!("'{}' has no parent directory", iso.display()))?;
    let manifest = runs::read(run_dir).map_err(anyhow::Error::msg)?;
    let mut violations = Vec::new();
    if manifest.product() != Some(product) {
        violations.push(Violation::new(
            run_dir.display().to_string(),
            format!("the run built '{}', not {product}", manifest.target_name),
        ));
    }
    if manifest.iso_path.as_deref() != Some(iso) || !iso.is_file() {
        violations.push(Violation::new(iso.display().to_string(), "the run manifest's ISO is missing"));
    }

    let artifacts = &variant.contract.artifacts;
    let build = distro_builder::product::build(product);
    let kernel = runs::kernel_dir(&variant.repo_root, &variant.id);
    let mut checked = vec!["run manifest", "ISO", "build runtime"];
    violations.extend(validate_build_runtime(
        variant,
        &BuildRuntimeArtifacts {
            rootfs_image: run_dir.join(&artifacts.rootfs),
            initramfs_live: run_dir.join(&artifacts.initramfs_live),
            overlay_image: run_dir.join(&artifacts.overlay),
            kernel_release: kernel.join(KERNEL_RELEASE_PATH),
            kernel_image: kernel.join(KERNEL_IMAGE_PATH),
            kernel_modules: kernel.join(KERNEL_MODULES_DIR),
        },
    ));
    if let Some(initramfs) = &artifacts.initramfs_installed {
        checked.push("installed initramfs");
        if !run_dir.join(initramfs).is_file() {
            violations.push(Violation::new(
                run_dir.join(initramfs).display().to_string(),
                "the installed system's initramfs, which the variant declares, is missing",
            ));
        }
    }
    if build.live_session {
        checked.push("live-boot runtime");
        violations.extend(validate_live_boot_runtime(
            &variant.contract,
            &LiveBootRuntimeArtifacts {
                rootfs_image: run_dir.join(&artifacts.rootfs),
                initramfs_live: run_dir.join(&artifacts.initramfs_live),
                overlay_image: run_dir.join(&artifacts.overlay),
                live_overlay_dir: run_dir.join(build.overlay_dir),
                rootfs_source_pointer: run_dir.join(build.rootfs_source_pointer),
            },
        ));
    }

    if !violations.is_empty() {
        let list: Vec<String> = violations.iter().map(|v| format!("  - {v}")).collect();
        bail!(
            "preflight: the {product} release of {} in '{}' breaks its contract ({} violations); rebuild the {product} release:\n{}",
            variant.id,
            run_dir.display(),
            violations.len(),
            list.join("\n")
        );
    }
    eprintln!("  preflight: {product} release {} matches the contract", manifest.run_id);
    Ok(format!(
        "the {product} release {} passes preflight ({})",
        manifest.run_id,
        checked.join(", ")
    ))
}
