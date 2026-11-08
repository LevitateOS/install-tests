//! Boot injection: KEY=VALUE settings handed to a live boot. The live init
//! reads them from a QEMU fw_cfg entry or from a small ISO
//! (`recinit::live`); the harness uses the ISO, because neither variant's
//! kernel has `qemu_fw_cfg` (`CONFIG_FW_CFG_SYSFS`), so the fw_cfg entry
//! would never be read.

use anyhow::{bail, Context, Result};
use recinit::live::{BOOT_INJECTION_FILE, BOOT_INJECTION_ISO_LABEL};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The key whose value the live init appends to root's `authorized_keys`
/// (`tools/recinit/templates/init_tiny.template`).
pub const SSH_AUTHORIZED_KEY: &str = "SSH_AUTHORIZED_KEY";

/// `KEY=VALUE` pairs separated by commas, as `--inject` takes them.
pub fn parse_pairs(raw: &str) -> Result<Vec<String>> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((key, _)) if !key.trim().is_empty() => Ok(part.to_string()),
            _ => bail!("invalid boot injection entry '{part}', expected KEY=VALUE"),
        })
        .collect()
}

/// A payload and the ISO that carries it, in a temporary directory that goes
/// with this value.
#[derive(Debug)]
pub struct BootInjection {
    dir: tempfile::TempDir,
}

impl BootInjection {
    /// Build the injection medium for `lines` (each `KEY=VALUE`).
    pub fn new(lines: &[String]) -> Result<Self> {
        for line in lines {
            parse_pairs(line)?;
        }
        let dir = tempfile::Builder::new()
            .prefix("scenario-boot-injection-")
            .tempdir()
            .context("creating the boot injection directory")?;
        let payload = dir.path().join(BOOT_INJECTION_FILE);
        std::fs::write(&payload, format!("{}\n", lines.join("\n")))
            .with_context(|| format!("writing boot injection payload '{}'", payload.display()))?;
        let injection = Self { dir };
        let status = Command::new("xorriso")
            .args(["-as", "mkisofs", "-quiet", "-V", BOOT_INJECTION_ISO_LABEL, "-o"])
            .arg(injection.iso())
            .arg("-graft-points")
            .arg(format!("{BOOT_INJECTION_FILE}={}", payload.display()))
            .status()
            .context("running xorriso to build the boot injection ISO")?;
        if !status.success() {
            bail!("xorriso could not build the boot injection ISO ({status})");
        }
        Ok(injection)
    }

    /// The ISO to attach as a CD.
    pub fn iso(&self) -> PathBuf {
        self.dir.path().join("boot-injection.iso")
    }

    /// Remove the medium, reporting a failure.
    pub fn close(self) -> Result<()> {
        let path: &Path = self.dir.path();
        let shown = path.display().to_string();
        self.dir
            .close()
            .with_context(|| format!("removing the boot injection directory '{shown}'"))
    }
}
