//! What the scenarios run in a guest, over either transport
//! (`recqemu::Guest`: the serial console or SSH): the evidence scripts with
//! their library and init backend, the tool checks, and the diagnostics of
//! a live system whose SSH does not answer.

use crate::budget::Budget;
use crate::expect::Evidence;
use anyhow::{bail, Context, Result};
use distro_contract::profile::{InitSystem, PrivilegeTool};
use distro_contract::Variant;
use recqemu::{shell_quote, Guest, Outcome};
use std::fs;
use std::path::Path;

/// Where the scripts' tree goes in the guest (`/run`: never in an image).
const GUEST_DIR: &str = "/run/scenario-tests";
/// The scripts' tree in the checkout.
const SCRIPTS_DIR: &str = "testing/install-tests/test-scripts";

/// The init backend's file under `lib/init/`.
fn backend_file(init: InitSystem) -> String {
    format!("lib/init/{}.sh", init.as_str())
}

/// Run `command`, failing unless it exits 0 within `budget`.
pub fn run(guest: &mut dyn Guest, what: &str, command: &str, budget: Budget) -> Result<String> {
    match guest.exec(command, budget.get())? {
        Outcome::Exited { code: 0, output, .. } => Ok(output),
        outcome => bail!("{what} failed ({}): {}", outcome.describe(), outcome.output().trim()),
    }
}

/// Write `content` to `dest` in the guest, in as few commands as the
/// transport carries. A chunk is measured quoted twice (once for `printf`,
/// once more by the serial protocol's `eval` wrapper, where a `'` becomes 13
/// characters), so a command never exceeds `max_command_len`.
fn write_file(guest: &mut dyn Guest, content: &str, dest: &str) -> Result<()> {
    let dest_q = shell_quote(dest);
    run(guest, "preparing a guest file", &format!("mkdir -p \"$(dirname {dest_q})\" && : > {dest_q}"), Budget::Command)?;
    let room = guest.max_command_len().saturating_sub(4 * dest_q.len() + 64);
    let mut chunk = String::new();
    let mut quoted_len = 2;
    for c in content.chars() {
        let grow = if c == '\'' { 13 } else { c.len_utf8() };
        if quoted_len + grow > room && !chunk.is_empty() {
            run(guest, "copying a file into the guest", &format!("printf '%s' {} >> {dest_q}", shell_quote(&chunk)), Budget::Command)?;
            chunk.clear();
            quoted_len = 2;
        }
        chunk.push(c);
        quoted_len += grow;
    }
    if !chunk.is_empty() {
        run(guest, "copying a file into the guest", &format!("printf '%s' {} >> {dest_q}", shell_quote(&chunk)), Budget::Command)?;
    }
    Ok(())
}

/// Copy `relative` (under the scripts' tree) into the guest's tree.
fn copy_script(guest: &mut dyn Guest, repo: &Path, relative: &str) -> Result<()> {
    let source = repo.join(SCRIPTS_DIR).join(relative);
    let content =
        fs::read_to_string(&source).with_context(|| format!("reading {}", source.display()))?;
    write_file(guest, &content, &format!("{GUEST_DIR}/{relative}"))
}

/// Copy the script library and the variant's init backend into the guest.
pub fn install_lib(guest: &mut dyn Guest, variant: &Variant) -> Result<()> {
    copy_script(guest, &variant.repo_root, "lib/common.sh")?;
    copy_script(guest, &variant.repo_root, &backend_file(variant.contract.profile.init))
}

/// Run a function of the init backend (`svc_failed`, `svc_add_failing /mnt x`);
/// the library must be in the guest.
pub fn backend(guest: &mut dyn Guest, variant: &Variant, call: &str, budget: Budget) -> Result<Outcome> {
    let file = shell_quote(&format!("{GUEST_DIR}/{}", backend_file(variant.contract.profile.init)));
    guest.exec(&format!(". {file} && {call}"), budget.get())
}

/// The facts every evidence script gets.
fn facts(variant: &Variant) -> Vec<(&'static str, String)> {
    let contract = &variant.contract;
    vec![
        ("SCENARIO_INIT", contract.profile.init.as_str().to_string()),
        ("SCENARIO_SERVICES", contract.live_service_units().join(" ")),
        ("SCENARIO_PRIVILEGE", contract.profile.privilege.as_str().to_string()),
        ("SCENARIO_PRIVILEGE_CONFIG", contract.profile.privilege.config_file().to_string()),
        ("SCENARIO_OS_ID", contract.identity.os_id.clone()),
    ]
}

/// Copy a scenario's evidence script (with the library and backend) into the
/// guest and run it with the variant's facts and `env`; it passes when it
/// exits 0 and prints its pass marker.
pub fn run_evidence(
    guest: &mut dyn Guest,
    variant: &Variant,
    evidence: &Evidence,
    env: &[(&str, &str)],
) -> Result<String> {
    install_lib(guest, variant)?;
    copy_script(guest, &variant.repo_root, &evidence.script)?;
    let mut command = format!("SCENARIO_PASS_MARKER={}", shell_quote(&evidence.pass_marker));
    let facts = facts(variant);
    for (key, value) in facts.iter().map(|(k, v)| (*k, v.as_str())).chain(env.iter().copied()) {
        command.push_str(&format!(" {key}={}", shell_quote(value)));
    }
    let path = format!("{GUEST_DIR}/{}", evidence.script);
    command.push_str(&format!(" sh {}", shell_quote(&path)));
    let outcome = guest.exec(&command, Budget::Evidence.get())?;
    let marker = outcome.output().contains(&evidence.pass_marker);
    if !outcome.success() || !marker {
        bail!(
            "evidence script {path} failed ({}, pass marker {}):\n{}",
            outcome.describe(),
            if marker { "printed" } else { "missing" },
            outcome.output().trim()
        );
    }
    Ok(format!("{} passed", evidence.script))
}

/// How a tool fared.
enum ToolCheck {
    Works,
    Missing,
    Broken(String),
}

/// Check that each tool is on PATH (`command -v`) and runs: its known
/// harmless invocation must exit 0, so a missing library or a broken binary
/// fails, not just absence from PATH. Fails listing every missing and broken
/// tool; returns the working ones.
pub fn check_tools(guest: &mut dyn Guest, variant: &Variant, tools: &[String]) -> Result<String> {
    let (mut found, mut missing, mut broken) = (Vec::new(), Vec::new(), Vec::new());
    for tool in tools {
        match check_tool(guest, variant, tool)? {
            ToolCheck::Works => found.push(tool.as_str()),
            ToolCheck::Missing => missing.push(tool.as_str()),
            ToolCheck::Broken(how) => broken.push(format!("{tool} ({how})")),
        }
    }
    if !missing.is_empty() || !broken.is_empty() {
        bail!(
            "Missing tools (not in PATH): {}\nBroken tools (on PATH, failed to run): {}\nWorking tools: {}",
            missing.join(", "),
            broken.join(", "),
            found.join(", ")
        );
    }
    Ok(format!("all {} tools run: {}", found.len(), found.join(", ")))
}

fn check_tool(guest: &mut dyn Guest, variant: &Variant, tool: &str) -> Result<ToolCheck> {
    let presence = guest.exec(&format!("command -v {} >/dev/null", shell_quote(tool)), Budget::Command.get())?;
    match presence {
        Outcome::Exited { code: 0, .. } => {}
        Outcome::Exited { .. } => return Ok(ToolCheck::Missing),
        Outcome::TimedOut { .. } => return Ok(ToolCheck::Broken("command -v timed out".to_string())),
    }
    let privilege = variant.contract.profile.privilege;
    let probe = if tool == privilege.as_str() {
        Some(privilege_check(privilege))
    } else {
        tool_run_command(tool)
    };
    let Some(probe) = probe else {
        return Ok(ToolCheck::Works);
    };
    Ok(match guest.exec(&format!("{{ {probe}; }} >/dev/null 2>&1"), Budget::Command.get())? {
        outcome if outcome.success() => ToolCheck::Works,
        outcome => ToolCheck::Broken(outcome.describe()),
    })
}

/// A harmless invocation of `tool` that exits 0 when it works, or `None`
/// when the tool has no such invocation and presence on PATH is the check
/// (busybox `vi`).
fn tool_run_command(tool: &str) -> Option<String> {
    Some(match tool {
        "recpart" | "recstrap" | "recfstab" | "recchroot" => format!("{tool} --help"),
        // A TUI app's --help runs the compiled binary's app and exits before the TTY check.
        "recab-tui" => format!("{tool} --help"),
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

/// What a live system whose SSH does not answer looks like from its serial
/// shell: the network, the injected payload, and sshd through the init
/// backend, the same probes on every variant.
pub fn ssh_diagnostics(guest: &mut dyn Guest, variant: &Variant) -> String {
    let mut report = String::from("live system SSH diagnostics from the serial shell:\n");
    if let Err(err) = install_lib(guest, variant) {
        report.push_str(&format!("(could not copy the init backend: {err:#})\n"));
    }
    let probes = [
        ("Kernel cmdline", "cat /proc/cmdline".to_string()),
        ("Network interfaces", "ip -brief addr".to_string()),
        ("Network routes", "ip route".to_string()),
        ("Root SSH auth files", "ls -ld /root /root/.ssh /root/.ssh/authorized_keys".to_string()),
        (
            "Injected boot payload",
            format!(
                "ls -l {d}; cat {d}/source {e}",
                d = recinit::live::BOOT_INJECTION_DIR,
                e = recinit::live::BOOT_INJECTION_ENV
            ),
        ),
        ("sshd auth settings", "sshd -T | grep -E 'permitrootlogin|pubkeyauthentication|authorizedkeysfile'".to_string()),
        ("Port 22 listeners", "ss -lntp | grep ':22'".to_string()),
    ];
    for (title, command) in probes {
        report.push_str(&format!("\n--- {title} ---\n$ {command}\n"));
        report.push_str(&describe(guest.exec(&command, Budget::Command.get())));
    }
    report.push_str("\n--- sshd service ---\n");
    report.push_str(&describe(backend(guest, variant, "svc_diagnose sshd", Budget::Command)));
    report
}

fn describe(outcome: Result<Outcome>) -> String {
    match outcome {
        Ok(outcome) => format!("({})\n{}\n", outcome.describe(), outcome.output().trim()),
        Err(err) => format!("(failed to collect: {err:#})\n"),
    }
}
