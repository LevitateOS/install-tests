//! Preflight: before a VM boots a release ISO, check the release against the
//! variant's contract, the same way for every variant. Every artifact the
//! contract declares for the product must be in the release's run directory
//! (the ISO the run manifest names, the rootfs and overlay images, the live
//! initramfs, the installed system's initramfs, and for a live-session product its overlay and rootfs trees), and
//! the contract's runtime checks must pass
//! (`distro_contract::validate_build_runtime`, `validate_live_boot_runtime`).
//!
//! It reads files only: the builder verifies the kernel recipe's output and
//! each producer its own artifacts when it builds them.

use anyhow::{Context, Result, bail};
use distro_builder::kernel::{KERNEL_IMAGE_PATH, KERNEL_MODULES_DIR, KERNEL_RELEASE_PATH};
use distro_contract::runs;
use distro_contract::{
    BuildRuntimeArtifacts, LiveBootRuntimeArtifacts, ProductId, Variant, Violation,
    validate_build_runtime, validate_live_boot_runtime,
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
        violations.push(Violation::new(
            iso.display().to_string(),
            "the run manifest's ISO is missing",
        ));
    }

    let artifacts = &variant.contract.artifacts;
    let build = distro_builder::product::build(product);
    let kernel = runs::kernel_dir(&variant.repo_root, &variant.id);
    let mut checked = vec!["run manifest", "ISO", "build runtime"];
    if run_dir.join(distro_contract::release::MANIFEST).is_file() {
        // A selected historical release owns its kernel and module tree. Do not
        // substitute today's kernel build directory for those exact artifacts.
        let raw = std::fs::read(run_dir.join(distro_contract::release::MANIFEST))?;
        let release = distro_contract::release::ReleaseManifest::parse(&raw)?;
        let prepared = distro_builder::PreparedProduct::read(run_dir)?.rootfs_source(run_dir);
        if release.identity.distro != variant.contract.identity.os_id
            || release.identity.flavour != product.flavour()
            || release.identity.init != variant.contract.profile.init
            || release.identity.libc != variant.contract.profile.libc
        {
            violations.push(Violation::new(
                "release.json",
                "selected release does not match distro/product/profile",
            ));
        }
        for artifact in release.artifacts.all() {
            use sha2::{Digest, Sha256};
            let path = run_dir.join(&artifact.path);
            let valid = (|| -> Result<bool> {
                let mut file = std::fs::File::open(&path)?;
                let mut hash = Sha256::new();
                let n = std::io::copy(&mut file, &mut hash)?;
                Ok(n == artifact.size && format!("{:x}", hash.finalize()) == artifact.sha256)
            })();
            if !matches!(valid, Ok(true)) {
                violations.push(Violation::new(
                    path.display().to_string(),
                    "artifact differs from selected canonical release",
                ));
            }
        }
        let modules = prepared
            .join("usr/lib/modules")
            .join(&release.kernel.release);
        if recinit::installed::modules_digest(&modules).ok().as_deref()
            != Some(release.kernel.modules_sha256.as_str())
        {
            violations.push(Violation::new(
                modules.display().to_string(),
                "selected release module tree does not match its bound kernel",
            ));
        }
        for path in [
            run_dir.join(&artifacts.initramfs_live),
            run_dir.join(&artifacts.overlay),
        ] {
            if !path.is_file() {
                violations.push(Violation::new(
                    path.display().to_string(),
                    "selected live boot artifact is missing",
                ));
            }
        }
        let os = std::fs::read_to_string(prepared.join("etc/os-release"))?;
        if distro_contract::profile::Flavour::from_os_release(&os)? != product.flavour() {
            violations.push(Violation::new(
                prepared.display().to_string(),
                "prepared root differs from selected flavour",
            ));
        }
        checked.push("canonical release payload hashes and bound kernel/modules");
    } else {
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
    }
    checked.push("installed initramfs");
    if !run_dir.join(&artifacts.initramfs_installed).is_file() {
        violations.push(Violation::new(
            run_dir
                .join(&artifacts.initramfs_installed)
                .display()
                .to_string(),
            "the installed system's initramfs is missing",
        ));
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
                rootfs_source_dir: distro_builder::PreparedProduct::read(run_dir)?
                    .rootfs_source(run_dir),
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
    eprintln!(
        "  preflight: {product} release {} matches the contract",
        manifest.run_id
    );
    Ok(format!(
        "the {product} release {} passes preflight ({})",
        manifest.run_id,
        checked.join(", ")
    ))
}
