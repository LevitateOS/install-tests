//! The scenarios: each consumes existing artifacts and checks one capability
//! the same way on every variant. Their names, inputs and order are
//! `distro_contract::ScenarioId`; what each expects of a variant is
//! `crate::expect`; what runs in the guest is `crate::guest` and
//! `test-scripts/`. Every run leaves a `distro_contract::runs` record in its
//! own run directory; the newest finished record of a scenario is its status.

mod ab_update;
mod install;
use crate::boot_injection::{BootInjection, SSH_AUTHORIZED_KEY};
use crate::budget::Budget;
use crate::expect::{self, Evidence};
use crate::guest;
use crate::preflight::require_preflight;
use crate::qemu::session::{self, Ssh, Vars, VmSpec};
use crate::qemu::{wait_installed_boot, wait_live_boot};
use anyhow::{Context, Result, bail};
use colored::Colorize;
use distro_builder::install_ux::{LAUNCHER, PROBE_PREFIX, SMOKE_ENV, SPLIT, SPLIT_SMOKE_OK};
use distro_contract::runs::{self, Run, RunManifest, RunRef, RunStatus};
use distro_contract::{Missing, ProductId, Variant};
use recshuttle::{Login, Remote};
use std::fs;
use std::path::{Path, PathBuf};

pub use distro_contract::ScenarioId;
pub use recpart::InstallMode;

/// The install run's disk and UEFI variable store, in its run directory.
pub(super) const INSTALL_DISK_FILENAME: &str = "disk.qcow2";
pub(super) const INSTALL_OVMF_VARS_FILENAME: &str = "ovmf-vars.fd";
/// The harness's own sources: a change to any of them reruns cached results.
const HARNESS_SOURCES: &[&str] = &[
    "testing/install-tests/src",
    "testing/install-tests/test-scripts",
    "tools/recqemu/src",
    "tools/recshuttle/src",
];

/// The latest successful install run of one mode: its disk and UEFI
/// variables, which the installed-system scenarios boot through overlays and
/// never change.
#[derive(Debug, Clone)]
pub struct InstallRun {
    pub run: RunRef,
    pub mode: InstallMode,
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
    update_fixture: Option<PathBuf>,
}

impl Inputs {
    fn resolve(
        repo: &Path,
        distro_id: &str,
        scenario: ScenarioId,
        mode: InstallMode,
        fixture: Option<&Path>,
    ) -> Result<Self> {
        let input = scenario.input();
        let fixture = if matches!(scenario, ScenarioId::Install | ScenarioId::AbUpdate) {
            fixture
                .map(|p| {
                    fs::canonicalize(p).with_context(|| format!("update fixture '{}'", p.display()))
                })
                .transpose()?
        } else {
            None
        };
        if scenario == ScenarioId::AbUpdate && fixture.is_none() {
            bail!(
                "ab-update requires --update-fixture <existing RECAB_P13 ISO>; a matching install run must already exist"
            );
        }
        if let Some(path) = &fixture {
            anyhow::ensure!(path.is_file(), "update fixture must be a regular ISO file");
        }
        Ok(Self {
            update_fixture: fixture,
            iso: input
                .iso
                .map(|product| resolve_release(repo, distro_id, product))
                .transpose()?,
            install: input
                .install_run
                .then(|| resolve_latest_install_run(repo, distro_id, mode))
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
fn latest_result(
    variant: &Variant,
    scenario: ScenarioId,
) -> Result<Option<Result<RunManifest, String>>> {
    let dir = runs::scenario_dir(&variant.repo_root, &variant.id, scenario);
    Ok(runs::list(&dir)?
        .into_iter()
        .map(|run| run.manifest)
        .find(|manifest| !matches!(manifest, Ok(m) if m.status == RunStatus::Building)))
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
    /// The layout `install` installs, and the install run the installed-system
    /// scenarios boot (ab-update needs A/B).
    pub install_mode: InstallMode,
    /// Explicit existing signed-feed fixture; never built by a scenario.
    pub update_fixture: Option<PathBuf>,
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
/// guest login, the extra injection and the install mode.
pub(crate) struct Harness {
    pub variant: Variant,
    pub login: Option<Login>,
    pub injection: Vec<String>,
    pub install_mode: InstallMode,
    /// Explicit existing signed-feed fixture; never built by a scenario.
    pub update_fixture: Option<PathBuf>,
}

impl Harness {
    pub(crate) fn new(
        repo: &Path,
        distro_id: &str,
        options: &RunOptions,
        boots: bool,
    ) -> Result<Self> {
        let variant = load_variant(repo, distro_id)?;
        let login = if boots {
            let key = match &options.login_key {
                Some(key) => key.clone(),
                None => ensure_login_key(repo)?,
            };
            Some(Login {
                key,
                user: "root".to_string(),
            })
        } else {
            None
        };
        Ok(Self {
            variant,
            login,
            injection: options.injection.clone(),
            install_mode: options.install_mode,
            update_fixture: options.update_fixture.clone(),
        })
    }

    fn login(&self) -> Result<&Login> {
        self.login
            .as_ref()
            .context("this run boots no VM, so it resolved no login key")
    }

    /// The guest behind a forwarded SSH port.
    pub(crate) fn remote(&self, port: u16) -> Result<Remote> {
        Ok(Remote::new(port, self.login()?.clone()))
    }

    /// The injection a live boot gets: the login key, `extra`, then the run's own lines.
    pub(crate) fn live_injection(&self, extra: &[String]) -> Result<BootInjection> {
        let mut lines = vec![format!(
            "{SSH_AUTHORIZED_KEY}={}",
            recshuttle::public_key(&self.login()?.key)?
        )];
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
            Outcome::NotRun("needs a passing install, and install failed in this run".to_string())
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
        if scenario == ScenarioId::Install
            && !matches!(outcome, Outcome::Passed(_) | Outcome::Cached)
        {
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
    if scenario == ScenarioId::AbUpdate && h.install_mode != InstallMode::Ab {
        bail!(
            "ab-update updates an A/B install; it has no {} mode",
            h.install_mode
        );
    }
    let inputs = Inputs::resolve(
        repo,
        distro_id,
        scenario,
        h.install_mode,
        h.update_fixture.as_deref(),
    )?;
    let fingerprint = scenario_input_fingerprint(variant, scenario, &inputs, h.install_mode)?;

    if force {
        eprintln!("Forcing {} rerun.", scenario.title());
    } else if let Some(Ok(last)) = latest_result(variant, scenario)? {
        if last.status == RunStatus::Success
            && last.fingerprint.as_deref() == Some(fingerprint.as_str())
        {
            eprintln!(
                "{} {} already passed (rerun with --force).",
                "[SKIP]".green(),
                scenario.title()
            );
            return Ok(None);
        }
    }

    eprintln!("{} {}", ">>".cyan(), scenario.title());
    let mut run = Run::start_scenario(repo, distro_id, scenario)?;
    run.manifest.inputs = inputs.refs();
    run.manifest.update_fixture = inputs.update_fixture.clone();
    run.manifest.fingerprint = Some(fingerprint);
    run.save()?;

    let result = (|| -> Result<String> {
        if let Some(release) = &inputs.iso {
            let preflight = require_preflight(&release.iso, release.product, variant)?;
            if scenario == ScenarioId::BuildPreflight {
                return Ok(preflight);
            }
        }
        match scenario {
            ScenarioId::BuildPreflight => bail!("the registry gives build-preflight no ISO"),
            ScenarioId::LiveBoot => run_live_boot(h, inputs.iso(scenario)?),
            ScenarioId::LiveTools => run_live_tools(h, inputs.iso(scenario)?, &run.dir),
            ScenarioId::Install => run_installation(h, inputs.iso(scenario)?, &mut run),
            ScenarioId::InstalledBoot => run_installed_boot(h, inputs.install(scenario)?, &run.dir),
            ScenarioId::AutomatedLogin => {
                run_automated_login(h, inputs.install(scenario)?, &run.dir)
            }
            ScenarioId::Runtime => run_runtime(h, inputs.install(scenario)?, &run.dir),
            ScenarioId::AbUpdate => ab_update::run(
                h,
                inputs.install(scenario)?,
                inputs
                    .update_fixture
                    .as_deref()
                    .context("update fixture missing")?,
                &run.dir,
            ),
            ScenarioId::DesktopBoot => run_desktop_boot(h, inputs.iso(scenario)?),
        }
    })();

    let result = result.and_then(|evidence| {
        anyhow::ensure!(
            scenario_input_fingerprint(variant, scenario, &inputs, h.install_mode)?
                == run
                    .manifest
                    .fingerprint
                    .as_deref()
                    .context("input fingerprint missing")?,
            "scenario inputs changed while it was running"
        );
        Ok(evidence)
    });
    run.manifest.evidence = Some(match &result {
        Ok(evidence) => evidence.clone(),
        Err(err) => format!("FAIL: {err:#}"),
    });
    let recorded = run
        .finish(result.is_ok())
        .with_context(|| format!("recording the {scenario} run for {distro_id}"))
        .and_then(|_| {
            let dir = runs::scenario_dir(repo, distro_id, scenario);
            Ok(runs::prune(
                repo,
                distro_id,
                &dir,
                runs::SCENARIO_RETENTION,
            )?)
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
pub fn scenario_statuses(
    repo: &Path,
    distro_id: &str,
) -> Result<Vec<(ScenarioId, ScenarioStatus)>> {
    statuses(&load_variant(repo, distro_id)?)
}

/// The layout the latest install run installed, which the installed-system
/// scenarios were last run against (the default before any install run).
fn last_install_mode(variant: &Variant) -> Result<InstallMode> {
    let mode = match latest_result(variant, ScenarioId::Install)? {
        Some(Ok(last)) => last.install_mode,
        _ => None,
    };
    Ok(mode
        .and_then(|name| serde_json::from_value(serde_json::Value::String(name)).ok())
        .unwrap_or_default())
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
            let mode = last_install_mode(variant)?;
            let fingerprint = Inputs::resolve(
                &variant.repo_root,
                &variant.id,
                scenario,
                mode,
                last.update_fixture.as_deref(),
            )
            .and_then(|inputs| scenario_input_fingerprint(variant, scenario, &inputs, mode));
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

/// A booted live ISO with its SSH login ready.
struct Live {
    vm: session::Vm,
    remote: Remote,
}

/// Boot `iso` for `scenario`, wait for the live shell and an SSH login; a
/// login that does not come is diagnosed from the serial shell.
fn boot_live(
    h: &Harness,
    scenario: ScenarioId,
    iso: &Path,
    disk: Option<(&Path, &Path)>,
) -> Result<Live> {
    let variant = &h.variant;
    let injection = h.live_injection(&[])?;
    let mut vm = session::spawn(VmSpec {
        iso: Some(iso),
        disk: disk.map(|(disk, _)| disk),
        vars: match disk {
            Some((_, vars)) => Vars::FreshAt(vars),
            None => Vars::Temporary,
        },
        ssh: Ssh::Free,
        injection: Some(&injection),
        media: (scenario == ScenarioId::Install)
            .then_some(h.update_fixture.as_deref())
            .flatten(),
        ..VmSpec::new(variant, scenario)
    })?;
    let mut remote = h.remote(vm.ssh_port()?)?;
    wait_live_boot(vm.console()?, variant)?;
    verify_live_boot_ssh_login(h, &mut vm, &mut remote)?;
    Ok(Live { vm, remote })
}

/// Wait for SSH to take a login; when it does not, the error carries the
/// serial shell's diagnostics.
pub(crate) fn verify_live_boot_ssh_login(
    h: &Harness,
    vm: &mut session::Vm,
    remote: &mut Remote,
) -> Result<()> {
    remote.wait_ready(Budget::SshReady.get()).map_err(|err| {
        let diagnostics = match vm.console() {
            Ok(console) => guest::ssh_diagnostics(console, &h.variant),
            Err(err) => format!("(no serial shell to diagnose from: {err:#})"),
        };
        anyhow::anyhow!(
            "the live system booted but SSH did not take a login: {err:#}\n\n{diagnostics}"
        )
    })
}

/// Live boot: the ISO boots, takes an SSH login, and its services run.
fn run_live_boot(h: &Harness, iso: &Path) -> Result<String> {
    let mut live = boot_live(h, ScenarioId::LiveBoot, iso, None)?;
    let evidence = guest::run_evidence(
        &mut live.remote,
        &h.variant,
        &scenario_evidence(ScenarioId::LiveBoot)?,
        &[],
    )?;
    Ok(format!("live shell up, SSH login works; {evidence}"))
}

/// Live tools: every tool the contract lists runs, the install screen's
/// launcher works, and the live environment is set up (all over SSH).
fn run_live_tools(h: &Harness, iso: &Path, run_dir: &Path) -> Result<String> {
    let variant = &h.variant;
    let mut live = boot_live(h, ScenarioId::LiveTools, iso, None)?;
    let remote = &mut live.remote;
    let tools = guest::check_tools(remote, variant, &variant.contract.live_tools())?;
    remote.run(
        "checking the install screen launcher",
        &format!("test -x /{LAUNCHER}"),
        Budget::Command.get(),
    )?;
    let ux = verify_install_ux_split_behavior(remote)?;
    guest::install_pty(
        remote,
        variant,
        &format!("/{LAUNCHER}"),
        &run_dir.join("install-pty.log"),
    )?;
    let evidence = guest::run_evidence(
        remote,
        variant,
        &scenario_evidence(ScenarioId::LiveTools)?,
        &[],
    )?;
    Ok(format!(
        "{tools}; {ux}; actual installer PTY interaction passed (install-pty.log); {evidence}"
    ))
}

/// Desktop boot: the live-desktop ISO boots into the desktop. Its display
/// manager runs and has logged the live user into a graphical session, no
/// service failed, the image says it is the desktop system, and each
/// desktop app (`DESKTOP_APPS`) is installed, has its menu entry and starts
/// (`desktop-boot.sh`, over SSH).
fn run_desktop_boot(h: &Harness, iso: &Path) -> Result<String> {
    let variant = &h.variant;
    let mut live = boot_live(h, ScenarioId::DesktopBoot, iso, None)?;
    // Each desktop app as program:menu-entry, for the guest script.
    let desktop_apps = distro_contract::DESKTOP_APPS
        .iter()
        .zip(distro_contract::DESKTOP_APP_ENTRIES)
        .map(|(app, entry)| format!("{app}:{entry}"))
        .collect::<Vec<_>>()
        .join(" ");
    let evidence = guest::run_evidence(
        &mut live.remote,
        variant,
        &scenario_evidence(ScenarioId::DesktopBoot)?,
        &[
            (
                "SCENARIO_DISPLAY_MANAGER",
                variant.contract.desktop.display_manager.as_str(),
            ),
            (
                "SCENARIO_LIVE_USER",
                distro_contract::install_ux::LIVE_DESKTOP_USER,
            ),
            (
                "SCENARIO_FLAVOUR_FIELD",
                distro_contract::profile::REC_FLAVOUR,
            ),
            (
                "SCENARIO_FLAVOUR",
                ProductId::LiveDesktop.flavour().as_str(),
            ),
            ("SCENARIO_DESKTOP_APPS", desktop_apps.as_str()),
        ],
    )?;
    Ok(format!("desktop session up with its apps; {evidence}"))
}

/// Install: the live-tools ISO installs to a disk in `h.install_mode` with
/// the tools it ships, and `install.sh` finds a complete install; the guest
/// then powers off cleanly.
fn run_installation(h: &Harness, iso: &Path, run: &mut Run) -> Result<String> {
    let variant = &h.variant;
    let mode = h.install_mode;
    run.manifest.install_mode = Some(mode.to_string());
    run.save()?;
    let disk_path = run.dir.join(INSTALL_DISK_FILENAME);
    let vars_path = run.dir.join(INSTALL_OVMF_VARS_FILENAME);
    // An A/B layout holds two 20 GiB root slots and at least 4 GiB of state
    // (recpart's defaults); the image is sparse either way.
    let disk_size = match mode {
        InstallMode::Ab => "48G",
        InstallMode::Mutable => "20G",
    };
    recqemu::create_disk(&disk_path, disk_size)?;

    let mut live = boot_live(h, ScenarioId::Install, iso, Some((&disk_path, &vars_path)))?;
    let system = install::TestSystem {
        mode,
        root_password: expect::root_password(variant),
        update_fixture: h.update_fixture.is_some(),
    };
    let (target, steps) = install::install(&mut live.remote, variant, &system)?;
    let trial_link = recab::trial_service_link(variant.contract.profile.init);
    // The directory recab puts the installed kernel in: slot A's, or the one root's.
    let kernel_dir = recab::layout_name(
        &variant.contract.identity.os_id,
        (mode == InstallMode::Ab).then_some(recab::Slot::A),
    );
    let state_file = recab::state_file();
    let mode_name = mode.to_string();
    let evidence = guest::run_evidence(
        &mut live.remote,
        variant,
        &scenario_evidence(ScenarioId::Install)?,
        &[
            ("SCENARIO_INSTALL_TARGET", target.as_str()),
            ("SCENARIO_INSTALL_MODE", mode_name.as_str()),
            ("SCENARIO_KERNEL_DIR", kernel_dir.as_str()),
            ("SCENARIO_STATE_FILE", state_file.as_str()),
            ("SCENARIO_TRIAL_LINK", &trial_link),
        ],
    )?;
    let target_q = recqemu::shell_quote(&target);
    // Flushing a freshly extracted install is a disk-writing stage, not a
    // short probe. Keep it separate from unmount so a timeout identifies the
    // operation that stalled; neither failure permits a passing install.
    eprintln!("    -> Flush installed filesystem writes");
    live.remote.run(
        "flushing installed filesystem writes",
        "sync",
        Budget::Stage.get(),
    )?;
    eprintln!("    -> Unmount the install target");
    live.remote.run(
        "unmounting the install target",
        &format!("umount -R {target_q}"),
        Budget::Command.get(),
    )?;
    // A clean power-off, so the install disk is left as a shut-down system.
    live.vm.shutdown_from_guest("poweroff")?;
    Ok(format!(
        "{mode} install: {steps} install steps over SSH; {evidence}"
    ))
}

/// Boot the install run's overlay and log in as root on the serial getty.
fn installed_login(
    h: &Harness,
    install: &InstallRun,
    scenario: ScenarioId,
    run_dir: &Path,
) -> Result<session::Vm> {
    let variant = &h.variant;
    let mut vm = installed_vm(variant, install, scenario, run_dir)?;
    let console = vm.console()?;
    wait_installed_boot(console, variant)?;
    console.login("root", expect::root_password(variant), Budget::Login.get())?;
    Ok(vm)
}

/// Installed boot: the installed system reaches its login prompt, starts up
/// with no failed service, and is not a live system (`installed-boot.sh`).
fn run_installed_boot(h: &Harness, install: &InstallRun, run_dir: &Path) -> Result<String> {
    let mut vm = installed_login(h, install, ScenarioId::InstalledBoot, run_dir)?;
    let evidence = guest::run_evidence(
        vm.console()?,
        &h.variant,
        &scenario_evidence(ScenarioId::InstalledBoot)?,
        &[],
    )?;
    Ok(format!(
        "{} install booted to its login prompt; {evidence}",
        install.mode
    ))
}

/// Automated login: root logs in on the serial getty and the shell runs commands.
fn run_automated_login(h: &Harness, install: &InstallRun, run_dir: &Path) -> Result<String> {
    let mut vm = installed_login(h, install, ScenarioId::AutomatedLogin, run_dir)?;
    let evidence = guest::run_evidence(
        vm.console()?,
        &h.variant,
        &scenario_evidence(ScenarioId::AutomatedLogin)?,
        &[],
    )?;
    Ok(format!("root logged in; {evidence}"))
}

/// Runtime: every tool the contract lists for the installed system runs.
fn run_runtime(h: &Harness, install: &InstallRun, run_dir: &Path) -> Result<String> {
    let variant = &h.variant;
    let mut vm = installed_login(h, install, ScenarioId::Runtime, run_dir)?;
    let console = vm.console()?;
    let tools = guest::check_tools(console, variant, &variant.contract.installed_tools())?;
    let evidence = guest::run_evidence(
        console,
        variant,
        &scenario_evidence(ScenarioId::Runtime)?,
        &[],
    )?;
    Ok(format!("{tools}; {evidence}"))
}

/// The ISO of the latest successful release build of `product`, and its run.
pub(crate) fn resolve_release(
    repo: &Path,
    distro_id: &str,
    product: ProductId,
) -> Result<ReleaseIso> {
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

/// The latest successful install run of `distro_id` that installed `mode`.
pub fn resolve_latest_install_run(
    repo: &Path,
    distro_id: &str,
    mode: InstallMode,
) -> Result<InstallRun> {
    let install_dir = runs::scenario_dir(repo, distro_id, ScenarioId::Install);
    let missing = || Missing::InstallRun {
        distro: distro_id.to_string(),
    };
    let mode_name = mode.to_string();
    let (run_dir, manifest) = runs::list(&install_dir)?
        .into_iter()
        .find_map(|run| match run.manifest {
            Ok(m)
                if m.status == RunStatus::Success
                    && m.install_mode.as_deref() == Some(mode_name.as_str()) =>
            {
                Some((run.dir, m))
            }
            _ => None,
        })
        .ok_or_else(missing)?;
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
        mode,
        disk_path,
        ovmf_vars_path,
    })
}

/// What a passing run depends on: the runs its inputs come from, the
/// variant's contract, and the harness's own sources (its derived
/// expectations, guest-side scripts, and the VM and SSH crates it drives),
/// so a change to any of them reruns it.
fn scenario_input_fingerprint(
    variant: &Variant,
    scenario: ScenarioId,
    inputs: &Inputs,
    mode: InstallMode,
) -> Result<String> {
    let mut parts: Vec<String> = inputs
        .refs()
        .iter()
        .map(|run| format!("{}:{}", run.target_name, run.run_id))
        .collect();
    let sources: Vec<PathBuf> = HARNESS_SOURCES
        .iter()
        .map(|dir| variant.repo_root.join(dir))
        .collect();
    parts.push(format!("harness:{:016x}", content_hash(&sources)?));
    parts.push(format!(
        "contract:{:016x}",
        content_hash(std::slice::from_ref(&variant.manifest))?
    ));
    if scenario == ScenarioId::Install {
        parts.push(format!("mode:{mode}"));
    }
    if let Some(fixture) = &inputs.update_fixture {
        use sha2::Digest;
        use std::io::Read;
        let mut file = fs::File::open(fixture)?;
        let mut hash = sha2::Sha256::new();
        let mut bytes = [0; 128 * 1024];
        loop {
            let n = file.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            hash.update(&bytes[..n]);
        }
        parts.push(format!(
            "update-fixture:{}:{:x}",
            fixture.display(),
            hash.finalize()
        ));
    }
    Ok(parts.join(";"))
}

/// A hash of the names and contents of the files at or under `paths`.
fn content_hash(paths: &[PathBuf]) -> Result<u64> {
    use std::hash::{Hash, Hasher};
    let mut files = Vec::new();
    let mut pending = paths.to_vec();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for entry in
                fs::read_dir(&path).with_context(|| format!("reading '{}'", path.display()))?
            {
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

fn verify_install_ux_split_behavior(remote: &mut Remote) -> Result<String> {
    let probe = remote.run(
        "probing the install helper",
        &format!("/{LAUNCHER} --probe"),
        Budget::Command.get(),
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
        Budget::Command.get(),
    )?;
    if !smoke.contains(SPLIT_SMOKE_OK) {
        bail!(
            "split-pane smoke output lacks '{SPLIT_SMOKE_OK}':\n{}",
            smoke.trim()
        );
    }
    Ok("install UX split-pane smoke verified (shell-left + docs-right)".to_string())
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
        format!(
            "copying the install run's UEFI variables '{}'",
            install.ovmf_vars_path.display()
        )
    })?;
    Ok((disk, vars))
}

/// Where to look when a scenario fails.
fn failure_hints(scenario: ScenarioId) -> &'static [&'static str] {
    match scenario {
        ScenarioId::BuildPreflight => {
            &["the base-rootfs ISO or the kernel predates the variant's contract: rebuild them"]
        }
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
            "the signed fixture, source policy or installed generation does not match",
            "a stale or incomplete candidate was offered for boot, or prepared changes were lost",
            "write counters or immutable-root/device observations did not match the lifecycle",
            "one-shot boot variables were not kept in the UEFI variable store",
        ],
        ScenarioId::DesktopBoot => &[
            "the desktop system lacks a package its session needs (the desktop lock)",
            "the display manager did not start, or its autologin found no session",
            "a desktop package's setup (accounts, caches, services) did not run in the rootfs recipe",
            "a desktop app is missing, or was compiled against another Qt than the desktop ships",
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
