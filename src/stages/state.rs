//! Stage state persistence.
//!
//! Tracks which stages passed per distro, with ISO mtime-based invalidation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Persisted state for a single distro's stages.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StageState {
    /// Stage 00 (build-only) ISO file mtime.
    /// `alias = "iso_mtime_secs"` preserves compatibility with old state files.
    #[serde(default, alias = "iso_mtime_secs")]
    pub stage00_iso_mtime_secs: u64,
    /// Runtime ISO mtime used for Stage 01+ checks.
    #[serde(default)]
    pub runtime_iso_mtime_secs: u64,
    /// Runtime ISO mtime by stage number (Stage 01+).
    #[serde(default)]
    pub runtime_iso_mtime_secs_by_stage: HashMap<u32, u64>,
    /// Map of stage number -> result.
    pub results: HashMap<u32, StageResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StageResult {
    pub passed: bool,
    pub timestamp: String,
    pub evidence: String,
}

impl StageState {
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

    /// Check if state is still valid for the given stage ISO.
    /// Stage 00 uses the build-only ISO mtime.
    /// Stage 01+ uses per-stage runtime ISO mtime.
    pub fn is_valid_for_stage_iso(&self, stage: u32, iso_path: &Path) -> bool {
        match iso_mtime_secs(iso_path) {
            Some(mtime) => {
                if stage == 0 {
                    self.stage00_iso_mtime_secs == mtime
                } else {
                    self.runtime_iso_mtime_secs_by_stage
                        .get(&stage)
                        .copied()
                        // Compatibility fallback for older state files that only stored
                        // a single runtime mtime.
                        .unwrap_or(self.runtime_iso_mtime_secs)
                        == mtime
                }
            }
            None => false,
        }
    }

    /// Update stage ISO mtime and clear affected stage results.
    /// Stage 00 rebuild invalidates all stage results.
    /// Stage N (N>=01) rebuild invalidates Stage N+ results while preserving lower stages.
    pub fn reset_for_stage_iso(&mut self, stage: u32, iso_path: &Path) {
        let mtime = iso_mtime_secs(iso_path).unwrap_or(0);
        if stage == 0 {
            self.stage00_iso_mtime_secs = mtime;
            self.runtime_iso_mtime_secs = 0;
            self.runtime_iso_mtime_secs_by_stage.clear();
            self.results.clear();
            return;
        }

        self.runtime_iso_mtime_secs_by_stage.insert(stage, mtime);
        // Keep legacy field for compatibility with older tooling that may still read it.
        if stage == 1 {
            self.runtime_iso_mtime_secs = mtime;
        }
        self.results.retain(|s, _| *s < stage);
    }

    /// Record a stage result.
    pub fn record(&mut self, stage: u32, passed: bool, evidence: &str) {
        let now = chrono_now();
        self.results.insert(
            stage,
            StageResult {
                passed,
                timestamp: now,
                evidence: evidence.to_string(),
            },
        );
    }

    /// Check if a stage has already passed.
    pub fn has_passed(&self, stage: u32) -> bool {
        self.results.get(&stage).map(|r| r.passed).unwrap_or(false)
    }

    /// Returns true if state contains any result at or above `stage`.
    pub fn has_any_results_from(&self, stage: u32) -> bool {
        self.results.keys().any(|s| *s >= stage)
    }

    /// Highest stage that passed.
    pub fn highest_passed(&self) -> u32 {
        // Must be contiguous from 1
        let mut n = 0;
        while self.has_passed(n + 1) {
            n += 1;
        }
        n
    }
}

fn state_path(distro_id: &str) -> PathBuf {
    // Find the repo root by looking for .stages/ relative to the workspace
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.stages")
        .join(format!("{}.json", distro_id))
}

fn iso_mtime_secs(iso_path: &Path) -> Option<u64> {
    std::fs::metadata(iso_path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn chrono_now() -> String {
    // Simple ISO-ish timestamp without pulling in chrono
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s_since_epoch", d.as_secs())
}
