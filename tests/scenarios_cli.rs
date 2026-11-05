//! Public CLI checks for independent scenario state and command selection.
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn command(state_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scenarios"));
    command.env("LEVITATE_SCENARIO_STATE_DIR", state_dir);
    command.env("NO_COLOR", "1");
    command
}

fn diagnostic(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

#[test]
fn status_never_reports_stale_cached_success_as_a_pass() {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("levitate.json");
    let cached = r#"{
        "input_fingerprints": {"live-tools": "obsolete-input", "runtime": "obsolete-input"},
        "results": {
            "live-tools": {"passed": true, "timestamp": "old", "evidence": "old run"},
            "runtime": {"passed": true, "timestamp": "old", "evidence": "old run"}
        }
    }"#;
    fs::write(&state, cached).unwrap();
    let output = command(temp.path())
        .args(["--distro", "levitate", "--status"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", diagnostic(&output));
    let text = String::from_utf8(output.stdout).unwrap();
    for scenario in ["live-tools", "runtime"] {
        let row = text.lines().find(|line| line.contains(scenario)).unwrap();
        assert!(row.contains("[STALE]"), "{text}");
    }
    assert!(!text.contains("[PASS]"), "{text}");
    assert!(!text.contains("Highest passed"), "{text}");
    assert_eq!(fs::read_to_string(state).unwrap(), cached);
}

#[test]
fn corrupt_state_is_reported_and_preserved_until_explicit_reset() {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("acorn.json");
    fs::write(&state, "{broken cache").unwrap();
    let output = command(temp.path())
        .args(["--distro", "acorn", "--status"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", diagnostic(&output));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("Invalid scenario state"), "{error}");
    assert!(
        error.contains("cargo xtask scenarios reset acorn"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&state).unwrap(), "{broken cache");
    let reset = command(temp.path())
        .args(["--distro", "acorn", "--reset"])
        .output()
        .unwrap();
    assert!(reset.status.success(), "{}", diagnostic(&reset));
    assert!(!state.exists());
}

#[test]
fn ordered_runner_and_conflicting_actions_are_rejected_without_mutation() {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("levitate.json");
    fs::write(&state, "sentinel").unwrap();
    for args in [
        vec!["--distro", "levitate", "--up-to-scenario", "runtime"],
        vec!["--distro", "levitate", "--status", "--reset"],
        vec!["--distro", "levitate", "--status", "--force"],
    ] {
        let output = command(temp.path()).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{}", diagnostic(&output));
        assert_eq!(fs::read_to_string(&state).unwrap(), "sentinel");
    }
}

#[test]
fn unknown_distro_cannot_reset_an_arbitrary_state_path() {
    let temp = TempDir::new().unwrap();
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let unrelated = temp.path().join("other.json");
    fs::write(&unrelated, "preserve me").unwrap();
    let output = command(&cache)
        .args(["--distro", "../other", "--reset"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", diagnostic(&output));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown distro"));
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "preserve me");
}
