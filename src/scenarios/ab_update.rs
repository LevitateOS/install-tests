//! Real A/B continuation across boots, against an explicit signed update fixture.
//! No scenario builds a release, edits the running root, or mounts a candidate writable.
use super::{Harness, InstallRun, ScenarioId};
use crate::update_fixture::{Fixture, failure_recipe, probe_recipe};
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
use std::{cell::Cell, fs, path::Path, thread, time::Instant};

pub(super) const MEDIA: &str = "/run/recab-p13-media";
const WORK: &str = "/var/tmp/recab-p13";
const MOUNT: &str = "/run/recab-p13-readonly";
const PROBE: &str = "usr/share/recab-p13";
const LARGE: u64 = 32 * 1024 * 1024;
const RANGE: u64 = 1024 * 1024;

pub(super) fn run(
    h: &Harness,
    install: &InstallRun,
    fixture: &Path,
    run_dir: &Path,
) -> Result<String> {
    let installed = distro_contract::runs::read(
        install
            .disk_path
            .parent()
            .context("install directory missing")?,
    )
    .map_err(anyhow::Error::msg)?;
    ensure!(
        installed.update_fixture.is_some(),
        "ab-update needs an existing install configured with a signed update fixture before sealing"
    );
    let (disk, vars) = super::overlay_install(install, run_dir)?;
    Machine {
        variant: &h.variant,
        disk: &disk,
        vars: &vars,
        fixture,
        run_dir,
        boots: Cell::new(0),
        records: Vec::new(),
    }
    .exercise()
}
struct Machine<'a> {
    variant: &'a Variant,
    disk: &'a Path,
    vars: &'a Path,
    fixture: &'a Path,
    run_dir: &'a Path,
    boots: Cell<u32>,
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
                "schema_version":1,"boots":self.boots.get(),"observations":self.records,
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
            json!({"guest":sys.sh("uname -m")?,"required_architectures":["x86_64","aarch64"]}),
        )?;
        sys.mount_media()?;
        let fixture: Fixture = sys.json(&format!("cat {MEDIA}/fixture.json"), Budget::Command)?;
        fixture.validate()?;
        let architecture = sys.sh("uname -m")?;
        let expected = match fixture.architecture {
            distro_contract::release::Architecture::X86_64 => "x86_64",
            distro_contract::release::Architecture::Aarch64 => "aarch64",
        };
        ensure!(
            architecture == expected,
            "update fixture architecture does not match the installed system"
        );
        let source: Policy = sys.json(&format!("cat {MEDIA}/source.json"), Budget::Command)?;
        source.validate()?;
        let installed: Policy =
            sys.json("cat /usr/lib/recab/update-source.json", Budget::Command)?;
        ensure!(
            serde_json::to_value(&source)? == serde_json::to_value(&installed)?,
            "install does not contain this fixture's public source policy; supply an existing install with the matching public source policy"
        );
        ensure!(
            source.source.is_some() && source.trust.is_some(),
            "fixture needs a signed update source"
        );
        let identity = sys.identity()?;
        ensure!(!identity.is_empty(), "initial machine identity is missing");
        let fs_ids = sys.filesystem_ids(&initial)?;
        let original = sys.inventory()?;
        let shared = sys.sh(&format!(
            "mkdir -p {WORK}; printf 'shared user data\n' > {WORK}/shared; sha256sum {WORK}/shared"
        ))?;
        // B is exercised at first use, before any continuation writes it.
        sys.sh("recab set-next B")?;
        sys.reboot()?;
        let mut sys = self.boot()?;
        sys.wait_committed(Slot::B)?;
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
        sys.origin(&source, &fixture)?;
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
        sys.write(&format!("{WORK}/X"), "local X survives the release\n")?;
        let edit: Value = sys.json(
            &format!(
                "recab --json edit {} --file {WORK}/X",
                shell_quote(&fixture.conflict_path)
            ),
            Budget::Stage,
        )?;
        self.save("configuration-X", edit)?;
        let p = sys.status()?.preparation.context("X has no preparation")?;
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
        self.save("signed-discovery", discovery)?;
        sys.refuse("recab --json update download")?; // origin deliberately truncates one EROFS transfer.
        let interrupted = self.observe(&mut sys, "interrupted-download")?;
        same_preparation(&interrupted, &p)?;
        let download: Value = sys.json("recab --json update download", Budget::Stage)?;
        ensure!(
            download["download"]["downloaded"] == true
                && download["download"]["release"] == fixture.release,
            "download did not verify the selected complete release"
        );
        self.save("verified-download", download)?;
        let downloaded = self.observe(&mut sys, "X-after-download")?;
        same_preparation(&downloaded, &p)?;
        ensure!(
            sys.inventory()? == original,
            "check/download changed the running root"
        );
        sys.inactive(&downloaded,&format!("test \"$(cat -- {})\" = 'local X survives the release' && test -e {MOUNT}/{PROBE}/X",shell_quote(&format!("{MOUNT}/{}",fixture.conflict_path))))?;
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
        self.resolve(&mut sys, &fixture)?;
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
        sys.immutable()?;
        ensure!(
            sys.identity()? == identity,
            "healthy B lost machine identity"
        );
        sys.probe(false)?;
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
        sys.probe(true)?;
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
        sys.probe(true)?;
        sys.verify_update(&fixture)?;
        sys.fidelity()?;
        sys.sh("recab rollback")?;
        sys.reboot()?;
        let mut sys = self.boot()?;
        expect(&sys.status()?, Slot::A, true)?;
        sys.probe(true)?;
        sys.immutable()?;
        ensure!(
            sys.identity()? == identity && sys.filesystem_ids(&after_y)? == fs_ids,
            "repeated continuation changed identities"
        );
        self.observe(&mut sys, "rollback-to-previous-complete-generation")?;
        sys.poweroff()?;
        Ok(format!(
            "A/B lifecycle across {} actual boots; signed release {}; X/update/Y retained; zero unchanged payload writes, bounded large-file writes, Recipe IO and block-sector observations in ab-update.json",
            self.boots.get(),
            fixture.release
        ))
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
    fn resolve(&mut self, sys: &mut Booted, fixture: &Fixture) -> Result<()> {
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
                    conflict["path"] == "recab-p13-probe",
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
                vec![
                    "recab-p13-probe".to_owned(),
                    "--recipe".into(),
                    format!("{WORK}/probe-release.rhai"),
                ]
            } else {
                vec!["--choice".into(), "local".into()]
            };
            let value: Value = sys.operation(op, &args)?;
            self.save("explicit-conflict-decision", value)?;
        }
        bail!("merge did not converge after captured recovery/resolution")
    }
    fn boot(&self) -> Result<Booted> {
        self.boots.set(self.boots.get() + 1);
        let mut vm = session::spawn(VmSpec {
            disk: Some(self.disk),
            vars: Vars::Existing(self.vars),
            media: Some(self.fixture),
            ..VmSpec::new(self.variant, ScenarioId::AbUpdate)
        })?;
        wait_installed_boot(vm.console()?, self.variant)?;
        vm.console()?.login(
            "root",
            crate::expect::root_password(self.variant),
            Budget::Login.get(),
        )?;
        Ok(Booted { vm })
    }
    fn unattended(&self) -> Result<()> {
        self.boots.set(self.boots.get() + 1);
        let mut vm = session::spawn(VmSpec {
            disk: Some(self.disk),
            vars: Vars::Existing(self.vars),
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
        self.sh(&format!("mkdir -p {WORK}; rm -f {WORK}/origin.ready; cp {MEDIA}/origin {WORK}/origin; chmod 755 {WORK}/origin; {WORK}/origin --root {MEDIA}/feed --port {port} --ready {WORK}/origin.ready {} >{WORK}/origin.log 2>&1 </dev/null &",format!("--interrupt-once {WORK}/origin.interrupted --interrupt-path {}",shell_quote(&fixture.interrupt_path))))?;
        self.sh(&format!("i=0; while ! test -f {WORK}/origin.ready; do i=$((i+1)); test $i -lt 50 || {{ cat {WORK}/origin.log; exit 1; }}; sleep 0.1; done"))?;
        Ok(())
    }
    fn inactive(&mut self, status: &Status, check: &str) -> Result<()> {
        let device = device(status, status.inactive_slot)?;
        self.run(&format!("(mkdir -p {MOUNT} && mount -t ext4 -o ro,noload,nosuid,nodev,noexec {} {MOUNT} && {{ trap 'umount {MOUNT}' 0; {check}; }})",shell_quote(device)),Budget::Stage)?;
        Ok(())
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
        status
            .preparation
            .as_ref()
            .is_some_and(|now| now.id == p.id && now.revision == p.revision),
        "acquisition changed the preparation"
    );
    ensure!(!status.trial_pending, "acquisition scheduled a boot");
    Ok(())
}
