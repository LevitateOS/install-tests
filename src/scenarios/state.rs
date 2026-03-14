//! Scenario state persistence.
//!
//! Tracks which scenario checks passed per distro, with resolved-input
//! fingerprint invalidation.

use super::ScenarioId;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persisted state for a single distro's scenario runs.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ScenarioState {
    /// Map of canonical scenario name -> resolved input fingerprint.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub input_fingerprints: std::collections::HashMap<String, String>,
    /// Map of canonical scenario name -> result.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub results: std::collections::HashMap<String, ScenarioResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScenarioResult {
    pub passed: bool,
    pub timestamp: String,
    pub evidence: String,
}

impl ScenarioState {
    /// Load state from disk, or return default if missing/corrupt.
    pub fn load(distro_id: &str) -> Self {
        let path = state_path(distro_id);
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save state to disk.
    pub fn save(&self, distro_id: &str) -> Result<()> {
        let path = state_path(distro_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json).with_context(|| format!("Writing {}", path.display()))?;
        Ok(())
    }

    /// Check if state is still valid for the given scenario input fingerprint.
    pub fn is_valid_for_scenario_input(&self, scenario: ScenarioId, fingerprint: &str) -> bool {
        self.input_fingerprints
            .get(scenario.key())
            .map(|stored| stored == fingerprint)
            .unwrap_or(false)
    }

    /// Update scenario input fingerprint and clear affected scenario results.
    ///
    /// A scenario input change invalidates that scenario and every later scenario
    /// in the canonical ladder while preserving earlier results.
    pub fn reset_for_scenario_input(&mut self, scenario: ScenarioId, fingerprint: &str) {
        self.input_fingerprints
            .insert(scenario.key().to_string(), fingerprint.to_string());
        self.results.retain(|key, _| {
            ScenarioId::parse_key(key)
                .map(|existing| existing.ordinal() < scenario.ordinal())
                .unwrap_or(false)
        });
    }

    /// Record a scenario result.
    pub fn record(&mut self, scenario: ScenarioId, passed: bool, evidence: &str) {
        let now = unix_timestamp_string();
        self.results.insert(
            scenario.key().to_string(),
            ScenarioResult {
                passed,
                timestamp: now,
                evidence: evidence.to_string(),
            },
        );
    }

    /// Check if a scenario has already passed.
    pub fn has_passed(&self, scenario: ScenarioId) -> bool {
        self.results
            .get(scenario.key())
            .map(|r| r.passed)
            .unwrap_or(false)
    }

    /// Returns true if state contains any result at or above `scenario`.
    pub fn has_any_results_from(&self, scenario: ScenarioId) -> bool {
        self.results.keys().any(|key| {
            ScenarioId::parse_key(key)
                .map(|existing| existing.ordinal() >= scenario.ordinal())
                .unwrap_or(false)
        })
    }

    /// Highest contiguous scenario that passed.
    pub fn highest_passed(&self) -> Option<ScenarioId> {
        let mut highest = None;
        for scenario in ScenarioId::ALL {
            if self.has_passed(scenario) {
                highest = Some(scenario);
                continue;
            }
            break;
        }
        highest
    }

    /// Returns true if a result exists for the given scenario.
    pub fn has_result(&self, scenario: ScenarioId) -> bool {
        self.results.contains_key(scenario.key())
    }
}

pub(crate) fn state_path(distro_id: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.scenarios")
        .join(format!("{}.json", distro_id))
}

fn unix_timestamp_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s_since_epoch", d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_for_scenario_input_drops_later_results_only() {
        let mut state = ScenarioState::default();
        state.record(ScenarioId::BuildPreflight, true, "ok");
        state.record(ScenarioId::LiveBoot, true, "ok");
        state.record(ScenarioId::LiveTools, true, "ok");
        state.record(ScenarioId::Install, true, "ok");

        state.reset_for_scenario_input(ScenarioId::LiveTools, "fingerprint");

        assert!(state.has_passed(ScenarioId::BuildPreflight));
        assert!(state.has_passed(ScenarioId::LiveBoot));
        assert!(!state.has_result(ScenarioId::LiveTools));
        assert!(!state.has_result(ScenarioId::Install));
    }

    #[test]
    fn canonical_state_path_uses_scenarios_dir() {
        let path = state_path("levitate");
        assert!(path.ends_with(".scenarios/levitate.json"));
    }
}
