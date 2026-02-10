//! Checkpoint state persistence.
//!
//! Tracks which checkpoints passed per distro, with ISO mtime-based invalidation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Persisted state for a single distro's checkpoints.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CheckpointState {
    /// ISO file mtime (as seconds since epoch) when checkpoints were run.
    pub iso_mtime_secs: u64,
    /// Map of checkpoint number -> result.
    pub results: HashMap<u32, CheckpointResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckpointResult {
    pub passed: bool,
    pub timestamp: String,
    pub evidence: String,
}

impl CheckpointState {
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

    /// Check if state is still valid for the given ISO.
    /// Returns false if ISO was rebuilt (mtime changed).
    pub fn is_valid_for_iso(&self, iso_path: &Path) -> bool {
        match iso_mtime_secs(iso_path) {
            Some(mtime) => self.iso_mtime_secs == mtime,
            None => false,
        }
    }

    /// Update the ISO mtime and clear all results (rebuild detected).
    pub fn reset_for_iso(&mut self, iso_path: &Path) {
        self.iso_mtime_secs = iso_mtime_secs(iso_path).unwrap_or(0);
        self.results.clear();
    }

    /// Record a checkpoint result.
    pub fn record(&mut self, checkpoint: u32, passed: bool, evidence: &str) {
        let now = chrono_now();
        self.results.insert(
            checkpoint,
            CheckpointResult {
                passed,
                timestamp: now,
                evidence: evidence.to_string(),
            },
        );
    }

    /// Check if a checkpoint has already passed.
    pub fn has_passed(&self, checkpoint: u32) -> bool {
        self.results
            .get(&checkpoint)
            .map(|r| r.passed)
            .unwrap_or(false)
    }

    /// Highest checkpoint that passed.
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
    // Find the repo root by looking for .checkpoints/ relative to the workspace
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.checkpoints")
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
