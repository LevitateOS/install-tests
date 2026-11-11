//! Pack explicit existing release artifacts as an independently consumed test ISO.
//! No system image, kernel, launcher, worker or runtime is built by this command.
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use distro_contract::release::{
    self as format, Artifact, IndexEntry, Policy, ReleaseIndex, Trust, UpdateSource,
};
use install_tests::update_fixture::{Fixture, check_origin, write_recipes};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Operation,
}
#[derive(Subcommand)]
enum Operation {
    /// Write the actual X/Y/reproduction/failure Recipe probes for focused native checks.
    Recipes {
        #[arg(long)]
        output: PathBuf,
        #[arg(long,value_parser=str::parse::<distro_contract::profile::InitSystem>)]
        init: distro_contract::profile::InitSystem,
    },
    /// Verify and package an existing signed release, native origin helper and public policy.
    Pack {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        base_policy: PathBuf,
        /// Ed25519 public key hex; the private key stays off the fixture.
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        index_signing_key: PathBuf,
        /// Already built ab-update-origin executable for the guest architecture/libc.
        #[arg(long)]
        origin: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        updated_path: String,
        #[arg(long)]
        conflict_path: String,
        #[arg(long, default_value_t = 40213)]
        port: u16,
    },
}
fn main() -> Result<()> {
    match Cli::parse().command {
        Operation::Recipes { output, init } => write_recipes(&output, init),
        Operation::Pack {
            manifest,
            base_policy,
            public_key,
            index_signing_key,
            origin,
            output,
            updated_path,
            conflict_path,
            port,
        } => {
            ensure!(!output.exists(), "output already exists");
            let trust = Trust {
                ed25519: public_key,
            };
            let mut policy: Policy = serde_json::from_slice(&fs::read(base_policy)?)?;
            let verified = recab::release::verify_bundle(&manifest, Some(&trust), false, None)?;
            let release = verified.manifest();
            ensure!(
                release.identity.sequence > policy.sequence
                    && release.identity.channel == policy.channel,
                "release must be newer than the matching installed base policy"
            );
            policy.trust = Some(trust.clone());
            policy.source = Some(UpdateSource {
                index: format!("http://127.0.0.1:{port}/index.json"),
                recipe: format::ACQUISITION_RECIPE.into(),
            });
            policy.validate()?;
            let inventory: format::SystemInventory =
                serde_json::from_slice(&fs::read(verified.inventory_path())?)?;
            let owned = inventory
                .entries
                .get(&updated_path)
                .context("updated_path is absent from the incoming release inventory")?;
            ensure!(
                owned.kind == "file",
                "updated_path must be an ordinary release-owned file"
            );
            let fixture = Fixture {
                schema_version: 1,
                release: verified.id(),
                architecture: release.identity.architecture.clone(),
                interrupt_path: format!("{}/{}", verified.id(), release.artifacts.rootfs.path),
                updated_path,
                updated_sha256: owned.sha256.clone().context("updated file has no digest")?,
                conflict_path,
            };
            fixture.validate()?;
            ensure!(
                inventory
                    .entries
                    .get(&fixture.conflict_path)
                    .is_some_and(|e| e.kind == "file"),
                "conflict_path must be a release-owned ordinary file"
            );
            let origin_bytes = fs::read(&origin)?;
            check_origin(&origin_bytes, &release.identity.architecture)?;
            let parent = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let dir = tempfile::Builder::new()
                .prefix("recab-p13-fixture-")
                .tempdir_in(parent)?;
            let files = dir.path().join("files");
            fs::create_dir(&files)?;
            let root = files.as_path();
            let image = dir.path().join("fixture.iso");
            let feed = root.join("feed");
            let bundle = feed.join(verified.id());
            fs::create_dir_all(&bundle)?;
            for (artifact, input) in [
                (&release.artifacts.rootfs, verified.rootfs()),
                (&release.artifacts.kernel, verified.kernel()),
                (&release.artifacts.initramfs, verified.initramfs()),
                (&release.artifacts.inventory, verified.inventory_path()),
            ] {
                let target = bundle.join(&artifact.path);
                fs::create_dir_all(target.parent().context("artifact parent missing")?)?;
                fs::copy(input, target)?;
            }
            let bytes = fs::read(verified.manifest_path())?;
            let key = fs::read(index_signing_key)?;
            let signature = format::sign(&bytes, &key, &trust)?;
            fs::write(bundle.join(format::MANIFEST), &bytes)?;
            fs::write(bundle.join(format::SIGNATURE), &signature)?;
            let index = ReleaseIndex {
                schema_version: 1,
                sequence: release.identity.sequence,
                channel: release.identity.channel.clone(),
                releases: vec![IndexEntry {
                    identity: release.identity.clone(),
                    manifest: Artifact {
                        path: format!("{}/release.json", verified.id()),
                        sha256: verified.id(),
                        size: bytes.len() as u64,
                    },
                    signature: Artifact {
                        path: format!("{}/release.sig", verified.id()),
                        sha256: format!("{:x}", Sha256::digest(&signature)),
                        size: signature.len() as u64,
                    },
                }],
            };
            index.validate()?;
            let bytes = serde_json::to_vec(&index)?;
            let signature = format::sign(&bytes, &key, &trust)?;
            fs::create_dir(feed.join("index-signatures"))?;
            fs::write(
                feed.join("index-signatures")
                    .join(format!("{:x}.sig", Sha256::digest(&bytes))),
                &signature,
            )?;
            fs::write(feed.join("index.sig"), signature)?;
            fs::write(feed.join("index.json"), bytes)?;
            fs::write(root.join("fixture.json"), serde_json::to_vec(&fixture)?)?;
            fs::write(root.join("source.json"), serde_json::to_vec(&policy)?)?;
            fs::copy(origin, root.join("origin"))?;
            let recipe = include_str!("../../../../distro/builder/os-recipes/update-acquire.rhai");
            let marker = "network: [] /* recab configured endpoint */";
            ensure!(
                recipe.matches(marker).count() == 1,
                "acquisition producer marker changed"
            );
            fs::write(
                root.join("acquire.rhai"),
                recipe.replace(marker, &format!("network: [\"127.0.0.1:{port}\"]")),
            )?;
            let status = Command::new("xorriso")
                .args([
                    "-as",
                    "mkisofs",
                    "-quiet",
                    "-R",
                    "-J",
                    "-V",
                    "RECAB_P13",
                    "-o",
                ])
                .arg(&image)
                .arg(root)
                .status()?;
            ensure!(status.success(), "packing fixture ISO failed");
            fs::hard_link(&image, &output)
                .context("publishing fixture without overwriting existing output")?;
            println!(
                "RECAB_P13 {} (signed release {}, architecture {:?}); private signing key excluded",
                output.display(),
                verified.id(),
                release.identity.architecture
            );
            Ok(())
        }
    }
}
