//! Pack explicit existing release artifacts as an independently consumed test ISO.
//! No system image, kernel, launcher, worker or runtime is built by this command.
use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
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
    /// Generate a public-origin descriptor from existing signed producer metadata (no payload copy).
    Public(PublicArgs),
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
        Operation::Public(args) => public_descriptor(args),
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

/// Generate public metadata only from explicit existing producer metadata.
#[derive(Args)]
struct PublicArgs {
    #[arg(long, default_value = ".")]
    repo_root: PathBuf,
    #[arg(long,value_parser=["levitate","acorn"])]
    distro: String,
    #[arg(long)]
    baseline_manifest: PathBuf,
    /// The original installed stable policy from the baseline's prepared root.
    #[arg(long)]
    base_policy: PathBuf,
    /// Existing publication metadata directory; no payloads are copied.
    #[arg(long)]
    publication: PathBuf,
    /// Existing incoming producer inventory, hash-bound to its canonical manifest.
    #[arg(long)]
    inventory: PathBuf,
    #[arg(long)]
    updated_path: String,
    #[arg(long)]
    conflict_path: String,
    #[arg(long)]
    output: PathBuf,
}
fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= limit,
        "metadata file exceeds bound"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "metadata grew beyond bound");
    Ok(bytes)
}
fn public_descriptor(args: PublicArgs) -> Result<()> {
    use format::production::{Audience, AuthorityProof, MAX_AUTHORITY, SigningRole};
    ensure!(!args.output.exists(), "output already exists");
    let base_raw = bounded(&args.baseline_manifest, format::MAX_MANIFEST as u64)?;
    let base = format::ReleaseManifest::parse(&base_raw)?;
    let original: Policy = serde_json::from_slice(&bounded(&args.base_policy, 65536)?)?;
    original.validate()?;
    ensure!(
        original.channel == base.identity.channel && original.sequence == base.identity.sequence,
        "base policy differs from baseline release"
    );
    let production = original
        .production
        .as_ref()
        .context("baseline requires production policy/reader")?;
    let base_dir = args
        .baseline_manifest
        .parent()
        .context("baseline directory missing")?;
    let base_authority = AuthorityProof::parse(&bounded(
        &base_dir.join("release-keys.json"),
        MAX_AUTHORITY as u64,
    )?)?
    .verify_archived(production, &base.identity.distro, &base.identity.channel)?;
    base_authority.verify(
        SigningRole::Release,
        &base_raw,
        &bounded(&base_dir.join("release.sig"), 64)?,
    )?;
    let pub_raw = bounded(
        &args.publication.join("publication.json"),
        format::MAX_MANIFEST as u64,
    )?;
    let publication: distro_builder::release_index::production_publish::Publication =
        serde_json::from_slice(&pub_raw)?;
    let raw = bounded(
        &args.publication.join("release.json"),
        format::MAX_MANIFEST as u64,
    )?;
    let next = format::ReleaseManifest::parse(&raw)?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let authority = AuthorityProof::parse(&bounded(
        &args.publication.join("release-keys.json"),
        MAX_AUTHORITY as u64,
    )?)?
    .verify(
        production,
        &next.identity.distro,
        &next.identity.channel,
        at,
    )?;
    let sig = bounded(&args.publication.join("release.sig"), 64)?;
    authority.verify(SigningRole::Release, &raw, &sig)?;
    publication.entry.for_manifest(&next, production)?;
    ensure!(
        publication.schema_version == 1
            && publication.distro == args.distro
            && publication.id == format::digest(&raw)
            && publication.entry.release.manifest.sha256 == publication.id
            && publication.entry.release.manifest.size == raw.len() as u64
            && publication.entry.release.signature.sha256 == format::digest(&sig)
            && publication.entry.release.signature.size == 64
            && publication.tag
                == format!(
                    "os-{}-x86_64-{}-{}-{}",
                    args.distro,
                    next.identity.flavour,
                    next.identity.sequence,
                    &publication.id[..16]
                ),
        "publication identity/signature binding differs"
    );
    ensure!(
        pub_raw == serde_json::to_vec_pretty(&publication)?,
        "publication must retain the producer's exact canonical bytes"
    );
    let raw = bounded(&args.inventory, format::MAX_INVENTORY)?;
    ensure!(
        format::digest(&raw) == next.artifacts.inventory.sha256
            && raw.len() as u64 == next.artifacts.inventory.size,
        "incoming inventory differs from the signed manifest"
    );
    let inventory: format::SystemInventory = serde_json::from_slice(&raw)?;
    inventory.validate()?;
    let raw = bounded(
        &base_dir.join(&base.artifacts.inventory.path),
        format::MAX_INVENTORY,
    )?;
    ensure!(
        format::digest(&raw) == base.artifacts.inventory.sha256
            && raw.len() as u64 == base.artifacts.inventory.size,
        "baseline inventory differs from signed manifest"
    );
    let old: format::SystemInventory = serde_json::from_slice(&raw)?;
    old.validate()?;
    for path in [&args.updated_path, &args.conflict_path] {
        format::relative(path)?;
        let a = old
            .entries
            .get(path)
            .context("observation path absent from baseline")?;
        let b = inventory
            .entries
            .get(path)
            .context("observation path absent from incoming inventory")?;
        ensure!(
            a.kind == "file"
                && b.kind == "file"
                && a.sha256.is_some()
                && b.sha256.is_some()
                && a.sha256 != b.sha256,
            "observation paths must be ordinary files actually changed by the new release"
        );
    }
    let mut source = original.clone();
    source.production.as_mut().unwrap().audience = Audience::Candidate;
    source
        .source
        .as_mut()
        .context("baseline source missing")?
        .index = format!(
        "{}{}/candidate/x86_64/index.json",
        production.metadata_base, args.distro
    );
    let descriptor = install_tests::update_fixture::PublicUpdate {
        schema_version: 1,
        baseline_release: format::digest(&base_raw),
        baseline: base.identity,
        release: publication.id,
        expected: next.identity,
        source,
        publication_sha256: format::digest(&pub_raw),
        source_commit: publication.source_commit,
        updated_sha256: inventory.entries[&args.updated_path]
            .sha256
            .clone()
            .unwrap(),
        updated_path: args.updated_path,
        conflict_path: args.conflict_path,
    };
    descriptor.for_variant(
        &distro_contract::load(&args.repo_root, &args.distro)?,
        descriptor.expected.flavour,
    )?;
    descriptor.personalize(&original)?;
    let parent = args
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    tmp.write_all(&serde_json::to_vec_pretty(&descriptor)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(&args.output)?;
    println!(
        "{} (public metadata only; no payload copy, signing, build, VM run or production enablement)",
        args.output.display()
    );
    Ok(())
}
