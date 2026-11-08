//! Scenario-based development loop.
//!
//! Independent named scenarios give focused feedback during development.
//! Each scenario consumes existing artifacts and validates one behavior. The
//! scenarios, their inputs and their order are `distro_contract::ScenarioId`;
//! what each expects of a variant is `crate::expect`. Every run leaves a
//! `distro_contract::runs` record in its own run directory; the newest
//! finished record of a scenario is its status.

mod ab_update;
mod install;
use crate::expect::{self, Evidence};
use crate::preflight::require_preflight;
use crate::qemu::session;
use crate::qemu::{Console, SerialExecutorExt};
use crate::boot_injection::{BootInjection, SSH_AUTHORIZED_KEY};
use anyhow::{bail, Context, Result};
use colored::Colorize;
use distro_contract::profile::{InitSystem, PrivilegeTool};
use distro_contract::runs::{self, Run, RunManifest, RunRef, RunStatus};
use distro_contract::{Missing, ProductId, Variant};
use recpart::InstallMode;
use recshuttle::{Login, Remote};
use distro_builder::install_ux::{LAUNCHER, PROBE_PREFIX, SMOKE_ENV, SPLIT, SPLIT_SMOKE_OK};
use crate::qemu::session::{Ssh, Vars, VmSpec};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use distro_contract::ScenarioId;

/// The install run's disk and UEFI variable store, in its run directory.
pub(super) const INSTALL_DISK_FILENAME: &str = "disk.qcow2";
pub(super) const INSTALL_OVMF_VARS_FILENAME: &str = "ovmf-vars.fd";
/// Where the scenarios' guest-side scripts live.
const TEST_SCRIPTS_DIR: &str = "testing/install-tests/test-scripts";
/// The harness's own sources: a change to any of them reruns cached results.
const HARNESS_SOURCES: &[&str] = &[
    "testing/install-tests/src",
    TEST_SCRIPTS_DIR,
    "tools/recqemu/src",
    "tools/recshuttle/src",
];

/// How the install scenario installs, and so what the installed-system
/// scenarios boot: the A/B layout that recab updates.
pub(super) const INSTALL_MODE: InstallMode = InstallMode::Ab;

/// The latest successful install run: its disk and UEFI variables, which the
/// installed-system scenarios boot through overlays and never change.
#[derive(Debug, Clone)]
pub struct InstallRun {
    pub run: RunRef,
    pub disk_path: PathBuf,
    pub ovmf_vars_path: PathBuf,
}

/// A release ISO and the run that built it.
pub(crate) struct ReleaseIso {
    product: ProductId,
    pub(crate) iso: PathBuf,
    run: RunRef,
}

/// A scenario's inputs (`ScenarioId::input`), resolved to what exists on disk.
struct Inputs {
    iso: Option<ReleaseIso>,
    install: Option<InstallRun>,
}

impl Inputs {
    fn resolve(repo: &Path, distro_id: &str, scenario: ScenarioId) -> Result<Self> {
        let input = scenario.input();
        Ok(Self {
            iso: input
                .iso
                .map(|product| resolve_release(repo, distro_id, product))
                .transpose()?,
            install: input
                .install_run
                .then(|| resolve_latest_install_run(repo, distro_id))
                .transpose()?,
        })
    }

    fn iso(&self, scenario: ScenarioId) -> Result<&Path> {
        self.iso
            .as_ref()
            .map(|release| release.iso.as_path())
            .with_context(|| format!("the registry gives {scenario} no ISO input"))
    }

    fn install(&self, scenario: ScenarioId) -> Result<&InstallRun> {
        self.install
            .as_ref()
            .with_context(|| format!("the registry gives {scenario} no install-run input"))
    }

    /// The runs these inputs come from, for the run record.
    fn refs(&self) -> Vec<RunRef> {
        self.iso
            .iter()
            .map(|release| release.run.clone())
            .chain(self.install.iter().map(|install| install.run.clone()))
            .collect()
    }
}

/// The variant `distro_id` of the checkout at `repo`, loaded once for the run.
fn load_variant(repo: &Path, distro_id: &str) -> Result<Variant> {
    Ok(distro_contract::load(repo, distro_id)?)
}

/// The newest finished run of `scenario`: its record, or why it cannot be read.
fn latest_result(variant: &Variant, scenario: ScenarioId) -> Result<Option<Result<RunManifest, String>>> {
    let dir = runs::scenario_dir(&variant.repo_root, &variant.id, scenario);
    Ok(runs::list(&dir)?.into_iter().map(|run| run.manifest).find(|manifest| {
        !matches!(manifest, Ok(m) if m.status == RunStatus::Building)
    }))
}

/// How `run` runs the scenarios.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Rerun scenarios that already passed for the same inputs.
    pub force: bool,
    /// Extra `KEY=VALUE` lines for every live boot's injection.
    pub injection: Vec<String>,
    /// The private key the guests trust (its `.pub` is injected); default:
    /// the checkout's guest key, created on first use.
    pub login_key: Option<PathBuf>,
}

/// What became of one scenario.
#[derive(Debug)]
pub enum Outcome {
    /// It ran and passed: what it proved.
    Passed(String),
    /// A passing run for the same inputs exists.
    Cached,
    /// It ran and failed, or could not start.
    Failed(anyhow::Error),
    /// An input it needs does not exist.
    Missing(Missing),
    /// It did not run because a scenario it consumes failed in this run.
    NotRun(String),
}

/// What every scenario of one run shares: the variant, loaded once, the
/// guest login and the extra injection.
pub(crate) struct Harness {
    pub variant: Variant,
    pub login: Option<Login>,
    pub injection: Vec<String>,
}

impl Harness {
    pub(crate) fn new(repo: &Path, distro_id: &str, options: &RunOptions, boots: bool) -> Result<Self> {
        let variant = load_variant(repo, distro_id)?;
        let login = if boots {
            let key = match &options.login_key {
                Some(key) => key.clone(),
                None => ensure_login_key(repo)?,
            };
            Some(Login { key, user: "root".to_string() })
        } else {
            None
        };
        Ok(Self {
            variant,
            login,
            injection: options.injection.clone(),
        })
    }

    fn login(&self) -> Result<&Login> {
        self.login.as_ref().context("this run boots no VM, so it resolved no login key")
    }

    /// The guest behind a forwarded SSH port.
    pub(crate) fn remote(&self, port: u16) -> Result<Remote> {
        Ok(Remote::new(port, self.login()?.clone()))
    }

    /// The injection a live boot gets: the login key, `extra`, then the run's own lines.
    pub(crate) fn live_injection(&self, extra: &[String]) -> Result<BootInjection> {
        let mut lines = vec![format!("{SSH_AUTHORIZED_KEY}={}", recshuttle::public_key(&self.login()?.key)?)];
        lines.extend(extra.iter().cloned());
        lines.extend(self.injection.iter().cloned());
        BootInjection::new(&lines)
    }
}

/// The checkout's guest login key (`runs::ssh_key`), created on first use.
pub fn ensure_login_key(repo: &Path) -> Result<PathBuf> {
    let key = runs::ssh_key(repo);
    recshuttle::ensure_key(&key)?;
    Ok(key)
}

/// Run `scenarios` for a distro of the checkout at `repo`, every one of
/// them, each on its own against the artifacts that exist; a failure does
/// not stop the rest, but a scenario that consumes the install run is not
/// run after the install scenario failed in the same call. Progress goes to
/// stderr.
pub fn run(
    repo: &Path,
    distro_id: &str,
    scenarios: &[ScenarioId],
    options: &RunOptions,
) -> Result<Vec<(ScenarioId, Outcome)>> {
    let boots = scenarios.iter().any(|s| *s != ScenarioId::BuildPreflight);
    let harness = Harness::new(repo, distro_id, options, boots)?;
    let mut outcomes = Vec::new();
    let mut install_failed = false;
    for &scenario in scenarios {
        let outcome = if install_failed && scenario.input().install_run {
            Outcome::NotRun(format!("needs a passing install, and install failed in this run"))
        } else {
            match run_one(&harness, scenario, options.force) {
                Ok(Some(evidence)) => Outcome::Passed(evidence),
                Ok(None) => Outcome::Cached,
                Err(err) => match err.downcast_ref::<Missing>() {
                    Some(missing) => Outcome::Missing(missing.clone()),
                    None => Outcome::Failed(err),
                },
            }
        };
        if scenario == ScenarioId::Install && !matches!(outcome, Outcome::Passed(_) | Outcome::Cached) {
            install_failed = true;
        }
        outcomes.push((scenario, outcome));
    }
    Ok(outcomes)
}

/// Run one scenario; `None` when a passing run for the same inputs exists
/// and `force` is not set. A failure is the error, with its cause.
fn run_one(h: &Harness, scenario: ScenarioId, force: bool) -> Result<Option<String>> {
    let variant = &h.variant;
    let repo = variant.repo_root.as_path();
    let distro_id = variant.id.as_str();
    let inputs = Inputs::resolve(repo, distro_id, scenario)?;
    let fingerprint = scenario_input_fingerprint(variant, &inputs)?;

    if force {
        eprintln!("Forcing {} rerun.", scenario.title());
    } else if let Some(Ok(last)) = latest_result(variant, scenario)? {
        if last.status == RunStatus::Success && last.fingerprint.as_deref() == Some(fingerprint.as_str()) {
            eprintln!(
                "{} {} already passed (rerun with --force).",
                "[SKIP]".green(),
                scenario.title()
            );
            return Ok(None);
        }
    }

    if let Some(release) = &inputs.iso {
        require_preflight(&release.iso, release.product, variant)?;
    }

    eprintln!("{} {}", ">>".cyan(), scenario.title());
    let mut run = Run::start_scenario(repo, distro_id, scenario)?;
    run.manifest.inputs = inputs.refs();
    run.manifest.fingerprint = Some(fingerprint);
    run.save()?;

    let result = match scenario {
        ScenarioId::BuildPreflight => inputs.iso(scenario).map(|iso| {
            format!(
                "Preflight conformance + artifact checks passed for {}",
                iso.display()
            )
        }),
        ScenarioId::LiveBoot => inputs.iso(scenario).and_then(|iso| run_live_boot(h, iso)),
        ScenarioId::LiveTools => inputs.iso(scenario).and_then(|iso| run_live_tools(h, iso)),
        ScenarioId::Install => inputs.iso(scenario).and_then(|iso| run_installation(h, iso, &mut run)),
        ScenarioId::InstalledBoot => inputs
            .install(scenario)
            .and_then(|install| run_installed_boot(h, install, &run.dir)),
        ScenarioId::AutomatedLogin => inputs
            .install(scenario)
            .and_then(|install| run_automated_login(h, install, &run.dir)),
        ScenarioId::Runtime => inputs
            .install(scenario)
            .and_then(|install| run_runtime(h, install, &run.dir)),
        ScenarioId::AbUpdate => inputs
            .iso(scenario)
            .and_then(|iso| ab_update::run(h, inputs.install(scenario)?, iso, &run.dir)),
    };

    run.manifest.evidence = Some(match &result {
        Ok(evidence) => evidence.clone(),
        Err(err) => format!("FAIL: {err:#}"),
    });
    let recorded = run
        .finish(result.is_ok())
        .with_context(|| format!("recording the {scenario} run for {distro_id}"))
        .and_then(|_| {
            let dir = runs::scenario_dir(repo, distro_id, scenario);
            Ok(runs::prune(repo, distro_id, &dir, runs::SCENARIO_RETENTION)?)
        });
    match result {
        Ok(evidence) => {
            recorded?;
            eprintln!(
                "{} {} passed: {}",
                "[PASS]".green().bold(),
                scenario.title(),
                evidence
            );
            Ok(Some(evidence))
        }
        Err(e) => {
            print_failure(scenario, &e);
            let e = e.context(format!("scenario {scenario} failed for {distro_id}"));
            Err(match recorded {
                Ok(()) => e,
                Err(record) => e.context(format!("and its run record failed: {record:#}")),
            })
        }
    }
}

/// Where a scenario stands for a distro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScenarioStatus {
    NotRun,
    Pass,
    Fail,
    /// Has a result, but its inputs changed since.
    Stale,
    /// Its latest record cannot be read, or its inputs cannot be resolved
    /// now (an ISO or the install run is missing or unreadable): why.
    Error(String),
}

impl ScenarioStatus {
    /// `not-run`, `pass`, `fail`, `stale` or `error`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotRun => "not-run",
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Stale => "stale",
            Self::Error(_) => "error",
        }
    }

    /// Why an `Error` status is one.
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Error(why) => Some(why),
            _ => None,
        }
    }
}

/// Each scenario's status for a distro, in run order, from its run records.
pub fn scenario_statuses(repo: &Path, distro_id: &str) -> Result<Vec<(ScenarioId, ScenarioStatus)>> {
    statuses(&load_variant(repo, distro_id)?)
}

fn statuses(variant: &Variant) -> Result<Vec<(ScenarioId, ScenarioStatus)>> {
    ScenarioId::ALL
        .into_iter()
        .map(|scenario| {
            let last = match latest_result(variant, scenario)? {
                None => return Ok((scenario, ScenarioStatus::NotRun)),
                Some(Err(why)) => return Ok((scenario, ScenarioStatus::Error(why))),
                Some(Ok(last)) => last,
            };
            let fingerprint = Inputs::resolve(&variant.repo_root, &variant.id, scenario)
                .and_then(|inputs| scenario_input_fingerprint(variant, &inputs));
            let status = match fingerprint {
                Err(err) => ScenarioStatus::Error(format!("{err:#}")),
                Ok(fingerprint) if last.fingerprint.as_deref() != Some(fingerprint.as_str()) => {
                    ScenarioStatus::Stale
                }
                Ok(_) if last.status == RunStatus::Success => ScenarioStatus::Pass,
                Ok(_) => ScenarioStatus::Fail,
            };
            Ok((scenario, status))
        })
        .collect()
}

/// Remove every scenario run of a distro (their records and disks).
pub fn reset(repo: &Path, distro_id: &str) -> Result<()> {
    let variant = load_variant(repo, distro_id)?;
    for scenario in ScenarioId::ALL {
        let dir = runs::scenario_dir(repo, &variant.id, scenario);
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("removing '{}'", dir.display()))?;
        }
    }
    Ok(())
}

/// Live Boot scenario — ISO boots in QEMU.
fn run_live_boot(h: &Harness, iso_path: &Path) -> Result<String> {
    let variant = &h.variant;
    let injection = h.live_injection(&[])?;
    let mut vm = session::spawn(VmSpec {
        iso: Some(iso_path),
        ssh: Ssh::Free,
        injection: Some(&injection),
        ..VmSpec::new(variant, ScenarioId::LiveBoot)
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = vm.console()?;
    console.wait_for_live_boot_with_context(live_boot_stall(variant), variant)?;
    verify_live_boot_ssh_login(h, console, ssh_host_port)?;

    evidence_over_ssh(h, ssh_host_port, &scenario_evidence(ScenarioId::LiveBoot)?, &[])?;
    evidence_over_ssh(
        h,
        ssh_host_port,
        &Evidence {
            script: "live-boot-ssh-preflight.sh".to_string(),
            pass_marker: "OPENSSH PREFLIGHT PASSED".to_string(),
        },
        &[],
    )?;

    Ok(
        "Boot markers detected + SSH login probe passed + live-boot script checks passed"
            .to_string(),
    )
}

/// Live Tools scenario — expected binaries in the live environment.
///
/// Each tool is run with a harmless invocation where it has one (see
/// [`check_tool`]), so missing libraries or a broken binary fail, not just
/// absence from PATH. The live overlay's behavior is checked too.
fn run_live_tools(h: &Harness, iso_path: &Path) -> Result<String> {
    let variant = &h.variant;
    let injection = h.live_injection(&[])?;
    let mut vm = session::spawn(VmSpec {
        iso: Some(iso_path),
        ssh: Ssh::Free,
        injection: Some(&injection),
        ..VmSpec::new(variant, ScenarioId::LiveTools)
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = vm.console()?;
    wait_for_live_tools_serial_readiness(console, variant)?;
    verify_live_boot_ssh_login(h, console, ssh_host_port)?;

    let required = variant.contract.live_tools();
    let tools: Vec<&str> = required.iter().map(String::as_str).collect();
    let mut missing = Vec::new();
    let mut found = Vec::new();
    let mut broken = Vec::new();

    let remote = h.remote(ssh_host_port)?;
    for tool in &tools {
        match check_tool(variant, tool, |cmd| Ok(remote.exec(cmd)?.exit_code))? {
            ToolCheck::Works => found.push(*tool),
            ToolCheck::Missing => missing.push(*tool),
            ToolCheck::Broken(code) => broken.push((*tool, code)),
        }
    }

    let overlay_evidence = verify_live_overlay_behavior(console)?;

    if !missing.is_empty() || !broken.is_empty() {
        let mut error_msg = String::new();

        if !missing.is_empty() {
            error_msg.push_str(&format!(
                "Missing tools (not in PATH): {}\n",
                missing.join(", ")
            ));
        }

        if !broken.is_empty() {
            error_msg.push_str("Broken tools (exist but failed to execute):\n");
            for (tool, code) in &broken {
                error_msg.push_str(&format!("  {tool} (exit {code})\n"));
            }
        }

        error_msg.push_str(&format!("\nWorking tools: {}", found.join(", ")));

        bail!("{}", error_msg.trim());
    }

    remote.run(
        "checking the install screen launcher",
        &format!("test -x /{LAUNCHER}"),
    )?;
    let ux_split_evidence = verify_install_ux_split_behavior(&remote)?;

    let script_evidence = evidence_over_ssh(
        h,
        ssh_host_port,
        &scenario_evidence(ScenarioId::LiveTools)?,
        &[],
    )?;
    let install_profile_evidence = format!("install UX verified; {ux_split_evidence}");

    Ok(format!(
        "All {} tools verified working (actually executed): {}; {}; {}; {}",
        found.len(),
        found.join(", "),
        install_profile_evidence,
        overlay_evidence,
        script_evidence
    ))
}

fn wait_for_live_tools_serial_readiness(console: &mut Console, variant: &Variant) -> Result<()> {
    // Live-tools is validated over SSH; the serial console may run the
    // install UX rather than a shell.
    console
        .wait_for_live_tools_with_context(live_boot_stall(variant), variant)
        .context("waiting for live-tools serial readiness")
}

fn run_installation(h: &Harness, iso_path: &Path, run: &mut Run) -> Result<String> {
    let variant = &h.variant;
    let injection = h.live_injection(&[])?;
    let mode = INSTALL_MODE;
    run.manifest.install_mode = Some(mode.to_string());
    run.save()?;
    let disk_path = run.dir.join(INSTALL_DISK_FILENAME);
    // An A/B layout holds two 20 GiB root slots and at least 4 GiB of state
    // (recpart's defaults); the image is sparse either way.
    let disk_size = match mode {
        InstallMode::Ab => "48G",
        InstallMode::Mutable => "20G",
    };
    recqemu::create_disk(&disk_path, disk_size)?;

    let ovmf_vars_path = run.dir.join(INSTALL_OVMF_VARS_FILENAME);
    let mut vm = session::spawn(VmSpec {
        iso: Some(iso_path),
        disk: Some(&disk_path),
        vars: Vars::FreshAt(&ovmf_vars_path),
        ssh: Ssh::Free,
        injection: Some(&injection),
        ..VmSpec::new(variant, ScenarioId::Install)
    })?;
    let ssh_host_port = vm.ssh_port()?;

    // The installation runs over SSH; the serial console only marks the
    // boot boundary.
    let remote = h.remote(ssh_host_port)?;
    let install_result = (|| -> Result<usize> {
        vm.console()?.wait_for_live_boot_with_context(live_boot_stall(variant), variant)?;
        remote.wait_ready(live_boot_stall(variant))?;

        let system = install::TestSystem {
            mode,
            root_password: expect::root_password(variant),
        };
        let (target, step_count) = install::install(&remote, variant, &system)?;
        evidence_over_ssh(
            h,
            ssh_host_port,
            &scenario_evidence(ScenarioId::Install)?,
            &[("SCENARIO_INSTALL_TARGET", target.as_str())],
        )?;

        let mut verify = vec![
            ("Root filesystem", format!("test -f {target}/etc/os-release")),
            ("systemd-boot loader config", format!("cat {target}/boot/loader/loader.conf")),
            ("fstab root entry", format!("awk '$2==\"/\" {{ok=1}} END{{exit(ok?0:1)}}' {target}/etc/fstab")),
        ];
        let os_id = &variant.contract.identity.os_id;
        let kernel_dir = match system.mode {
            InstallMode::Mutable => format!("{target}/boot"),
            InstallMode::Ab => format!("{target}/boot/{}-a", os_id),
        };
        verify.push(("Kernel on ESP", format!("ls {kernel_dir}/vmlinuz")));
        if variant.contract.artifacts.initramfs_installed.is_some() {
            verify.push(("Initramfs on ESP", format!("ls {kernel_dir}/initramfs.img")));
        }
        match system.mode {
            InstallMode::Mutable => verify.push((
                "systemd-boot entry",
                format!("cat {target}/boot/loader/entries/{}.conf", os_id),
            )),
            InstallMode::Ab => {
                verify.push((
                    "systemd-boot entry slot A",
                    format!("cat {target}/boot/loader/entries/{}-a.conf", os_id),
                ));
                verify.push((
                    "systemd-boot entry slot B",
                    format!("cat {target}/boot/loader/entries/{}-b.conf", os_id),
                ));
                verify.push((
                    "recab state names slot A known-good",
                    format!("grep -q '\"known_good_slot\": \"A\"' {target}/var/lib/recab/state.json"),
                ));
                verify.push((
                    "recab's trial service enabled",
                    format!("test -L {target}/{}", recab::trial_service_link(variant.contract.profile.init)),
                ));
            }
        }
        verify.push(("Sync filesystem", "sync".to_string()));
        verify.push(("Unmount install target", format!("umount -R {target}")));
        for (what, cmd) in &verify {
            remote.run(what, cmd)?;
        }

        Ok(step_count)
    })();

    // A clean power-off, so the install disk is left as a shut-down system.
    let install_result = install_result.and_then(|steps| {
        vm.shutdown_from_guest("poweroff", SHUTDOWN_TIMEOUT)?;
        Ok(steps)
    });
    drop(vm);

    install_result.map(|step_count| format!("{step_count} install steps completed + verified over SSH"))
}

fn run_installed_boot(h: &Harness, install: &InstallRun, run_dir: &Path) -> Result<String> {
    let variant = &h.variant;
    let mut vm = installed_vm(variant, install, ScenarioId::InstalledBoot, run_dir)?;
    let failed_services = vm
        .console()?
        .wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, variant)?;
    Ok(if failed_services.is_empty() {
        "Installed system boot markers detected".to_string()
    } else {
        format!(
            "Installed system boot markers detected; failed services seen: {}",
            failed_services.join(" | ")
        )
    })
}

fn run_automated_login(h: &Harness, install: &InstallRun, run_dir: &Path) -> Result<String> {
    let variant = &h.variant;
    let mut vm = installed_vm(variant, install, ScenarioId::AutomatedLogin, run_dir)?;
    let console = vm.console()?;

    console.wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, variant)?;

    console.login("root", expect::root_password(variant), LOGIN_TIMEOUT)?;

    let result = console.exec("echo SCENARIO_LOGIN_OK", COMMAND_TIMEOUT)?;
    let script_evidence = evidence_on_console(
        &variant.repo_root,
        console,
        &scenario_evidence(ScenarioId::AutomatedLogin)?,
    )?;

    if result.output.contains("SCENARIO_LOGIN_OK") {
        Ok(format!("Login succeeded, shell functional; {script_evidence}"))
    } else {
        bail!(
            "Login succeeded but shell not functional. Got: {}",
            result.output.trim()
        );
    }
}

/// The runtime scenario: the installed system's tools.
fn run_runtime(h: &Harness, install: &InstallRun, run_dir: &Path) -> Result<String> {
    let variant = &h.variant;
    let mut vm = installed_vm(variant, install, ScenarioId::Runtime, run_dir)?;
    let console = vm.console()?;

    console.wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, variant)?;
    console.login("root", expect::root_password(variant), LOGIN_TIMEOUT)?;

    let required = variant.contract.installed_tools();
    let tools: Vec<&str> = required.iter().map(String::as_str).collect();
    let mut missing = Vec::new();
    let mut found = Vec::new();

    let mut broken = Vec::new();
    for tool in &tools {
        match check_tool(variant, tool, |cmd| Ok(console.exec(cmd, COMMAND_TIMEOUT)?.exit_code))? {
            ToolCheck::Works => found.push(*tool),
            ToolCheck::Missing => missing.push(*tool),
            ToolCheck::Broken(code) => broken.push(format!("{tool} (exit {code})")),
        }
    }


    if !missing.is_empty() || !broken.is_empty() {
        bail!(
            "Missing runtime tools: {}\nBroken: {}\nFound: {}",
            missing.join(", "),
            broken.join(", "),
            found.join(", ")
        );
    }

    let script_evidence = evidence_on_console(&variant.repo_root, console, &scenario_evidence(ScenarioId::Runtime)?)?;
    Ok(format!(
        "All {} runtime tools present; {script_evidence}",
        found.len()
    ))
}

/// The ISO of the latest successful release build of `product`, and its run.
pub(crate) fn resolve_release(repo: &Path, distro_id: &str, product: ProductId) -> Result<ReleaseIso> {
    let release_dir = runs::release_dir(repo, distro_id, product);
    let missing = || Missing::ReleaseIso {
        distro: distro_id.to_string(),
        product,
    };
    let (_, manifest) = runs::latest_success(&release_dir)?.ok_or_else(missing)?;
    let iso = runs::latest_release_iso(repo, distro_id, product)?.ok_or_else(missing)?;
    Ok(ReleaseIso {
        product,
        iso,
        run: manifest.reference(),
    })
}

/// What a passing run depends on: the runs its inputs come from, the
/// variant's contract, and the harness's own sources (its derived
/// expectations, guest-side scripts, and the VM and SSH crates it drives),
/// so a change to any of them reruns it.
fn scenario_input_fingerprint(variant: &Variant, inputs: &Inputs) -> Result<String> {
    let mut parts: Vec<String> = inputs
        .refs()
        .iter()
        .map(|run| format!("{}:{}", run.target_name, run.run_id))
        .collect();
    let sources: Vec<PathBuf> = HARNESS_SOURCES.iter().map(|dir| variant.repo_root.join(dir)).collect();
    parts.push(format!("harness:{:016x}", content_hash(&sources)?));
    parts.push(format!("contract:{:016x}", content_hash(&[variant.manifest.clone()])?));
    Ok(parts.join(";"))
}

/// A hash of the names and contents of the files at or under `paths`.
fn content_hash(paths: &[PathBuf]) -> Result<u64> {
    use std::hash::{Hash, Hasher};
    let mut files = Vec::new();
    let mut pending = paths.to_vec();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(&path).with_context(|| format!("reading '{}'", path.display()))? {
                pending.push(entry?.path());
            }
        } else {
            files.push(path);
        }
    }
    files.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for file in files {
        file.hash(&mut hasher);
        fs::read(&file)
            .with_context(|| format!("reading '{}'", file.display()))?
            .hash(&mut hasher);
    }
    Ok(hasher.finish())
}

pub(crate) fn verify_live_boot_ssh_login(h: &Harness, console: &mut Console, ssh_host_port: u16) -> Result<()> {
    h.remote(ssh_host_port)?
        .wait_ready(Duration::from_secs(60))
        .map_err(|err| {
            let diagnostics = collect_live_boot_ssh_diagnostics(console);
            anyhow::anyhow!("live-boot SSH login probe failed after shell-ready boundary: {err:#}\n\n{diagnostics}")
        })
}

fn collect_live_boot_ssh_diagnostics(console: &mut Console) -> String {
    let checks = [
        ("Kernel cmdline", "cat /proc/cmdline"),
        (
            "Network interfaces",
            "ip -brief addr 2>/dev/null || ip addr 2>/dev/null || true",
        ),
        (
            "Network routes",
            "ip route 2>/dev/null || route -n 2>/dev/null || true",
        ),
        (
            "OpenRC networking status",
            "rc-service networking status 2>/dev/null || true",
        ),
        (
            "OpenRC dhcpcd status",
            "rc-service dhcpcd status 2>/dev/null || true",
        ),
        (
            "Root SSH auth files",
            "ls -ld /root /root/.ssh /root/.ssh/authorized_keys 2>/dev/null || true",
        ),
        (
            "Injected boot payload",
            &format!(
                "ls -l {d} {d}/* 2>/dev/null || true; cat {d}/source {e} 2>/dev/null || true",
                d = recinit::live::BOOT_INJECTION_DIR,
                e = recinit::live::BOOT_INJECTION_ENV
            ),
        ),
        (
            "sshd auth settings",
            "sshd -T 2>/dev/null | grep -E 'permitrootlogin|pubkeyauthentication|passwordauthentication|authorizedkeysfile' || true",
        ),
        (
            "sshd service status",
            "systemctl status sshd.service --no-pager -l || true",
        ),
        (
            "anaconda-sshd service status",
            "systemctl status anaconda-sshd.service --no-pager -l || true",
        ),
        (
            "sshd journal",
            "journalctl -b -u sshd.service --no-pager -n 80 || true",
        ),
        (
            "Port 22 listeners",
            "ss -lntp 2>/dev/null | grep ':22' || netstat -lntp 2>/dev/null | grep ':22' || true",
        ),
    ];

    let mut report = String::from("live-boot SSH diagnostics from live shell:\n");
    for (title, cmd) in checks {
        report.push_str(&format!("\n--- {} ---\n$ {}\n", title, cmd));
        match console.exec(cmd, Duration::from_secs(15)) {
            Ok(result) => {
                let output = result.output.trim();
                if output.is_empty() {
                    report.push_str("(no output)\n");
                } else {
                    report.push_str(output);
                    report.push('\n');
                }
            }
            Err(err) => {
                report.push_str(&format!("(failed to collect: {:#})\n", err));
            }
        }
    }
    report
}

/// Transfer the scenario evidence into /run, then execute it. Test scripts
/// are harness inputs, never a dependency of an installed release. Runs through `exec`,
/// which runs a guest command and returns its exit status and output; the
/// script must exit 0 and print its pass marker.
fn run_evidence(
    repo: &Path,
    evidence: &Evidence,
    env: &[(&str, &str)],
    mut exec: impl FnMut(&str) -> Result<(i32, String)>,
) -> Result<String> {
    let name = Path::new(&evidence.script);
    if name.components().count() != 1 || !evidence.script.ends_with(".sh") {
        bail!("invalid evidence script name: {}", evidence.script);
    }
    let scripts = repo.join(TEST_SCRIPTS_DIR);
    let path = format!("/run/scenario-tests/{}", evidence.script);
    let (code, output) = exec("mkdir -p /run/scenario-tests/lib")?;
    if code != 0 {
        bail!("creating guest evidence directory failed: {output}");
    }
    for (source, dest) in [
        (
            scripts.join("lib/common.sh"),
            "/run/scenario-tests/lib/common.sh".to_string(),
        ),
        (scripts.join(name), path.clone()),
    ] {
        let content = fs::read_to_string(&source)
            .with_context(|| format!("reading evidence {}", source.display()))?;
        let (code, output) = exec(&format!("true > {}", install::shell_quote(&dest)))?;
        if code != 0 {
            bail!("preparing evidence {dest} failed: {output}");
        }
        // Keep each serial input line below the terminal's canonical line bound.
        let mut chunk = String::new();
        for c in content.chars() {
            chunk.push(c);
            if chunk.len() >= 200 {
                transfer_evidence_chunk(&mut exec, &dest, &chunk)?;
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            transfer_evidence_chunk(&mut exec, &dest, &chunk)?;
        }
    }
    let mut cmd = format!(
        "SCENARIO_PASS_MARKER={}",
        install::shell_quote(&evidence.pass_marker)
    );
    for (key, value) in env {
        cmd.push_str(&format!(" {key}={}", install::shell_quote(value)));
    }
    let (code, output) = exec(&format!("{cmd} sh {}", install::shell_quote(&path)))?;
    if code != 0 || !output.contains(&evidence.pass_marker) {
        bail!(
            "evidence script {path} failed (exit {code}, pass marker {}):\n{}",
            if output.contains(&evidence.pass_marker) {
                "printed"
            } else {
                "missing"
            },
            output.trim()
        );
    }
    Ok(format!("{} passed", evidence.script))
}

/// `run_evidence` over SSH.
fn evidence_over_ssh(
    h: &Harness,
    ssh_host_port: u16,
    evidence: &Evidence,
    env: &[(&str, &str)],
) -> Result<String> {
    let remote = h.remote(ssh_host_port)?;
    run_evidence(&h.variant.repo_root, evidence, env, |cmd| {
        let out = remote.exec(cmd)?;
        Ok((out.exit_code, out.output))
    })
}

/// `run_evidence` on the serial console.
fn evidence_on_console(repo: &Path, console: &mut Console, evidence: &Evidence) -> Result<String> {
    run_evidence(repo, evidence, &[], |cmd| {
        let out = console.exec(cmd, EVIDENCE_TIMEOUT)?;
        Ok((out.exit_code, out.output))
    })
}

fn verify_install_ux_split_behavior(remote: &Remote) -> Result<String> {
    let probe = remote.run(
        "probing the install helper",
        &format!("/{LAUNCHER} --probe"),
    )?;
    let helper = probe
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(PROBE_PREFIX))
        .unwrap_or("")
        .trim();
    if helper != format!("/{SPLIT}") {
        bail!(
            "install UX helper mismatch: expected '/{SPLIT}', found '{}'.\nProbe output: {}",
            if helper.is_empty() { "<empty>" } else { helper },
            probe.trim()
        );
    }
    let smoke = remote.run(
        "running the split-pane smoke launch",
        &format!("{SMOKE_ENV}=1 /{LAUNCHER}"),
    )?;
    if !smoke.contains(SPLIT_SMOKE_OK) {
        bail!(
            "split-pane smoke output lacks '{SPLIT_SMOKE_OK}':\n{}",
            smoke.trim()
        );
    }
    Ok("install UX split-pane smoke verified (shell-left + docs-right)".to_string())
}

/// The latest successful install run of `distro_id`.
pub fn resolve_latest_install_run(repo: &Path, distro_id: &str) -> Result<InstallRun> {
    let install_dir = runs::scenario_dir(repo, distro_id, ScenarioId::Install);
    let missing = || Missing::InstallRun {
        distro: distro_id.to_string(),
    };
    let (run_dir, manifest) = runs::latest_success(&install_dir)?.ok_or_else(missing)?;
    let disk_path = run_dir.join(INSTALL_DISK_FILENAME);
    let ovmf_vars_path = run_dir.join(INSTALL_OVMF_VARS_FILENAME);
    if !disk_path.is_file() || !ovmf_vars_path.is_file() {
        return Err(anyhow::Error::new(missing()).context(format!(
            "the install run '{}' has lost its disk or UEFI variables",
            manifest.run_id
        )));
    }
    Ok(InstallRun {
        run: manifest.reference(),
        disk_path,
        ovmf_vars_path,
    })
}

fn transfer_evidence_chunk(
    exec: &mut impl FnMut(&str) -> Result<(i32, String)>,
    dest: &str,
    content: &str,
) -> Result<()> {
    let (code, output) = exec(&format!(
        "printf '%s' {} >> {}",
        install::shell_quote(content),
        install::shell_quote(dest)
    ))?;
    if code != 0 {
        bail!("transferring evidence {dest} failed: {output}");
    }
    Ok(())
}

fn verify_live_overlay_behavior(console: &mut Console) -> Result<String> {
    let marker = console.exec("test -f /live-boot-marker", COMMAND_TIMEOUT)?;
    if !marker.success() {
        bail!("Live overlay marker missing: /live-boot-marker");
    }

    let overlay_mount = console.exec(
        "mount | grep ' type overlay ' | grep 'lowerdir=/live-overlay:/rootfs'",
        COMMAND_TIMEOUT,
    )?;
    if !overlay_mount.success() {
        bail!("Overlay root mount is missing required lowerdir=/live-overlay:/rootfs chain");
    }

    Ok("overlayfs lowerdir chain verified".to_string())
}

/// How long the installed system may stay silent while booting.
const INSTALLED_BOOT_STALL: Duration = Duration::from_secs(90);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const EVIDENCE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a guest may take to power off or reboot.
pub(super) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(180);

/// Longest silence tolerated during live boot before it counts as a stall:
/// OpenRC's early boot can stay quiet for longer than systemd's.
pub(crate) fn live_boot_stall(variant: &Variant) -> Duration {
    Duration::from_secs(match variant.contract.profile.init {
        InitSystem::Systemd => 60,
        InitSystem::Openrc => 180,
    })
}

/// The evidence of a scenario that runs a guest-side script.
fn scenario_evidence(scenario: ScenarioId) -> Result<Evidence> {
    Evidence::of(scenario).with_context(|| format!("{scenario} runs no guest-side script"))
}

/// Boot the install run's disk through an overlay and a copy of its UEFI
/// variables in `run_dir`, so the install run stays as the install left it.
fn installed_vm(
    variant: &Variant,
    install: &InstallRun,
    scenario: ScenarioId,
    run_dir: &Path,
) -> Result<session::Vm> {
    let (disk, vars) = overlay_install(install, run_dir)?;
    session::spawn(VmSpec {
        disk: Some(&disk),
        vars: Vars::Existing(&vars),
        ..VmSpec::new(variant, scenario)
    })
}

/// A copy-on-write disk over the install run's disk and a copy of its UEFI
/// variables, in `run_dir`.
pub(crate) fn overlay_install(install: &InstallRun, run_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let disk = run_dir.join(INSTALL_DISK_FILENAME);
    let vars = run_dir.join(INSTALL_OVMF_VARS_FILENAME);
    recqemu::create_overlay(&install.disk_path, &disk)?;
    fs::copy(&install.ovmf_vars_path, &vars).with_context(|| {
        format!("copying the install run's UEFI variables '{}'", install.ovmf_vars_path.display())
    })?;
    Ok((disk, vars))
}

enum ToolCheck {
    Works,
    Missing,
    Broken(i32),
}

/// Check that `tool` is on PATH (`command -v`) and runs: its known harmless
/// invocation must exit 0. `exec` runs a command in the guest and returns
/// its exit status.
fn check_tool(
    variant: &Variant,
    tool: &str,
    mut exec: impl FnMut(&str) -> Result<i32>,
) -> Result<ToolCheck> {
    if exec(&format!("command -v {tool} >/dev/null"))? != 0 {
        return Ok(ToolCheck::Missing);
    }
    let privilege = variant.contract.profile.privilege;
    let run = if tool == privilege.as_str() {
        Some(privilege_check(privilege))
    } else {
        tool_run_command(tool)
    };
    let Some(run) = run else {
        return Ok(ToolCheck::Works);
    };
    Ok(match exec(&format!("{{ {run}; }} >/dev/null 2>&1"))? {
        0 => ToolCheck::Works,
        code => ToolCheck::Broken(code),
    })
}

/// A harmless invocation of `tool` that exits 0 when it works (missing
/// libraries, broken binaries fail), or `None` when the tool has no such
/// invocation and presence on PATH is the check (busybox `vi`).
fn tool_run_command(tool: &str) -> Option<String> {
    Some(match tool {
        "recpart" | "recstrap" | "recfstab" | "recchroot" => format!("{tool} --help"),
        "mkfs.ext4" => "mkfs.ext4 -V".to_string(),
        "nvme" => "nvme version".to_string(),
        "hdparm" => "hdparm -V".to_string(),
        "ip" => "ip link show".to_string(),
        "ping" => "ping -V || ping -h || ping --help".to_string(),
        "less" => "less --help || less -V".to_string(),
        "grep" => "echo x | grep x".to_string(),
        "find" => "find / -maxdepth 0".to_string(),
        "ssh" => "ssh -V".to_string(),
        "sh" | "ash" => format!("{tool} -c true"),
        "cat" => "cat /dev/null".to_string(),
        "ls" => "ls /".to_string(),
        "ps" => "ps".to_string(),
        "dmesg" => "dmesg".to_string(),
        "vi" => return None,
        _ => format!("{tool} --version"),
    })
}

/// A harmless invocation of the variant's privilege tool.
fn privilege_check(tool: PrivilegeTool) -> String {
    match tool {
        PrivilegeTool::Sudo => "sudo --version".to_string(),
        // OpenDoas has no --version option: check its configuration instead.
        PrivilegeTool::Doas => format!("doas -C {}", tool.config_file()),
    }
}

/// Where to look when a scenario fails.
fn failure_hints(scenario: ScenarioId) -> &'static [&'static str] {
    match scenario {
        ScenarioId::BuildPreflight => &[
            "the base-rootfs ISO or the kernel predates the variant's contract: rebuild them",
        ],
        ScenarioId::LiveBoot => &[
            "the ISO is not built or is corrupted",
            "the kernel panics, or the live initramfs cannot find the live medium",
            "the UEFI firmware finds no boot entry",
        ],
        ScenarioId::LiveTools => &[
            "a tool is missing from the variant's packages or the builder's live tools",
            "a tool is on PATH but fails to run (a missing library)",
        ],
        ScenarioId::Install => &[
            "recpart, recstrap, recfstab, recchroot or recsetup failed",
            "the disk is too small or partitioning failed",
            "the boot loader or the slot entries were not written",
        ],
        ScenarioId::InstalledBoot => &[
            "the boot loader entry, kernel or installed initramfs on the ESP is wrong",
            "kernel modules for the disk are missing",
        ],
        ScenarioId::AutomatedLogin => &[
            "no getty on the serial console",
            "root's password was not set by the install",
        ],
        ScenarioId::Runtime => &[
            "a tool is missing from the variant's packages",
            "PATH is misconfigured",
        ],
        ScenarioId::AbUpdate => &[
            "recab or recstrap is missing from the installed rootfs",
            "recab stage did not write slot B's entry or kernel",
            "one-shot boot variables were not kept in the UEFI variable store",
        ],
    }
}

fn print_failure(scenario: ScenarioId, err: &anyhow::Error) {
    eprintln!();
    eprintln!(
        "{} {} FAILED: {}",
        "[FAIL]".red().bold(),
        scenario.title(),
        scenario.key()
    );
    eprintln!();
    eprintln!("  Error: {:#}", err);
    eprintln!();
    eprintln!("  Common causes:");
    for hint in failure_hints(scenario) {
        eprintln!("    - {hint}");
    }
    eprintln!();
}
