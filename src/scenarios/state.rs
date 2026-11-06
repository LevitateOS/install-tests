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
    /// Load state from disk; only an absent file means no results exist.
    pub fn load(distro_id: &str) -> Result<Self> {
        let path = state_path(distro_id);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Reading scenario state '{}'", path.display()))
            }
        };
        serde_json::from_str(&contents).with_context(|| {
            format!(
                "Invalid scenario state '{}'. Reset it with: cargo xtask scenarios reset {}",
                path.display(),
                distro_id
            )
        })
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

    /// Update this scenario's input fingerprint and invalidate only its result.
    pub fn reset_for_scenario_input(&mut self, scenario: ScenarioId, fingerprint: &str) {
        self.input_fingerprints
            .insert(scenario.key().to_string(), fingerprint.to_string());
        self.results.remove(scenario.key());
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

    /// Returns true if a result exists for the given scenario.
    pub fn has_result(&self, scenario: ScenarioId) -> bool {
        self.results.contains_key(scenario.key())
    }
}

pub(crate) fn state_path(distro_id: &str) -> PathBuf {
    let root = std::env::var_os("LEVITATE_SCENARIO_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::repo_root().join(".scenarios"));
    root.join(format!("{}.json", distro_id))
}

fn unix_timestamp_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s_since_epoch", d.as_secs())
}
