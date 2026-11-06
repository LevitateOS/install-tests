//! Boot injection: KEY=VALUE settings handed to a live boot through the test
//! channels the live init reads (a QEMU fw_cfg entry, else a small ISO; see
//! `distro_spec::shared::live_boot`).

use anyhow::{anyhow, bail, Context, Result};
use distro_spec::shared::live_boot::{
    BOOT_INJECTION_FILE, BOOT_INJECTION_FW_CFG, BOOT_INJECTION_ISO_LABEL,
};
use recqemu::QemuBuilder;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};


/// Environment variables through which the scenarios binary hands an
/// injection to the scenario runs: a payload file, or `KEY=VALUE,...` pairs.
pub const ENV_INJECT_FILE: &str = "LEVITATE_BOOT_INJECTION_FILE";
pub const ENV_INJECT_KV: &str = "LEVITATE_BOOT_INJECTION_KV";

/// A payload file and the ISO carrying it; temporary files are removed on drop.
#[derive(Debug)]
pub struct BootInjection {
    pub payload_file: PathBuf,
    pub media_iso_file: PathBuf,
    owns_payload: bool,
}

impl Drop for BootInjection {
    fn drop(&mut self) {
        if self.owns_payload {
            let _ = std::fs::remove_file(&self.payload_file);
        }
        let _ = std::fs::remove_file(&self.media_iso_file);
    }
}

impl BootInjection {
    /// Hand the payload to the guest on both channels the live init reads.
    pub fn attach(&self, builder: QemuBuilder) -> QemuBuilder {
        builder
            .fw_cfg_file(BOOT_INJECTION_FW_CFG, &self.payload_file)
            .extra_cdrom(&self.media_iso_file)
    }

    /// Inject an existing payload file (KEY=VALUE lines).
    pub fn from_file(path: &Path) -> Result<Self> {
        if !path.is_file() {
            bail!("boot injection payload '{}' is not a file", path.display());
        }
        Ok(Self {
            payload_file: path.to_path_buf(),
            media_iso_file: create_boot_injection_iso(path)?,
            owns_payload: false,
        })
    }

    /// Inject `KEY=VALUE` pairs separated by commas; `None` when there are none.
    pub fn from_pairs(raw: &str) -> Result<Option<Self>> {
        let mut lines = Vec::new();
        for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some((key, _)) if !key.trim().is_empty() => lines.push(part.to_string()),
                _ => bail!("invalid boot injection entry '{part}', expected KEY=VALUE"),
            }
        }
        if lines.is_empty() {
            return Ok(None);
        }
        let path = temp_path("env")?;
        std::fs::write(&path, format!("{}\n", lines.join("\n")))
            .with_context(|| format!("writing boot injection payload '{}'", path.display()))?;
        let media_iso_file = match create_boot_injection_iso(&path) {
            Ok(iso) => iso,
            Err(err) => {
                let _ = std::fs::remove_file(&path);
                return Err(err);
            }
        };
        Ok(Some(Self {
            payload_file: path,
            media_iso_file,
            owns_payload: true,
        }))
    }
}

/// The injection the scenarios binary handed over (file wins over pairs).
pub fn boot_injection_from_env() -> Result<Option<BootInjection>> {
    if let Ok(path) = std::env::var(ENV_INJECT_FILE) {
        return BootInjection::from_file(Path::new(&path)).map(Some);
    }
    match std::env::var(ENV_INJECT_KV) {
        Ok(raw) => BootInjection::from_pairs(&raw),
        Err(_) => Ok(None),
    }
}

fn temp_path(extension: &str) -> Result<PathBuf> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX_EPOCH")?
        .as_nanos();
    Ok(std::env::temp_dir().join(format!(
        "levitate-boot-injection-{}-{ts}.{extension}",
        std::process::id()
    )))
}

/// An ISO whose root holds the payload as `BOOT_INJECTION_FILE`, built with
/// the first ISO tool found.
fn create_boot_injection_iso(payload_path: &Path) -> Result<PathBuf> {
    let iso_path = temp_path("iso")?;
    let graft = format!("{BOOT_INJECTION_FILE}={}", payload_path.display());
    let mut tried = Vec::new();
    for (tool, prefix) in [
        ("xorriso", &["-as", "mkisofs"][..]),
        ("genisoimage", &[][..]),
        ("mkisofs", &[][..]),
    ] {
        let status = Command::new(tool)
            .args(prefix)
            .args(["-quiet", "-V", BOOT_INJECTION_ISO_LABEL, "-o"])
            .arg(&iso_path)
            .arg("-graft-points")
            .arg(&graft)
            .status();
        match status {
            Ok(status) if status.success() => return Ok(iso_path),
            Ok(status) => tried.push(format!("{tool} exited with status {status}")),
            Err(err) => tried.push(format!("{tool} unavailable: {err}")),
        }
    }
    Err(anyhow!(
        "failed to build boot-injection ISO from '{}': {}",
        payload_path.display(),
        tried.join("; ")
    ))
}
