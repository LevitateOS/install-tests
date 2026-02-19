use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Canonical fw_cfg path consumed by initramfs.
pub const FW_CFG_NAME: &str = "opt/levitate/boot-injection";

const ENV_INJECT_FILE: &str = "LEVITATE_BOOT_INJECTION_FILE";
const ENV_INJECT_KV: &str = "LEVITATE_BOOT_INJECTION_KV";

#[derive(Debug, Clone)]
pub struct BootInjection {
    pub fw_cfg_name: String,
    pub payload_file: PathBuf,
}

/// Parse a boot injection spec from environment variables.
///
/// - `LEVITATE_BOOT_INJECTION_FILE=/abs/path/to/payload.env`
/// - `LEVITATE_BOOT_INJECTION_KV=KEY=VALUE,FOO=BAR`
///
/// If both are present, `..._FILE` wins.
pub fn boot_injection_from_env() -> Result<Option<BootInjection>> {
    if let Ok(path) = std::env::var(ENV_INJECT_FILE) {
        let payload = PathBuf::from(path);
        if !payload.is_file() {
            return Err(anyhow!(
                "{} points to non-file '{}'",
                ENV_INJECT_FILE,
                payload.display()
            ));
        }
        return Ok(Some(BootInjection {
            fw_cfg_name: FW_CFG_NAME.to_string(),
            payload_file: payload,
        }));
    }

    let raw = match std::env::var(ENV_INJECT_KV) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Ok(None),
    };

    let entries = parse_kv_csv(&raw)?;
    let payload = write_env_payload_file(&entries)?;
    Ok(Some(BootInjection {
        fw_cfg_name: FW_CFG_NAME.to_string(),
        payload_file: payload,
    }))
}

fn parse_kv_csv(raw: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid key/value '{}', expected KEY=VALUE", part))?;
        let key = k.trim();
        if key.is_empty() {
            return Err(anyhow!("empty key in '{}'", part));
        }
        out.push((key.to_string(), v.to_string()));
    }
    if out.is_empty() {
        return Err(anyhow!("no key/value pairs found in {}", ENV_INJECT_KV));
    }
    Ok(out)
}

fn write_env_payload_file(entries: &[(String, String)]) -> Result<PathBuf> {
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before UNIX_EPOCH")?
        .as_millis();
    let path = std::env::temp_dir().join(format!("levitate-boot-injection-{pid}-{ts}.env"));
    write_env_payload_path(&path, entries)?;
    Ok(path)
}

fn write_env_payload_path(path: &Path, entries: &[(String, String)]) -> Result<()> {
    let mut lines = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        lines.push(format!("{k}={v}"));
    }
    std::fs::write(path, format!("{}\n", lines.join("\n")))
        .with_context(|| format!("writing boot injection payload '{}'", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kv_csv() {
        let pairs = parse_kv_csv("A=1,B=two words").expect("parse");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("A".to_string(), "1".to_string()));
        assert_eq!(pairs[1], ("B".to_string(), "two words".to_string()));
    }
}
