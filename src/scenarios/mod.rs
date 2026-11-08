//! Scenario-based development loop.
//!
//! Independent named scenarios give focused feedback during development.
//! Each scenario consumes existing artifacts and validates one behavior. The
//! scenarios, their inputs and their order are `distro_contract::ScenarioId`;
//! what each expects of a variant is `crate::expect`.

pub mod state;

mod ab_update;
mod install;
use crate::expect::{self, Evidence};
use crate::preflight::require_preflight;
use crate::qemu::session;
use crate::qemu::{Console, SerialExecutorExt};
use anyhow::{bail, Context, Result};
use colored::Colorize;
use distro_contract::profile::{InitSystem, PrivilegeTool};
use distro_contract::{ProductId, Variant};
use recab::InitSystemExt;
use recpart::InstallMode;
use recshuttle::Remote;
use distro_builder::install_ux::{LAUNCHER, PROBE_PREFIX, SMOKE_ENV, SPLIT, SPLIT_SMOKE_OK};
use crate::qemu::session::VmSpec;
use crate::repo_root;
use distro_builder::run_history;
use serde::Serialize;
use state::ScenarioState;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

pub use distro_contract::ScenarioId;

const SCENARIO_RUNTIME_RETENTION_COUNT: usize = 5;
pub(super) const INSTALL_DISK_FILENAME: &str = "disk.qcow2";
const INSTALL_OVMF_VARS_FILENAME: &str = "ovmf-vars.fd";
const SCENARIO_ROOT_DIRNAME: &str = "scenarios";
/// Where the scenarios' guest-side scripts live.
const TEST_SCRIPTS_DIR: &str = "testing/install-tests/test-scripts";

/// How the install scenario installs, and so what the installed-system
/// scenarios boot: the A/B layout that recab updates.
pub(super) const INSTALL_MODE: InstallMode = InstallMode::Ab;

#[derive(Debug, Clone)]
pub struct InstallScenarioRuntime {
    pub run_id: String,
    pub disk_path: PathBuf,
    pub ovmf_vars_path: PathBuf,
}

/// A scenario's inputs (`ScenarioId::input`), resolved to what exists on disk.
struct Inputs {
    iso: Option<(ProductId, PathBuf)>,
    install: Option<InstallScenarioRuntime>,
}

impl Inputs {
    fn resolve(distro_id: &str, scenario: ScenarioId) -> Result<Self> {
        let input = scenario.input();
        Ok(Self {
            iso: input
                .iso
                .map(|product| resolve_release_iso(distro_id, product).map(|iso| (product, iso)))
                .transpose()?,
            install: input
                .install_run
                .then(|| resolve_latest_install_runtime(distro_id))
                .transpose()?,
        })
    }

    fn iso(&self, scenario: ScenarioId) -> Result<&Path> {
        self.iso
            .as_ref()
            .map(|(_, iso)| iso.as_path())
            .with_context(|| format!("the registry gives {scenario} no ISO input"))
    }

    fn install(&self, scenario: ScenarioId) -> Result<&InstallScenarioRuntime> {
        self.install
            .as_ref()
            .with_context(|| format!("the registry gives {scenario} no install-run input"))
    }
}

/// The variant `distro_id`, loaded once for the whole run.
fn load_variant(distro_id: &str) -> Result<Variant> {
    Ok(distro_contract::load(&repo_root(), distro_id)?)
}

/// Run one scenario for a distro; `force` reruns it even when a cached pass
/// for the same inputs exists. A failure is the error, with its cause.
pub fn run_scenario(distro_id: &str, scenario: ScenarioId, force: bool) -> Result<()> {
    let variant = load_variant(distro_id)?;
    let distro_id = variant.id.as_str();
    let inputs = Inputs::resolve(distro_id, scenario)?;
    if let Some((product, iso)) = &inputs.iso {
        require_preflight(iso, *product, &variant)?;
    }

    let input_fingerprint = scenario_input_fingerprint(&variant, &inputs)?;

    let mut state = ScenarioState::load(distro_id)?;
    if !state.is_valid_for_scenario_input(scenario, &input_fingerprint) {
        println!(
            "{}",
            format!(
                "Scenario input changed for {} — resetting its cached result.",
                scenario.title()
            )
            .yellow()
        );
        state.reset_for_scenario_input(scenario, &input_fingerprint);
        state.save(distro_id)?;
    }

    if force {
        state.results.remove(scenario.key());
        state.save(distro_id)?;
        println!("Forcing {} rerun.", scenario.title());
    }

    if state.has_passed(scenario) {
        println!(
            "{} {} already passed (rerun with --force).",
            "[SKIP]".green(),
            scenario.title()
        );
        return Ok(());
    }

    println!("{} {}", ">>".cyan(), scenario.title());

    let result = match scenario {
        ScenarioId::BuildPreflight => inputs.iso(scenario).map(|iso| {
            format!(
                "Preflight conformance + artifact checks passed for {}",
                iso.display()
            )
        }),
        ScenarioId::LiveBoot => inputs.iso(scenario).and_then(|iso| run_live_boot(&variant, iso)),
        ScenarioId::LiveTools => inputs.iso(scenario).and_then(|iso| run_live_tools(&variant, iso)),
        ScenarioId::Install => inputs.iso(scenario).and_then(|iso| run_installation(&variant, iso)),
        ScenarioId::InstalledBoot => {
            inputs.install(scenario).and_then(|install| run_installed_boot(&variant, install))
        }
        ScenarioId::AutomatedLogin => {
            inputs.install(scenario).and_then(|install| run_automated_login(&variant, install))
        }
        ScenarioId::Runtime => inputs.install(scenario).and_then(|install| run_runtime(&variant, install)),
        ScenarioId::AbUpdate => inputs.iso(scenario).and_then(|iso| {
            ab_update::run(&variant, inputs.install(scenario)?, iso)
        }),
    };

    match result {
        Ok(evidence) => {
            state.record(scenario, true, &evidence);
            state.save(distro_id)?;
            println!(
                "{} {} passed: {}",
                "[PASS]".green().bold(),
                scenario.title(),
                evidence
            );
            Ok(())
        }
        Err(e) => {
            state.record(scenario, false, &format!("{:#}", e));
            state.save(distro_id)?;
            print_failure(scenario, &e);
            Err(e.context(format!("scenario {scenario} failed for {distro_id}")))
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
    /// Has a result, but its inputs cannot be resolved now (an ISO or the
    /// install run is missing or unreadable): why.
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

/// Each scenario's status for a distro, in run order.
pub fn scenario_statuses(distro_id: &str) -> Result<Vec<(ScenarioId, ScenarioStatus)>> {
    statuses(&load_variant(distro_id)?)
}

fn statuses(variant: &Variant) -> Result<Vec<(ScenarioId, ScenarioStatus)>> {
    let state = ScenarioState::load(&variant.id)?;
    Ok(ScenarioId::ALL
        .into_iter()
        .map(|scenario| {
            if !state.has_result(scenario) {
                return (scenario, ScenarioStatus::NotRun);
            }
            let fingerprint = Inputs::resolve(&variant.id, scenario)
                .and_then(|inputs| scenario_input_fingerprint(variant, &inputs));
            let status = match fingerprint {
                Err(err) => ScenarioStatus::Error(format!("{err:#}")),
                Ok(fingerprint) if !state.is_valid_for_scenario_input(scenario, &fingerprint) => {
                    ScenarioStatus::Stale
                }
                Ok(_) if state.has_passed(scenario) => ScenarioStatus::Pass,
                Ok(_) => ScenarioStatus::Fail,
            };
            (scenario, status)
        })
        .collect())
}

/// Print scenario status for a distro.
pub fn print_status(distro_id: &str) -> Result<()> {
    let variant = load_variant(distro_id)?;
    println!("{} Scenario Status", variant.contract.identity.os_name.bold());
    println!();
    for (scenario, status) in statuses(&variant)? {
        let label = match &status {
            ScenarioStatus::Stale => "[STALE]".yellow(),
            ScenarioStatus::Error(_) => "[ERROR]".red(),
            ScenarioStatus::Pass => "[PASS]".green(),
            ScenarioStatus::Fail => "[FAIL]".red(),
            ScenarioStatus::NotRun => "[    ]".dimmed(),
        };
        println!("  {} {:<15} {}", label, scenario.key(), scenario.title());
        if let ScenarioStatus::Error(why) = &status {
            println!("          {}", why.lines().next().unwrap_or_default().dimmed());
        }
    }
    println!();
    Ok(())
}

/// Reset all scenario state for a distro.
pub fn reset_state(distro_id: &str) -> Result<()> {
    let canonical_distro_id = load_variant(distro_id)?.id;
    let path = state::state_path(&canonical_distro_id);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    println!("Scenario state reset for {}.", canonical_distro_id);
    Ok(())
}

/// Live Boot scenario — ISO boots in QEMU.
fn run_live_boot(variant: &Variant, iso_path: &Path) -> Result<String> {
    let mut vm = session::spawn(VmSpec {
        name: &format!("{}-live-boot", variant.id),
        iso: Some(iso_path),
        ssh: true,
        ..Default::default()
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = &mut vm.console;
    console.wait_for_live_boot_with_context(live_boot_stall(variant), variant)?;
    verify_live_boot_ssh_login(console, ssh_host_port)?;

    evidence_over_ssh(ssh_host_port, &scenario_evidence(ScenarioId::LiveBoot)?, &[])?;
    evidence_over_ssh(
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
fn run_live_tools(variant: &Variant, iso_path: &Path) -> Result<String> {
    let mut vm = session::spawn(VmSpec {
        name: &format!("{}-live-tools", variant.id),
        iso: Some(iso_path),
        ssh: true,
        ..Default::default()
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = &mut vm.console;
    wait_for_live_tools_serial_readiness(console, variant)?;
    verify_live_boot_ssh_login(console, ssh_host_port)?;

    let required = variant.contract.live_tools();
    let tools: Vec<&str> = required.iter().map(String::as_str).collect();
    let mut missing = Vec::new();
    let mut found = Vec::new();
    let mut broken = Vec::new();

    let remote = Remote::new(ssh_host_port);
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

    let remote = Remote::new(ssh_host_port);
    remote.run(
        "checking the install screen launcher",
        &format!("test -x /{LAUNCHER}"),
    )?;
    let ux_split_evidence = verify_install_ux_split_behavior(&remote)?;

    let script_evidence = evidence_over_ssh(
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

fn run_installation(variant: &Variant, iso_path: &Path) -> Result<String> {
    let mode = INSTALL_MODE;
    let scenario_run = ScenarioRun::start(&variant.id, ScenarioId::Install, None)?;
    let disk_path = scenario_run.output_dir.join(INSTALL_DISK_FILENAME);
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    // An A/B layout holds two 20 GiB root slots and at least 4 GiB of state
    // (recpart's defaults); the image is sparse either way.
    let disk_size = match mode {
        InstallMode::Ab => "48G",
        InstallMode::Mutable => "20G",
    };
    recqemu::create_disk(&disk_path, disk_size)?;

    let ovmf_vars_path = session::setup_ovmf_vars_at(
        &scenario_run.output_dir.join(INSTALL_OVMF_VARS_FILENAME),
    )?;
    let mut vm = session::spawn(VmSpec {
        name: &format!("{}-install", variant.id),
        iso: Some(iso_path),
        disk: Some(&disk_path),
        ovmf_vars: Some(&ovmf_vars_path),
        ssh: true,
        ..Default::default()
    })?;
    let ssh_host_port = vm.ssh_port()?;
    let console = &mut vm.console;

    // The installation runs over SSH; the serial console only marks the
    // boot boundary.
    let remote = Remote::new(ssh_host_port);
    let install_result = (|| -> Result<usize> {
        console.wait_for_live_boot_with_context(live_boot_stall(variant), variant)?;
        remote.wait_ready(live_boot_stall(variant))?;

        let system = install::TestSystem {
            mode,
            root_password: expect::root_password(variant),
        };
        let (target, step_count) = install::install(&remote, variant, &system)?;
        evidence_over_ssh(
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
                    format!("test -L {target}/{}", variant.contract.profile.init.trial_service_link()),
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

    remote.power_off();
    drop(vm);

    match install_result {
        Ok(step_count) => {
            let evidence = format!(
                "{} install steps completed + verified over SSH",
                step_count
            );
            scenario_run.finish_success(
                &evidence,
                Some(disk_path.as_path()),
                Some(ovmf_vars_path.as_path()),
            )?;
            Ok(evidence)
        }
        Err(err) => {
            let failure = format!("FAIL: {:#}", err);
            let _ = scenario_run.finish_failed(
                &failure,
                Some(disk_path.as_path()),
                Some(ovmf_vars_path.as_path()),
            );
            Err(err)
        }
    }
}

fn run_installed_boot(variant: &Variant, install_runtime: &InstallScenarioRuntime) -> Result<String> {
    let scenario_run = ScenarioRun::start(
        &variant.id,
        ScenarioId::InstalledBoot,
        Some(install_runtime.run_id.clone()),
    )?;
    let mut vm = installed_vm(variant, install_runtime, ScenarioId::InstalledBoot)?;
    let console = &mut vm.console;

    let result = console.wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, variant);
    drop(vm);

    match result {
        Ok(failed_services) => {
            let evidence = if failed_services.is_empty() {
                "Installed system boot markers detected".to_string()
            } else {
                format!(
                    "Installed system boot markers detected; failed services seen: {}",
                    failed_services.join(" | ")
                )
            };
            scenario_run.finish_success(
                &evidence,
                Some(install_runtime.disk_path.as_path()),
                Some(install_runtime.ovmf_vars_path.as_path()),
            )?;
            Ok(evidence)
        }
        Err(e) => {
            let failure = format!("FAIL: {:#}", e);
            let _ = scenario_run.finish_failed(
                &failure,
                Some(install_runtime.disk_path.as_path()),
                Some(install_runtime.ovmf_vars_path.as_path()),
            );
            Err(anyhow::anyhow!("{:#}", e))
        }
    }
}

fn run_automated_login(variant: &Variant, install_runtime: &InstallScenarioRuntime) -> Result<String> {
    let mut vm = installed_vm(variant, install_runtime, ScenarioId::AutomatedLogin)?;
    let console = &mut vm.console;

    console.wait_for_installed_boot_with_context(INSTALLED_BOOT_STALL, variant)?;

    console.login("root", expect::root_password(variant), LOGIN_TIMEOUT)?;

    let result = console.exec("echo SCENARIO_LOGIN_OK", COMMAND_TIMEOUT)?;
    let script_evidence = evidence_on_console(
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
fn run_runtime(variant: &Variant, install_runtime: &InstallScenarioRuntime) -> Result<String> {
    let mut vm = installed_vm(variant, install_runtime, ScenarioId::Runtime)?;
    let console = &mut vm.console;

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

    let script_evidence = evidence_on_console(console, &scenario_evidence(ScenarioId::Runtime)?)?;
    Ok(format!(
        "All {} runtime tools present; {script_evidence}",
        found.len()
    ))
}

/// The ISO of the latest successful release build of `product`.
pub fn resolve_release_iso(distro_id: &str, product: ProductId) -> Result<PathBuf> {
    let release_root = run_history::release_product_dir(&repo_root(), distro_id, product);
    let run_id = run_history::latest_successful_run_id(&release_root)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no successful {product} release for '{distro_id}' under '{}'.\n\
             Build it first: cargo xtask build {product} {distro_id}",
            release_root.display(),
        )
    })?;
    let run_dir = release_root.join(&run_id);
    let manifest = run_history::read_run_manifest(&run_dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "release product '{product}' for '{distro_id}' is missing run-manifest metadata under '{}'.",
            run_dir.display()
        )
    })?;
    if manifest.status != run_history::STATUS_SUCCESS {
        bail!(
            "latest '{product}' release run for '{distro_id}' is not successful under '{}'",
            run_dir.display()
        );
    }
    if manifest.target_kind.as_deref() != Some(run_history::TARGET_KIND_RELEASE_PRODUCT)
        || manifest.target_name.as_deref() != Some(product.name())
    {
        bail!(
            "release run manifest under '{}' does not match expected product '{product}'",
            run_dir.display(),
        );
    }

    manifest
        .iso_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "release product '{product}' for '{distro_id}' is missing ISO output in '{}'.",
                run_dir.display()
            )
        })
}

/// What a cached pass depends on: the scenario's inputs (its ISO, its
/// install run), the guest-side scripts it runs, the variant's contract,
/// and the harness binary, which holds the derived expectations and links
/// recqemu, recshuttle and recpart, so a change to any of them reruns it.
fn scenario_input_fingerprint(variant: &Variant, inputs: &Inputs) -> Result<String> {
    let mtime = |path: &Path| -> Result<u64> {
        Ok(fs::metadata(path)
            .with_context(|| format!("reading metadata for '{}'", path.display()))?
            .modified()
            .with_context(|| format!("reading mtime for '{}'", path.display()))?
            .duration_since(UNIX_EPOCH)
            .with_context(|| format!("mtime before UNIX_EPOCH for '{}'", path.display()))?
            .as_secs())
    };
    let mut parts = Vec::new();
    if let Some((product, iso)) = &inputs.iso {
        parts.push(format!("iso:{product}:{}:{}", iso.display(), mtime(iso)?));
    }
    if let Some(install) = &inputs.install {
        parts.push(format!("install-runtime:{}", install.run_id));
    }
    let harness_path = std::env::current_exe().context("locating the scenario harness binary")?;
    parts.push(format!(
        "harness:{}:{}",
        mtime(&harness_path)?,
        fs::metadata(&harness_path)?.len()
    ));
    parts.push(format!(
        "scripts:{:016x}",
        content_hash(&[repo_root().join(TEST_SCRIPTS_DIR)])?
    ));
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

fn verify_live_boot_ssh_login(console: &mut Console, ssh_host_port: u16) -> Result<()> {
    Remote::new(ssh_host_port)
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
    evidence: &Evidence,
    env: &[(&str, &str)],
    mut exec: impl FnMut(&str) -> Result<(i32, String)>,
) -> Result<String> {
    let name = Path::new(&evidence.script);
    if name.components().count() != 1 || !evidence.script.ends_with(".sh") {
        bail!("invalid evidence script name: {}", evidence.script);
    }
    let scripts = repo_root().join(TEST_SCRIPTS_DIR);
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
    ssh_host_port: u16,
    evidence: &Evidence,
    env: &[(&str, &str)],
) -> Result<String> {
    let remote = Remote::new(ssh_host_port);
    run_evidence(evidence, env, |cmd| {
        let out = remote.exec(cmd)?;
        Ok((out.exit_code, out.output))
    })
}

/// `run_evidence` on the serial console.
fn evidence_on_console(console: &mut Console, evidence: &Evidence) -> Result<String> {
    run_evidence(evidence, &[], |cmd| {
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

#[derive(Debug, Serialize)]
struct ScenarioRunManifest {
    run_id: String,
    distro_id: String,
    scenario_name: String,
    status: String,
    created_at_utc: String,
    finished_at_utc: Option<String>,
    scenario_root_dir: String,
    output_dir: String,
    evidence_path: Option<String>,
    disk_path: Option<String>,
    ovmf_vars_path: Option<String>,
    source_install_run_id: Option<String>,
}

struct ScenarioRun {
    run_id: String,
    distro_id: String,
    scenario: ScenarioId,
    scenario_root_dir: PathBuf,
    output_dir: PathBuf,
    created_at_utc: String,
    source_install_run_id: Option<String>,
}

impl ScenarioRun {
    fn start(
        distro_id: &str,
        scenario: ScenarioId,
        source_install_run_id: Option<String>,
    ) -> Result<Self> {
        let scenario_root_dir = scenario_runtime_root_dir(distro_id, scenario);
        let (run_id, output_dir) =
            run_history::allocate_run_dir(&scenario_root_dir)?;
        let run = Self {
            run_id,
            distro_id: distro_id.to_string(),
            scenario,
            scenario_root_dir,
            output_dir,
            created_at_utc: now_utc_sortable()?,
            source_install_run_id,
        };
        run.write_manifest(run_history::STATUS_BUILDING, None, None, None, None)?;
        Ok(run)
    }

    fn finish_success(
        &self,
        evidence: &str,
        disk_path: Option<&Path>,
        ovmf_vars_path: Option<&Path>,
    ) -> Result<()> {
        let evidence_path = self.write_evidence(evidence)?;
        self.write_manifest(
            run_history::STATUS_SUCCESS,
            Some(now_utc_sortable()?),
            Some(evidence_path.as_path()),
            disk_path,
            ovmf_vars_path,
        )?;
        run_history::prune_old_runs(
            &self.scenario_root_dir,
            SCENARIO_RUNTIME_RETENTION_COUNT,
        )
    }

    fn finish_failed(
        &self,
        evidence: &str,
        disk_path: Option<&Path>,
        ovmf_vars_path: Option<&Path>,
    ) -> Result<()> {
        let evidence_path = self.write_evidence(evidence)?;
        self.write_manifest(
            run_history::STATUS_FAILED,
            Some(now_utc_sortable()?),
            Some(evidence_path.as_path()),
            disk_path,
            ovmf_vars_path,
        )
    }

    fn write_manifest(
        &self,
        status: &str,
        finished_at_utc: Option<String>,
        evidence_path: Option<&Path>,
        disk_path: Option<&Path>,
        ovmf_vars_path: Option<&Path>,
    ) -> Result<()> {
        let metadata = ScenarioRunManifest {
            run_id: self.run_id.clone(),
            distro_id: self.distro_id.clone(),
            scenario_name: self.scenario.key().to_string(),
            status: status.to_string(),
            created_at_utc: self.created_at_utc.clone(),
            finished_at_utc,
            scenario_root_dir: self.scenario_root_dir.display().to_string(),
            output_dir: self.output_dir.display().to_string(),
            evidence_path: evidence_path.map(|p| p.display().to_string()),
            disk_path: disk_path.map(|p| p.display().to_string()),
            ovmf_vars_path: ovmf_vars_path.map(|p| p.display().to_string()),
            source_install_run_id: self.source_install_run_id.clone(),
        };
        let manifest_path = run_history::run_manifest_path(&self.output_dir);
        write_json_atomic(&manifest_path, &metadata).with_context(|| {
            format!(
                "writing scenario runtime metadata '{}'",
                manifest_path.display()
            )
        })
    }

    fn write_evidence(&self, evidence: &str) -> Result<PathBuf> {
        let evidence_path = self.output_dir.join("last-result.txt");
        let body = format!(
            "timestamp_unix_ns={}\ndistro={}\nscenario={}\n{}\n",
            now_unix_nanos()?,
            self.distro_id,
            self.scenario.key(),
            evidence
        );
        fs::write(&evidence_path, body).with_context(|| {
            format!(
                "writing scenario runtime evidence '{}'",
                evidence_path.display()
            )
        })?;
        Ok(evidence_path)
    }
}

pub fn resolve_latest_install_runtime(distro_id: &str) -> Result<InstallScenarioRuntime> {
    let scenario_root = scenario_runtime_root_dir(distro_id, ScenarioId::Install);
    let run_id = run_history::latest_successful_run_id(&scenario_root)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "install scenario runtime not found for '{}': no successful runs under '{}'.\n\
                 Run: cargo xtask test install {}",
                distro_id,
                scenario_root.display(),
                distro_id
            )
        })?;
    let run_dir = scenario_root.join(&run_id);
    let disk_path = run_dir.join(INSTALL_DISK_FILENAME);
    let ovmf_vars_path = run_dir.join(INSTALL_OVMF_VARS_FILENAME);
    if !disk_path.is_file() {
        bail!(
            "install scenario disk image missing for '{}' run '{}': '{}'.\n\
             Re-run install: cargo xtask test install {} --force",
            distro_id,
            run_id,
            disk_path.display(),
            distro_id
        );
    }
    if !ovmf_vars_path.is_file() {
        bail!(
            "install scenario OVMF vars missing for '{}' run '{}': '{}'.\n\
             Re-run install: cargo xtask test install {} --force",
            distro_id,
            run_id,
            ovmf_vars_path.display(),
            distro_id
        );
    }
    Ok(InstallScenarioRuntime {
        run_id,
        disk_path,
        ovmf_vars_path,
    })
}

fn scenario_runtime_root_dir(distro_id: &str, scenario: ScenarioId) -> PathBuf {
    run_history::distro_output_dir(&repo_root(), distro_id)
        .join(SCENARIO_ROOT_DIRNAME)
        .join(scenario.key())
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

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot write scenario runtime metadata without parent directory: '{}'",
            path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating metadata parent directory '{}'", parent.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let payload = serde_json::to_vec_pretty(value)
        .with_context(|| "serializing scenario runtime metadata")?;
    fs::write(&tmp, payload).with_context(|| format!("writing temp file '{}'", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "renaming temp metadata '{}' to '{}'",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn now_utc_sortable() -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .with_context(|| {
            "system clock before UNIX_EPOCH while recording scenario runtime metadata".to_string()
        })?;
    Ok(format!("{:020}{:09}", now.as_secs(), now.subsec_nanos()))
}

fn now_unix_nanos() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .with_context(|| {
            "system clock before UNIX_EPOCH while recording scenario runtime evidence".to_string()
        })?
        .as_nanos())
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

/// Longest silence tolerated during live boot before it counts as a stall:
/// OpenRC's early boot can stay quiet for longer than systemd's.
fn live_boot_stall(variant: &Variant) -> Duration {
    Duration::from_secs(match variant.contract.profile.init {
        InitSystem::Systemd => 60,
        InitSystem::Openrc => 180,
    })
}

/// The evidence of a scenario that runs a guest-side script.
fn scenario_evidence(scenario: ScenarioId) -> Result<Evidence> {
    Evidence::of(scenario).with_context(|| format!("{scenario} runs no guest-side script"))
}

/// Boot the latest install run's disk.
fn installed_vm(
    variant: &Variant,
    install_runtime: &InstallScenarioRuntime,
    scenario: ScenarioId,
) -> Result<session::Vm> {
    session::spawn(VmSpec {
        name: &format!("{}-{}", variant.id, scenario.key()),
        disk: Some(&install_runtime.disk_path),
        ovmf_vars: Some(&install_runtime.ovmf_vars_path),
        ..Default::default()
    })
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
