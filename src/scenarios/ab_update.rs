//! Real A/B continuation across boots, against an explicit signed update fixture.
//! No scenario builds a release, edits the running root, or mounts a candidate writable.
use super::{Harness, InstallRun, ScenarioId};
use crate::update_fixture::{
    Fixture, PublicUpdate, failure_recipe, metadata_recipe, probe_recipe, profile_recipe,
};
use crate::{
    budget::Budget,
    qemu::{
        Console,
        session::{self, Vars, VmSpec},
        wait_installed_boot,
    },
};
use anyhow::{Context, Result, bail, ensure};
use distro_contract::{Variant, release::Policy};
use recab::{ChangeProgress, Finalization, Preparation, Readiness, Slot, Status};
use recqemu::{Guest, Outcome, shell_quote};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{fs, path::Path, thread, time::Instant};

pub(super) const MEDIA: &str = "/run/recab-p13-media";
const WORK: &str = "/var/tmp/recab-p13";
const MOUNT: &str = "/run/recab-p13-readonly";
const PROBE: &str = "usr/share/recab-p13";
const LARGE: u64 = 32 * 1024 * 1024;
const RANGE: u64 = 1024 * 1024;

pub(super) fn run(
    h: &Harness,
    install: &InstallRun,
    fixture: Option<&Path>,
    public_path: Option<&Path>,
    run_dir: &Path,
) -> Result<(String, Option<distro_contract::runs::PublicUpdateEvidence>)> {
    let installed = distro_contract::runs::read(
        install
            .disk_path
            .parent()
            .context("install directory missing")?,
    )
    .map_err(anyhow::Error::msg)?;
    let public = public_path.map(PublicUpdate::read).transpose()?;
    let descriptor_sha256 = public_path
        .map(|p| fs::read(p).map(|b| distro_contract::release::digest(&b)))
        .transpose()?;
    if let Some(public) = &public {
        public.for_variant(&h.variant, h.selection.flavour)?;
        ensure!(
            fixture.is_none()
                && installed.public_update_binding.as_deref()
                    == Some(public.installation_binding()?.as_str()),
            "public mode requires the matching existing candidate-personalized paired install; local fixture media is prohibited"
        );
    } else {
        ensure!(
            fixture.is_some()
                && installed.update_fixture.is_some()
                && installed.public_update.is_none(),
            "local ab-update needs its existing fixture-configured install"
        );
    }
    let (disk, vars) = super::overlay_install(install, run_dir)?;
    let result = Machine {
        variant: &h.variant,
        disk: &disk,
        vars: &vars,
        fixture,
        public: public.clone(),
        descriptor_sha256: descriptor_sha256.clone(),
        flavour: install.flavour,
        run_dir,
        boots: 0,
        records: vec![],
        completed: false,
        resolved_policy: None,
    }
    .exercise()?;
    let evidence = public.map(|p| public_evidence(&p, descriptor_sha256.unwrap(), true));
    Ok((result, evidence))
}
fn public_evidence(
    public: &PublicUpdate,
    descriptor_sha256: String,
    passed: bool,
) -> distro_contract::runs::PublicUpdateEvidence {
    distro_contract::runs::PublicUpdateEvidence {
        baseline_release: public.baseline_release.clone(),
        release_id: public.release.clone(),
        public_index: public.source.source.as_ref().unwrap().index.clone(),
        descriptor_sha256,
        publication_sha256: public.publication_sha256.clone(),
        source_commit: public.source_commit.clone(),
        flavour: public.expected.flavour,
        x86_64_lifecycle_passed: passed,
        directions: if passed {
            vec!["A->B".into(), "B->A".into()]
        } else {
            vec![]
        },
    }
}

struct Machine<'a> {
    variant: &'a Variant,
    disk: &'a Path,
    vars: &'a Path,
    fixture: Option<&'a Path>,
    public: Option<PublicUpdate>,
    descriptor_sha256: Option<String>,
    flavour: distro_contract::profile::Flavour,
    completed: bool,
    resolved_policy: Option<Policy>,
    run_dir: &'a Path,
    boots: u32,
    records: Vec<Value>,
}
struct Booted {
    vm: session::Vm,
}

impl Machine<'_> {
    fn save(&mut self, phase: &str, value: Value) -> Result<()> {
        self.records
            .push(json!({"phase":phase,"observation":value}));
        fs::write(
            self.run_dir.join("ab-update.json"),
            serde_json::to_vec_pretty(&json!({
                "schema_version":2,"boots":self.boots,"observations":self.records,"flavour":self.flavour,
                "public_update":self.public.as_ref().map(|p|public_evidence(p,self.descriptor_sha256.clone().unwrap(),self.completed)),
                "scope":"guest application counters and virtual block-device sectors; not SSD SMART or physical wear"
            }))?,
        )?;
        Ok(())
    }
    fn observe(&mut self, sys: &mut Booted, phase: &str) -> Result<Status> {
        let s = sys.status()?;
        self.save(phase, serde_json::to_value(&s)?)?;
        Ok(s)
    }
    fn exercise(mut self) -> Result<String> {
        let mut sys = self.boot()?;
        let initial = self.observe(&mut sys, "initial-A")?;
        expect(&initial, Slot::A, true)?;
        let first = generation(&initial, Slot::A)?.to_owned();
        ensure!(
            initial.schema_version >= 10
                && initial.preparation.is_none()
                && initial.last_trial.is_none(),
            "install is not a fresh lifecycle installation"
        );
        ensure!(
            initial.slots.iter().all(|s| s.generation_ready
                && s.bootable
                && s.seal.is_some()
                && s.generation.as_ref().is_some_and(|g| g.id == first)),
            "initial A/B systems are not equivalent and ready"
        );
        ensure!(
            initial
                .slots
                .iter()
                .find(|s| s.slot == Slot::B)
                .is_some_and(|s| s.readiness == Some(Readiness::Ready)),
            "initial B was falsely committed as a trial"
        );
        sys.immutable()?;
        sys.fidelity()?;
        self.save(
            "execution-architecture",
            json!({"guest":sys.sh("uname -m")?,"required_architectures":["x86_64"]}),
        )?;
        let (fixture, source) = if let Some(public) = &self.public {
            let baseline: Value =
                sys.json("cat /usr/lib/recab/release-baseline.json", Budget::Command)?;
            ensure!(
                baseline["identity"] == serde_json::to_value(&public.baseline)?,
                "installed release baseline differs from public descriptor"
            );
            sys.sh(&format!(
                "test ! -e /dev/disk/by-label/RECAB_P13 && test ! -e {WORK}/origin"
            ))?;
            (public.fixture(), public.source.clone())
        } else {
            sys.mount_media()?;
            (
                sys.json::<Fixture>(&format!("cat {MEDIA}/fixture.json"), Budget::Command)?,
                sys.json::<Policy>(&format!("cat {MEDIA}/source.json"), Budget::Command)?,
            )
        };
        fixture.validate()?;
        source.validate()?;
        ensure!(
            sys.sh("uname -m")? == "x86_64",
            "ab-update supports x86_64 only"
        );
        let installed: Policy =
            sys.json("cat /usr/lib/recab/update-source.json", Budget::Command)?;
        ensure!(
            source == installed
                && source.source.is_some()
                && (source.trust.is_some() || source.production.is_some()),
            "install lacks the exact source policy configured before sealing"
        );
        self.save("acquisition-source",json!({"public_origin":self.public.is_some(),"policy":source,"local_origin_started":false}))?;
        let identity = sys.identity()?;
        ensure!(!identity.is_empty(), "initial machine identity is missing");
        let fs_ids = sys.filesystem_ids(&initial)?;
        let original = sys.inventory()?;
        let shared = sys.sh(&format!(
            "mkdir -p {WORK}; printf 'shared user data\n' > {WORK}/shared; sha256sum {WORK}/shared"
        ))?;
        sys.write(&format!("{WORK}/user-profile.rhai"), profile_recipe())?;
        let ledger = sys.sh("sha256sum /var/lib/recab/state.json")?;
        let profile:Value=sys.json(&format!("cd {WORK} && recipe --json --package-root {WORK}/user-store --build-dir {WORK}/user-build --state-dir {WORK}/user-state install {WORK}/user-profile.rhai"),Budget::Stage)?;
        ensure!(
            sys.sh("sha256sum /var/lib/recab/state.json")? == ledger,
            "independent Recipe profile changed recab state"
        );
        self.save("independent-Recipe-profile", profile)?;
        sys.shared(&shared)?;
        // B is exercised at first use, before any continuation writes it.
        sys.sh("recab set-next B")?;
        sys.reboot()?;
        let mut sys = self.boot()?;
        sys.wait_committed(Slot::B)?;
        sys.shared(&shared)?;
        sys.immutable()?;
        ensure!(
            sys.identity()? == identity && generation(&sys.status()?, Slot::B)? == first,
            "first-use B differs from personalized A"
        );
        self.observe(&mut sys, "initial-B-bootable")?;
        sys.sh("recab rollback")?;
        sys.reboot()?;
        let mut sys = self.boot()?;
        expect(&sys.status()?, Slot::A, true)?;
        let before_release = sys.sh(&format!(
            "sha256sum -- {}",
            shell_quote(&format!("/{}", fixture.updated_path))
        ))?;
        ensure!(
            !before_release.starts_with(&fixture.updated_sha256),
            "fixture's newer release must actually change updated_path"
        );
        if self.public.is_none() {
            sys.origin(&source, &fixture)?;
        }
        let before = sys.sectors(&initial)?;
        let prepared: Preparation = sys.json("recab --json prepare", Budget::Stage)?;
        ensure!(
            prepared.slot == Slot::B
                && prepared
                    .synchronization
                    .as_ref()
                    .is_some_and(|r| r.payload_bytes_written == 0 && r.accounting_complete),
            "identical first continuation rewrote system payloads"
        );
        self.save("identical-continuation", serde_json::to_value(&prepared)?)?;
        sys.refuse("recab set-next B")?;
        sys.write(&format!("{WORK}/probe.rhai"), &probe_recipe(false, false))?;
        let x: ChangeProgress = sys.json(
            &format!("recab --json change install {WORK}/probe.rhai"),
            Budget::Stage,
        )?;
        ensure!(
            x.producer_io.is_some() && x.invocation_io.is_some(),
            "Recipe staging/export IO was not reported"
        );
        self.save("Recipe-X", serde_json::to_value(&x)?)?;
        let selections: Value = sys.json("recipe --system --json installed", Budget::Command)?;
        ensure!(
            selections["active"]["packages"]
                .get("recab-p13-probe")
                .is_none()
                && selections["prepared"]["packages"]
                    .get("recab-p13-probe")
                    .is_some(),
            "Recipe did not separate active and prepared package selections"
        );
        self.save("active-and-prepared-Recipe-selections", selections)?;
        sys.sh("recipe --system isinstalled recab-p13-probe --prepared")?;
        let initial_metadata = self.metadata_change(&mut sys, 1)?;
        let metadata_x = self.metadata_change(&mut sys, 2)?;
        ensure!(
            initial_metadata["content"] == metadata_x["content"],
            "metadata-only update changed its data"
        );
        sys.write(&format!("{WORK}/X"), "local X survives the release\n")?;
        let edit: Value = sys.json(
            &format!(
                "recab --json edit {} --file {WORK}/X",
                shell_quote(&fixture.conflict_path)
            ),
            Budget::Stage,
        )?;
        self.save("configuration-X", edit)?;
        let before_download = sys.status()?;
        let selection = boot_selection(&before_download);
        let p = before_download
            .preparation
            .context("X has no preparation")?;
        let discovery: Value = sys.json("recab --json update check", Budget::Stage)?;
        ensure!(
            discovery["available"]["id"] == fixture.release,
            "check selected another release: {discovery}"
        );
        ensure!(
            discovery["available"]["sequence"]
                .as_u64()
                .is_some_and(|n| n > source.sequence),
            "fixture must be newer than the installed release"
        );
        let incoming_sequence = discovery["available"]["sequence"]
            .as_u64()
            .context("release sequence missing")?;
        self.save("signed-discovery", discovery)?;
        if let Some(public) = &self.public {
            let pin: Value = sys.json(
                &format!("cat /var/lib/recab/updates/pins/{}.json", fixture.release),
                Budget::Command,
            )?;
            let parts = public_pin(public, &pin, &self.variant.id)?;
            self.save("public-authenticated-transport",json!({"release":fixture.release,"parts":parts,"publication_sha256":public.publication_sha256}))?;
            sys.cancel_public_download(&fixture.release, parts)?;
        } else {
            sys.refuse("recab --json update download")?;
        } // Local origin deliberately truncates one transfer.
        let interrupted = self.observe(&mut sys, "interrupted-download")?;
        same_preparation(&interrupted, &p)?;
        ensure!(
            boot_selection(&interrupted) == selection,
            "interrupted acquisition changed boot selection"
        );
        let download: Value = sys.json(
            &format!("recab --json update download {}", fixture.release),
            if self.public.is_some() {
                Budget::PublicDownload
            } else {
                Budget::Stage
            },
        )?;
        if self.public.is_some() {
            ensure!(
                download["download"]["reused_parts"].as_u64().unwrap_or(0) > 0
                    || download["download"]["reused_files"].as_u64().unwrap_or(0) > 0,
                "resumed public transfer did not reuse authenticated saved parts"
            );
        }
        ensure!(
            download["download"]["downloaded"] == true
                && download["download"]["release"] == fixture.release,
            "download did not verify the selected complete release"
        );
        self.save("verified-download", download)?;
        let downloaded = self.observe(&mut sys, "X-after-download")?;
        same_preparation(&downloaded, &p)?;
        ensure!(
            boot_selection(&downloaded) == selection,
            "check/download changed boot selection"
        );
        ensure!(
            sys.inventory()? == original,
            "check/download changed the running root"
        );
        sys.inactive(&downloaded,&format!("test \"$(cat -- {})\" = 'local X survives the release' && test -e {MOUNT}/{PROBE}/X",shell_quote(&format!("{MOUNT}/{}",fixture.conflict_path))))?;
        let mut resolved = sys.release_policy(&fixture.release)?;
        ensure!(
            resolved.sequence == incoming_sequence && resolved.channel == source.channel,
            "incoming policy lost release identity"
        );
        if let Some(public) = &self.public {
            let production = resolved
                .production
                .as_mut()
                .context("incoming public release lost delegated authority")?;
            ensure!(
                production.root.lineage == public.source.production.as_ref().unwrap().root.lineage,
                "incoming public release changed trust lineage"
            );
            production.audience = distro_contract::release::production::Audience::Candidate;
            resolved
                .source
                .as_mut()
                .context("incoming release source missing")?
                .index = format!(
                "{}{}/candidate/x86_64/index.json",
                production.metadata_base, self.variant.id
            );
        } else {
            resolved.source = source.source.clone();
            resolved.trust = source.trust.clone();
        }
        resolved.validate()?;
        self.resolved_policy = Some(resolved.clone());
        let old_revision = p.revision;
        let merge: Value = sys.json(
            &format!(
                "recab --json update apply {} --preparation {} --revision {}",
                fixture.release, p.id, p.revision
            ),
            Budget::Stage,
        )?;
        self.save("preserving-release-merge", merge)?;
        let conflicts: Value = sys.json("recab --json update conflicts", Budget::Command)?;
        ensure!(
            conflicts["conflicts"].as_array().is_some_and(|cs| cs
                .iter()
                .any(|c| c["path"] == fixture.conflict_path && c["resolution"].is_null())),
            "fixture did not exercise a real local/release conflict: {conflicts}"
        );
        sys.refuse("recab finalize")?;
        sys.refuse("recab set-next B")?;
        sys.write(
            &format!("{WORK}/probe-release.rhai"),
            &probe_recipe(false, true),
        )?;
        sys.write(
            &format!("{WORK}/metadata-release.rhai"),
            &metadata_recipe(2, true),
        )?;
        self.resolve(&mut sys, &fixture, &resolved, incoming_sequence)?;
        sys.refuse(&format!(
            "recab finalize --preparation {} --revision {old_revision}",
            p.id
        ))?;
        let release_ready = self.observe(&mut sys, "release-merged-X-preserved")?;
        sys.inactive(&release_ready,&format!(r#"test "$(cat -- {})" = 'local X survives the release' && digest=$(sha256sum < {}) && test "${{digest%% *}}" = {}"#,shell_quote(&format!("{MOUNT}/{}",fixture.conflict_path)),shell_quote(&format!("{MOUNT}/{}",fixture.updated_path)),fixture.updated_sha256))?;
        // A failure is a real prepared Recipe selection, sealed in the trial generation.
        sys.write(
            &format!("{WORK}/failure.rhai"),
            &failure_recipe(self.variant.contract.profile.init),
        )?;
        let failure: Value = sys.json(
            &format!("recab --json change install {WORK}/failure.rhai"),
            Budget::Stage,
        )?;
        self.save("prepared-failing-service", failure)?;
        let failed: Finalization = sys.json("recab --json finalize", Budget::Stage)?;
        self.save("failed-trial-generation", serde_json::to_value(&failed)?)?;
        sys.sh("recab set-next B")?;
        self.protected(
            &mut sys,
            &initial,
            &before,
            &original,
            Slot::A,
            "A-protected-through-X-and-update",
        )?;
        sys.reboot()?;
        self.unattended()?;
        let mut sys = self.boot()?;
        sys.wait_failed(Slot::A, Slot::B, &failed.generation.id)?;
        sys.shared(&shared)?;
        ensure!(
            sys.identity()? == identity && sys.inventory()? == original,
            "failed trial changed protected A"
        );
        let failure_state = self.observe(&mut sys, "automatic-fallback-A")?;
        ensure!(
            failure_state
                .slots
                .iter()
                .find(|s| s.slot == Slot::B)
                .and_then(|s| s.generation.as_ref())
                .is_some_and(|g| g.id == failed.generation.id),
            "fallback lost the failed candidate"
        );
        let id = failure_state
            .preparation
            .as_ref()
            .context("failed candidate lost preparation")?
            .id
            .clone();
        sys.sh("recab prepare")?;
        let removed: Value = sys.json(
            "recab --json change remove recab-p13-failure",
            Budget::Stage,
        )?;
        self.save("repair-prepared-service", removed)?;
        ensure!(
            sys.status()?
                .preparation
                .as_ref()
                .is_some_and(|p| p.id == id),
            "repair reseeded X"
        );
        let healthy: Finalization = sys.json("recab --json finalize", Budget::Stage)?;
        sys.sh("recab set-next B")?;
        sys.reboot()?;
        let mut sys = self.boot()?;
        sys.wait_committed(Slot::B)?;
        sys.shared(&shared)?;
        sys.immutable()?;
        ensure!(
            sys.identity()? == identity,
            "healthy B lost machine identity"
        );
        sys.probe(false)?;
        sys.sh("recipe --system isinstalled recab-p13-probe")?;
        sys.verify_metadata(&metadata_x)?;
        sys.verify_update(&fixture)?;
        sys.fidelity()?;
        ensure!(
            sys.sh(&format!("sha256sum {WORK}/shared"))? == shared,
            "shared data was reversed by trial/fallback"
        );
        let committed = self.observe(&mut sys, "healthy-B-committed")?;
        ensure!(
            committed.rollback_slot == Some(Slot::A),
            "A is not retained as previous generation"
        );
        let stable = sys.inventory()?;
        let before = sys.sectors(&committed)?;
        let reverse: Preparation = sys.json("recab --json prepare", Budget::Stage)?;
        ensure!(
            reverse.slot == Slot::A && reverse.source_generation == healthy.generation.id,
            "reverse did not continue committed B"
        );
        ensure!(
            sys.status()?.rollback_slot.is_none(),
            "overwritten old rollback was falsely retained"
        );
        self.save(
            "reverse-incremental-prepare",
            serde_json::to_value(&reverse)?,
        )?;
        sys.write(&format!("{WORK}/probe-Y.rhai"), &probe_recipe(true, false))?;
        let y_before = sys.sectors(&committed)?;
        let y: ChangeProgress = sys.json(
            &format!(
                "recab --json change update recab-p13-probe --producer-recipe {WORK}/probe-Y.rhai"
            ),
            Budget::Stage,
        )?;
        ensure!(
            y.report
                .as_ref()
                .is_some_and(|r| r.payload_bytes_written > 0
                    && r.payload_bytes_written <= 2 * RANGE
                    && r.accounting_complete),
            "small edit rewrote the 32 MiB file: {y:?}"
        );
        ensure!(
            y.producer_io.is_some() && y.invocation_io.is_some(),
            "Y omitted producer/invocation IO"
        );
        self.save(
            "bounded-Y-write-including-Recipe-IO",
            serde_json::to_value(&y)?,
        )?;
        let y_after = sys.sectors(&committed)?;
        let y_blocks = y_after["A"]
            .as_u64()
            .context("A sectors missing")?
            .checked_sub(y_before["A"].as_u64().context("A sectors missing")?)
            .context("block counters reset")?
            .checked_mul(512)
            .context("block count overflow")?;
        ensure!(
            y_blocks > 0 && y_blocks < LARGE,
            "Y rewrote a full large file at the virtual root block device"
        );
        self.save("Y-root-block-IO",json!({"before_sectors":y_before,"after_sectors":y_after,"sector_bytes":512,"written_bytes":y_blocks}))?;
        let metadata_y = self.metadata_change(&mut sys, 3)?;
        ensure!(
            metadata_y["content"] == metadata_x["content"],
            "metadata removal changed its data"
        );
        sys.write(
            &format!("{WORK}/failure.rhai"),
            &failure_recipe(self.variant.contract.profile.init),
        )?;
        let reverse_failure: Value = sys.json(
            &format!("recab --json change install {WORK}/failure.rhai"),
            Budget::Stage,
        )?;
        self.save("reverse-prepared-failing-service", reverse_failure)?;
        let failed_a: Finalization = sys.json("recab --json finalize", Budget::Stage)?;
        sys.sh("recab set-next A")?;
        self.protected(
            &mut sys,
            &committed,
            &before,
            &stable,
            Slot::B,
            "B-protected-through-failed-A-trial",
        )?;
        sys.reboot()?;
        self.unattended()?;
        let mut sys = self.boot()?;
        sys.wait_failed(Slot::B, Slot::A, &failed_a.generation.id)?;
        sys.shared(&shared)?;
        ensure!(
            sys.inventory()? == stable && sys.identity()? == identity,
            "failed A trial changed protected B"
        );
        let reverse_failed = self.observe(&mut sys, "automatic-fallback-B")?;
        let session = reverse_failed
            .preparation
            .as_ref()
            .context("reverse failed candidate lost preparation")?
            .id
            .clone();
        sys.sh("recab prepare")?;
        let repair: Value = sys.json(
            "recab --json change remove recab-p13-failure",
            Budget::Stage,
        )?;
        self.save("repair-reverse-prepared-service", repair)?;
        ensure!(
            sys.status()?
                .preparation
                .as_ref()
                .is_some_and(|p| p.id == session),
            "reverse repair reseeded X/Y"
        );
        let y_final: Finalization = sys.json("recab --json finalize", Budget::Stage)?;
        ensure!(
            y_final.boot_bytes_written == 0 && y_final.reused_boot_files == 2,
            "unchanged reverse boot set was recopied"
        );
        sys.sh("recab set-next A")?;
        self.protected(
            &mut sys,
            &committed,
            &before,
            &stable,
            Slot::B,
            "B-protected-through-Y",
        )?;
        ensure!(
            sys.filesystem_ids(&committed)? == fs_ids,
            "continuation reformatted a slot"
        );
        sys.reboot()?;
        let mut sys = self.boot()?;
        sys.wait_committed(Slot::A)?;
        sys.shared(&shared)?;
        sys.probe(true)?;
        sys.verify_metadata(&metadata_y)?;
        sys.verify_update(&fixture)?;
        sys.fidelity()?;
        ensure!(sys.identity()? == identity, "reverse lost machine identity");
        let after_y = self.observe(&mut sys, "A-holds-X-update-and-Y")?;
        // Repeat the direction change: now the existing 32 MiB file differs by one range.
        let repeated: Preparation = sys.json("recab --json prepare", Budget::Stage)?;
        ensure!(
            repeated.slot == Slot::B
                && repeated
                    .synchronization
                    .as_ref()
                    .is_some_and(|r| r.payload_bytes_written <= 4 * RANGE && r.accounting_complete),
            "repeat continuation copied a whole large file/root"
        );
        self.save(
            "second-direction-change-write-reduction",
            serde_json::to_value(&repeated)?,
        )?;
        let again: Preparation = sys.json("recab --json prepare", Budget::Stage)?;
        ensure!(
            again.id == repeated.id && again.revision == repeated.revision,
            "reopening changed or reseeded the session"
        );
        sys.refuse(&format!(
            "recab discard --preparation {} --revision 0",
            again.id
        ))?;
        // Discard is explicit and cannot resurrect the old B generation.
        sys.sh(&format!(
            "recab discard --preparation {} --revision {}",
            again.id, again.revision
        ))?;
        let discarded = self.observe(&mut sys, "discard-does-not-restore-overwritten-B")?;
        ensure!(
            discarded.rollback_slot.is_none()
                && discarded
                    .slots
                    .iter()
                    .find(|s| s.slot == Slot::B)
                    .is_some_and(|s| !s.generation_ready && !s.bootable),
            "discard restored an overwritten rollback"
        );
        let restarted: Preparation = sys.json("recab --json prepare", Budget::Stage)?;
        ensure!(
            restarted
                .synchronization
                .as_ref()
                .is_some_and(|r| r.payload_bytes_written == 0),
            "unchanged continuation after discard recopied payloads"
        );
        let finalization: Finalization = sys.json("recab --json finalize", Budget::Stage)?;
        ensure!(
            finalization.boot_bytes_written == 0,
            "repeat finalized boot objects were recopied"
        );
        sys.sh("recab set-next B")?;
        sys.reboot()?;
        let mut sys = self.boot()?;
        sys.wait_committed(Slot::B)?;
        sys.shared(&shared)?;
        sys.probe(true)?;
        sys.verify_metadata(&metadata_y)?;
        sys.verify_update(&fixture)?;
        sys.fidelity()?;
        sys.sh("recab rollback")?;
        sys.reboot()?;
        let mut sys = self.boot()?;
        expect(&sys.status()?, Slot::A, true)?;
        sys.probe(true)?;
        sys.verify_metadata(&metadata_y)?;
        sys.immutable()?;
        ensure!(
            sys.identity()? == identity && sys.filesystem_ids(&after_y)? == fs_ids,
            "repeated continuation changed identities"
        );
        sys.shared(&shared)?;
        self.observe(&mut sys, "rollback-to-previous-complete-generation")?;
        sys.poweroff()?;
        self.completed = true;
        self.save("lifecycle-complete",json!({"public_origin":self.public.is_some(),"release":fixture.release,"flavour":self.flavour}))?;
        Ok(format!(
            "A/B lifecycle across {} actual boots; signed release {}; X/update/Y retained; zero unchanged payload writes, bounded large-file writes, Recipe IO and block-sector observations in ab-update.json",
            self.boots, fixture.release
        ))
    }
    fn metadata_change(&mut self, sys: &mut Booted, revision: u8) -> Result<Value> {
        let file = format!("{WORK}/metadata-{revision}.rhai");
        sys.write(&file, &metadata_recipe(revision, false))?;
        let action = if revision == 1 {
            format!("install {file}")
        } else {
            format!("update recab-p13-metadata --producer-recipe {file}")
        };
        let changed: ChangeProgress =
            sys.json(&format!("recab --json change {action}"), Budget::Stage)?;
        ensure!(
            changed
                .report
                .as_ref()
                .is_some_and(|r| r.payload_bytes_written < 32768 && r.accounting_complete),
            "metadata-only change rewrote unrelated system payloads"
        );
        self.save(
            &format!("metadata-only-{revision}"),
            serde_json::to_value(changed)?,
        )?;
        let status = sys.status()?;
        let inventory = sys.inactive(&status, &format!("recab --json inventory --root {MOUNT}"))?;
        let node = metadata_node(&serde_json::from_str(&inventory)?)?;
        let (uid, gid, mode) = if revision == 2 {
            (1003, 1004, 0o644)
        } else {
            (1001, 1002, 0o640)
        };
        ensure!(
            node["uid"] == uid && node["gid"] == gid && node["mode"] == mode,
            "metadata owner/mode did not change: {node}"
        );
        let names = node["xattrs"]
            .as_array()
            .context("metadata xattrs missing")?
            .iter()
            .map(|a| serde_json::from_value::<Vec<u8>>(a[0].clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for name in [
            b"user.recab-p13".as_slice(),
            b"system.posix_acl_access",
            b"security.capability",
        ] {
            ensure!(
                names.iter().any(|n| n == name) == (revision != 3),
                "metadata xattr/ACL/capability update or removal missing"
            );
        }
        self.save(&format!("metadata-observed-{revision}"), node.clone())?;
        Ok(node)
    }
    fn protected(
        &mut self,
        sys: &mut Booted,
        slots: &Status,
        before: &Value,
        inventory: &Value,
        slot: Slot,
        phase: &str,
    ) -> Result<()> {
        ensure!(
            &sys.inventory()? == inventory,
            "operation changed the running system"
        );
        let after = sys.sectors(slots)?;
        ensure!(
            after[slot.to_string()].as_u64() == before[slot.to_string()].as_u64(),
            "immutable running partition received block writes"
        );
        self.save(
            phase,
            json!({"before_sectors":before,"after_sectors":after,"sector_bytes":512}),
        )
    }
    fn resolve(
        &mut self,
        sys: &mut Booted,
        fixture: &Fixture,
        source: &Policy,
        sequence: u64,
    ) -> Result<()> {
        for _ in 0..64 {
            let u: Value = sys.json("recab --json update status", Budget::Command)?;
            if u["merge_complete"] == true {
                return Ok(());
            }
            let status = sys.status()?;
            let op = status
                .operations
                .iter()
                .find(|o| o.id == "update_recover" && o.verdict == recab::Verdict::Ready);
            if let Some(op) = op {
                let value: Value = sys.operation(op, &[])?;
                self.save("recover-captured-merge", value)?;
                continue;
            }
            let conflict = u["merge"]["conflicts"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["resolution"].is_null()))
                .context("merge incomplete without actionable conflict")?;
            let kind = conflict["kind"].as_str().unwrap_or("");
            let op = if kind == "package_rebuild" {
                ensure!(
                    ["recab-p13-probe", "recab-p13-metadata"]
                        .iter()
                        .any(|name| conflict["path"] == *name),
                    "fixture needs an explicit reproduction recipe for selected package: {conflict}"
                );
                status
                    .operations
                    .iter()
                    .find(|o| o.id == "update_reproduce")
            } else {
                let path = conflict["path"].as_str().context("conflict path missing")?;
                ensure!(
                    [
                        fixture.conflict_path.as_str(),
                        distro_contract::release::SOURCE_CONFIG,
                        distro_contract::release::ACQUISITION_RECIPE
                    ]
                    .contains(&path),
                    "unexpected conflict requires a decision: {conflict}"
                );
                let id = format!(
                    "resolve:{}",
                    conflict["id"].as_str().context("conflict id missing")?
                );
                status.operations.iter().find(|o| o.id == id)
            }
            .context("recab did not offer the conflict operation")?;
            ensure!(
                op.verdict == recab::Verdict::Ready,
                "conflict operation refused: {op:?}"
            );
            let args = if kind == "package_rebuild" {
                let package = conflict["path"]
                    .as_str()
                    .context("package conflict missing name")?;
                let recipe = if package == "recab-p13-metadata" {
                    "metadata-release.rhai"
                } else {
                    "probe-release.rhai"
                };
                vec![
                    package.to_owned(),
                    "--recipe".into(),
                    format!("{WORK}/{recipe}"),
                ]
            } else if conflict["path"] == distro_contract::release::SOURCE_CONFIG {
                let mut policy = source.clone();
                policy.sequence = sequence;
                policy.validate()?;
                let path = format!("{WORK}/resolved-update-source.json");
                sys.write(&path, &serde_json::to_string(&policy)?)?;
                vec!["--choice".into(), "edited".into(), "--file".into(), path]
            } else {
                vec!["--choice".into(), "local".into()]
            };
            let value: Value = sys.operation(op, &args)?;
            self.save("explicit-conflict-decision", value)?;
        }
        bail!("merge did not converge after captured recovery/resolution")
    }
    fn boot(&mut self) -> Result<Booted> {
        self.boots += 1;
        let log = self
            .run_dir
            .join(format!("boot-{:02}.serial.log", self.boots));
        self.save("boot-start",json!({"number":self.boots,"serial_log":log.file_name(),"automatic_failure_trial":false}))?;
        let mut vm = session::spawn(VmSpec {
            disk: Some(self.disk),
            serial_log: Some(&log),
            vars: Vars::Existing(self.vars),
            media: self.fixture,
            flavour: self.flavour,
            ..VmSpec::new(self.variant, ScenarioId::AbUpdate)
        })?;
        wait_installed_boot(vm.console()?, self.variant)?;
        vm.console()?.login(
            "root",
            crate::expect::root_password(self.variant),
            Budget::Login.get(),
        )?;
        let mut sys = Booted { vm };
        ensure!(
            distro_contract::profile::Flavour::from_os_release(&sys.sh("cat /etc/os-release")?)?
                == self.flavour,
            "booted system differs from selected base/desktop profile"
        );
        if let Some(public) = &self.public {
            let actual: Policy =
                sys.json("cat /usr/lib/recab/update-source.json", Budget::Command)?;
            let expected = if actual.sequence == public.baseline.sequence {
                &public.source
            } else {
                ensure!(
                    actual.sequence == public.expected.sequence,
                    "booted policy has an unexpected release sequence"
                );
                self.resolved_policy
                    .as_ref()
                    .context("updated policy was not captured from the verified release")?
            };
            ensure!(
                &actual == expected,
                "boot lost its candidate audience or changed the captured release policy"
            );
        }
        Ok(sys)
    }
    fn unattended(&mut self) -> Result<()> {
        self.boots += 1;
        let log = self
            .run_dir
            .join(format!("boot-{:02}.serial.log", self.boots));
        self.save("boot-start",json!({"number":self.boots,"serial_log":log.file_name(),"automatic_failure_trial":true}))?;
        let mut vm = session::spawn(VmSpec {
            disk: Some(self.disk),
            serial_log: Some(&log),
            vars: Vars::Existing(self.vars),
            flavour: self.flavour,
            ..VmSpec::new(self.variant, ScenarioId::AbUpdate)
        })?;
        vm.wait_for_exit(Budget::TrialReboot.get())
            .context("failed trial did not reboot automatically")
    }
}

impl Booted {
    fn console(&mut self) -> Result<&mut Console> {
        self.vm.console()
    }
    fn run(&mut self, command: &str, budget: Budget) -> Result<String> {
        match self.console()?.exec(command, budget.get())? {
            Outcome::Exited {
                code: 0, output, ..
            } => Ok(output.trim().to_owned()),
            o => bail!("'{command}' {}:\n{}", o.describe(), o.output()),
        }
    }
    fn sh(&mut self, s: &str) -> Result<String> {
        self.run(s, Budget::Command)
    }
    fn json<T: DeserializeOwned>(&mut self, s: &str, b: Budget) -> Result<T> {
        let out = self.run(s, b)?;
        let text = out
            .find('{')
            .map(|i| &out[i..])
            .context("command emitted no JSON object")?;
        serde_json::from_str(text).with_context(|| format!("parsing {s}: {out}"))
    }
    fn status(&mut self) -> Result<Status> {
        self.json("recab --json status", Budget::Command)
    }
    fn inventory(&mut self) -> Result<Value> {
        self.json("recab --json inventory --root /", Budget::Stage)
    }
    fn write(&mut self, path: &str, contents: &str) -> Result<()> {
        self.sh(&format!(
            "mkdir -p {WORK}; printf '%s' {} > {}",
            shell_quote(contents),
            shell_quote(path)
        ))?;
        Ok(())
    }
    fn operation<T: DeserializeOwned>(
        &mut self,
        op: &recab::operations::Operation,
        extra: &[String],
    ) -> Result<T> {
        let args = op
            .argv
            .iter()
            .chain(extra)
            .map(|s| shell_quote(s))
            .collect::<Vec<_>>()
            .join(" ");
        self.json(&format!("recab --json {args}"), Budget::Stage)
    }
    fn refuse(&mut self, s: &str) -> Result<()> {
        match self.console()?.exec(s, Budget::Stage.get())? {
            Outcome::Exited { code: 21, .. } => Ok(()),
            o => bail!(
                "unsafe/unexpected success for '{s}': {} {}",
                o.describe(),
                o.output()
            ),
        }
    }
    fn identity(&mut self) -> Result<String> {
        self.sh("test -s /etc/machine-id && test -s /etc/ssh/ssh_host_ed25519_key.pub && sha256sum /etc/machine-id /etc/ssh/ssh_host_*_key.pub")
    }
    fn immutable(&mut self) -> Result<()> {
        self.sh(r#"findmnt -n -o OPTIONS / | tr ',' '\n' | grep -qx ro && test "$(blockdev --getro "$(findmnt -n -o SOURCE /)")" = 1 && test ! -e /etc/recab-p13-refused && if (printf forbidden > /etc/recab-p13-refused) 2>/dev/null; then exit 1; fi"#)?;
        Ok(())
    }
    fn mount_media(&mut self) -> Result<()> {
        self.sh(&format!("mkdir -p {MEDIA}; mountpoint -q {MEDIA} || mount -o ro /dev/disk/by-label/RECAB_P13 {MEDIA}"))?;
        Ok(())
    }
    fn origin(&mut self, policy: &Policy, fixture: &Fixture) -> Result<()> {
        self.mount_media()?;
        let url = url::Url::parse(&policy.source.as_ref().context("source missing")?.index)?;
        ensure!(
            url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.path() == "/index.json",
            "P13 fixture source must use loopback HTTP /index.json"
        );
        let port = url.port_or_known_default().context("origin port missing")?;
        self.sh(&format!("mkdir -p {WORK}; rm -f {WORK}/origin.ready; cp {MEDIA}/origin {WORK}/origin; chmod 755 {WORK}/origin; {WORK}/origin --root {MEDIA}/feed --port {port} --ready {WORK}/origin.ready {} >{WORK}/origin.log 2>&1 </dev/null &",format_args!("--interrupt-once {WORK}/origin.interrupted --interrupt-path {}",shell_quote(&fixture.interrupt_path))))?;
        self.sh(&format!("i=0; while ! test -f {WORK}/origin.ready; do i=$((i+1)); test $i -lt 50 || {{ cat {WORK}/origin.log; exit 1; }}; sleep 0.1; done"))?;
        Ok(())
    }
    fn inactive(&mut self, status: &Status, check: &str) -> Result<String> {
        let device = device(status, status.inactive_slot)?;
        self.run(&format!("(mkdir -p {MOUNT} && mount -t ext4 -o ro,noload,nosuid,nodev,noexec {} {MOUNT} && {{ trap 'umount {MOUNT}' 0; {check}; }})",shell_quote(device)),Budget::Stage)
    }
    fn sectors(&mut self, status: &Status) -> Result<Value> {
        self.sh("sync")?;
        let mut out = serde_json::Map::new();
        for s in [Slot::A, Slot::B] {
            let d = device(status, s)?;
            let result = self.sh(&format!(
                "d=$(basename \"$(readlink -f {})\"); cat /sys/class/block/\"$d\"/stat",
                shell_quote(d)
            ))?;
            let n = result
                .split_whitespace()
                .nth(6)
                .context("partition block counters missing")?
                .parse::<u64>()?;
            out.insert(s.to_string(), json!(n));
        }
        Ok(Value::Object(out))
    }
    fn filesystem_ids(&mut self, status: &Status) -> Result<String> {
        let mut out = String::new();
        for slot in [Slot::A, Slot::B] {
            let d = device(status, slot)?;
            out.push_str(&self.sh(&format!(
                "blkid -c /dev/null -s UUID -o value {}",
                shell_quote(d)
            ))?);
        }
        Ok(out)
    }
    fn wait_committed(&mut self, slot: Slot) -> Result<()> {
        let end = Instant::now() + Budget::Commit.get();
        loop {
            let s = self.status()?;
            if s.active_slot == slot && s.committed {
                return Ok(());
            }
            ensure!(
                Instant::now() < end,
                "healthy {slot} was not committed: {s:?}"
            );
            thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    fn wait_failed(&mut self, source: Slot, target: Slot, g: &str) -> Result<()> {
        let end = Instant::now() + Budget::Commit.get();
        loop {
            let s = self.status()?;
            if s.active_slot == source
                && s.committed
                && s.last_trial.as_ref().is_some_and(|t| {
                    t.slot == target && !t.committed && t.generation.as_deref() == Some(g)
                })
            {
                return Ok(());
            }
            ensure!(
                Instant::now() < end,
                "failed trial was not reconciled to {source}: {s:?}"
            );
            thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    fn verify_metadata(&mut self, expected: &Value) -> Result<()> {
        ensure!(
            &metadata_node(&self.inventory()?)? == expected,
            "metadata-only intent was lost across release/trial/continuation"
        );
        Ok(())
    }
    fn shared(&mut self, expected: &str) -> Result<()> {
        ensure!(
            self.sh(&format!("sha256sum {WORK}/shared"))? == expected,
            "shared data changed across continuation/trial/rollback"
        );
        self.sh(&format!("test \"$(cat {WORK}/user-store/profile/share/recab-p13-user.txt)\" = 'independent user profile'"))?;
        Ok(())
    }
    fn fidelity(&mut self) -> Result<()> {
        self.sh(r#"p=/etc/p13-local; test "$(stat -c %i $p/hard-a)" = "$(stat -c %i $p/hard-b)" && test "$(stat -c '%u:%g:%a' $p/hard-a)" = 1001:1002:640 && test "$(readlink $p/symlink)" = hard-a && test "$(stat -c %s $p/sparse)" = 16777216 && test "$(stat -c %b $p/sparse)" -lt 128 && raw=$(printf 'raw-\377') && test "$(cat $p/"$raw")" = 'non-UTF8 system name'"#)?;
        Ok(())
    }
    fn probe(&mut self, y: bool) -> Result<()> {
        self.sh(&format!("test \"$(cat /{PROBE}/X)\" = X && test \"$(stat -c %s /{PROBE}/large)\" = {LARGE} && test \"$(head -c 1 /{PROBE}/large)\" = {} && {}",if y{"Y"}else{"a"},if y{format!("test -e /{PROBE}/Y && test ! -e /{PROBE}/delete-me")}else{format!("test -e /{PROBE}/delete-me")}))?;
        Ok(())
    }
    fn verify_update(&mut self, f: &Fixture) -> Result<()> {
        self.sh(&format!(r#"digest=$(sha256sum < {}) && test "${{digest%% *}}" = {} && test "$(cat -- {})" = 'local X survives the release'"#,shell_quote(&format!("/{}",f.updated_path)),f.updated_sha256,shell_quote(&format!("/{}",f.conflict_path))))?;
        Ok(())
    }
    fn reboot(mut self) -> Result<()> {
        self.vm.shutdown_from_guest("reboot")
    }
    fn poweroff(mut self) -> Result<()> {
        self.vm.shutdown_from_guest("poweroff")
    }
}
fn device(status: &Status, slot: Slot) -> Result<&str> {
    status
        .slots
        .iter()
        .find(|s| s.slot == slot)
        .and_then(|s| s.device.as_deref())
        .context("recab did not identify the slot device")
}
fn generation(status: &Status, slot: Slot) -> Result<&str> {
    status
        .slots
        .iter()
        .find(|s| s.slot == slot)
        .and_then(|s| s.generation.as_ref())
        .map(|g| g.id.as_str())
        .context("complete generation missing")
}
fn expect(status: &Status, slot: Slot, committed: bool) -> Result<()> {
    ensure!(
        status.active_slot == slot && status.committed == committed,
        "expected {slot} committed={committed}: {status:?}"
    );
    Ok(())
}
fn same_preparation(status: &Status, p: &Preparation) -> Result<()> {
    ensure!(
        status.preparation.as_ref().is_some_and(|now| now == p),
        "acquisition changed the preparation"
    );
    ensure!(!status.trial_pending, "acquisition scheduled a boot");
    Ok(())
}

fn metadata_node(inventory: &Value) -> Result<Value> {
    let node = inventory["entries"]
        .as_array()
        .context("inventory entries missing")?
        .iter()
        .find(|n| n["path"] == json!(b"usr/share/recab-p13-metadata/file".as_slice()))
        .context("metadata probe absent")?;
    Ok(
        json!({"content":node["content"],"uid":node["uid"],"gid":node["gid"],"mode":node["mode"],"xattrs":node["xattrs"]}),
    )
}

fn boot_selection(status: &Status) -> Value {
    json!({"active":status.active_slot,"next":status.next_boot,"default":status.default_slot,
        "known_good":status.known_good_slot,"scheduled":status.scheduled_trial,"trial_pending":status.trial_pending})
}

/// Authenticate the actual guest pin and bind its observed transport to the
/// immutable publisher record expected by the descriptor, without fetching any
/// bytes outside the installed Recipe coordinator.
fn public_pin(public: &PublicUpdate, pin: &Value, distro: &str) -> Result<u64> {
    use distro_contract::release::{self as f, production::*};
    let proof = &pin["production"];
    let authority: AuthorityProof = serde_json::from_value(proof["authority"].clone())?;
    let at = proof["accepted_at"]
        .as_u64()
        .context("pin acceptance time missing")?;
    let policy = public.source.production.as_ref().unwrap();
    let verified = authority.verify(
        policy,
        &public.expected.distro,
        &public.expected.channel,
        at,
    )?;
    let raw = proof["index"].as_str().context("signed index missing")?;
    let signature: Vec<u8> = serde_json::from_value(proof["signature"].clone())?;
    let index = ReleaseIndexV2::authenticate(raw.as_bytes(), &signature, &verified, policy, at)?;
    let raw = pin["manifest_json"].as_str().context("manifest missing")?;
    let manifest = f::ReleaseManifest::parse(raw.as_bytes())?;
    ensure!(
        f::digest(raw.as_bytes()) == public.release && manifest.identity == public.expected,
        "public discovery differs from exact expected identity/id"
    );
    let signature: Vec<u8> = serde_json::from_value(pin["signature"].clone())?;
    verified.verify(SigningRole::Release, raw.as_bytes(), &signature)?;
    let entry = index
        .releases
        .into_iter()
        .find(|e| e.release.manifest.sha256 == public.release)
        .context("expected release missing from signed transport")?;
    entry.for_manifest(&manifest, policy)?;
    ensure!(
        entry.release.manifest.size == raw.len() as u64
            && entry.release.signature.size == signature.len() as u64
            && entry.release.signature.sha256 == f::digest(&signature),
        "pin signature/manifest bytes differ from the signed publication"
    );
    let total = entry.parts.values().map(|p| p.len() as u64).sum();
    let record = distro_builder::release_index::production_publish::Publication {
        schema_version: 1,
        distro: distro.into(),
        source_commit: public.source_commit.clone(),
        id: public.release.clone(),
        tag: format!(
            "os-{distro}-x86_64-{}-{}-{}",
            public.expected.flavour,
            public.expected.sequence,
            &public.release[..16]
        ),
        entry,
    };
    ensure!(
        f::digest(&serde_json::to_vec_pretty(&record)?) == public.publication_sha256,
        "guest transport/source does not match the exact immutable publication record"
    );
    Ok(total)
}
impl Booted {
    fn release_policy(&mut self, release: &str) -> Result<Policy> {
        let raw = self.sh(&format!(
            "cat /var/lib/recab/releases/{release}/release.json"
        ))?;
        let manifest = distro_contract::release::ReleaseManifest::parse(raw.as_bytes())?;
        let path = format!(
            "/var/lib/recab/releases/{release}/{}",
            manifest.artifacts.rootfs.path
        );
        let mount = "/run/recab-p13-release-policy";
        self.json(&format!("(mkdir -p {mount} && mount -t erofs -o loop,ro,nosuid,nodev,noexec {} {mount} && {{ trap 'umount {mount}' 0; cat {mount}/usr/lib/recab/update-source.json; }})",shell_quote(&path)),Budget::Command)
    }
    fn cancel_public_download(&mut self, release: &str, total: u64) -> Result<()> {
        ensure!(total >= 4, "public transport has too few members");
        let command = format!(
            r#"(
set -eu
mkdir -p {WORK}
test ! -e /var/lib/recab/releases/{release}/release.json
rm -f {WORK}/download.pid {WORK}/download.done
setsid sh -c 'echo $$ > {WORK}/download.pid; set +e; recab --json update download {release} > {WORK}/public-download.json 2> {WORK}/public-download.log; code=$?; echo "$code" > {WORK}/download.done' </dev/null >/dev/null 2>&1 &
i=0
while ! test -s {WORK}/download.pid; do i=$((i+1)); test "$i" -lt 100; sleep 0.1; done
pid=$(cat {WORK}/download.pid)
case "$pid" in ''|*[!0-9]*) exit 1;; esac
trap 'kill -KILL -"$pid" 2>/dev/null || true' 0
i=0
while :; do
    if test -e {WORK}/download.done; then cat {WORK}/public-download.log; echo 'public transfer ended before interruption could be observed' >&2; exit 1; fi
    parts=$(sed -n 's/.*"verified_parts":\([0-9][0-9]*\).*/\1/p' /var/lib/recab/updates/status.json)
    if test -n "$parts" && test "$parts" -gt 0 && test "$parts" -lt {total}; then
        kill -STOP -"$pid"
        cp /var/lib/recab/updates/status.json {WORK}/public-interrupted-status.json
        parts=$(sed -n 's/.*"verified_parts":\([0-9][0-9]*\).*/\1/p' {WORK}/public-interrupted-status.json)
        test "$parts" -gt 0 && test "$parts" -lt {total}
        test ! -e /var/lib/recab/releases/{release}/release.json
        kill -KILL -"$pid"
        break
    fi
    i=$((i+1)); test "$i" -lt 30000; sleep 0.1
done
)"#
        );
        self.run(&command, Budget::PublicDownload)?;
        Ok(())
    }
}
