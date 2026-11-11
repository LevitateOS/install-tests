//! Inputs and Recipe probes for the explicit RECAB_P13 integration artifact.
use anyhow::{Result, ensure};
use std::path::Path;
const PROBE: &str = "usr/share/recab-p13";

/// The release data and native origin program already exist on this ISO.
/// The fixture contains no private signing key and no alternative release format.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub schema_version: u32,
    pub release: String,
    pub architecture: distro_contract::release::Architecture,
    pub interrupt_path: String,
    pub updated_path: String,
    pub updated_sha256: String,
    pub conflict_path: String,
}
impl Fixture {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported RECAB_P13 fixture");
        ensure!(
            self.architecture == distro_contract::release::Architecture::X86_64,
            "RECAB_P13 supports x86_64 only"
        );
        distro_contract::release::hash(&self.release)?;
        distro_contract::release::hash(&self.updated_sha256)?;
        for p in [
            &self.updated_path,
            &self.conflict_path,
            &self.interrupt_path,
        ] {
            distro_contract::release::relative(p)?;
        }
        ensure!(
            self.updated_path != self.conflict_path
                && !self.updated_path.starts_with("usr/share/recab-p13"),
            "fixture must distinguish incoming release content from X"
        );
        Ok(())
    }
}

pub(crate) fn probe_recipe(y: bool, release: bool) -> String {
    let needs = if release {
        "inputs:#{release:\"file\"},"
    } else {
        ""
    };
    let binding = if release {
        "RECAB_RELEASE_BASE"
    } else {
        "\"\""
    };
    let verify = if release {
        "verify_sha256(join_path(RUNTIME.paths.inputs,\"release\"),ctx.recab_release);"
    } else {
        ""
    };
    format!(
        r#"const RECIPE_API=2;
const NEEDS=#{{{needs}outputs:#{{system:"directory"}}}};
let ctx=#{{name:"recab-p13-probe",version:"{}",root:RUNTIME.paths.output+"/system",recab_release:{binding}}};
fn is_acquired(ctx){{ready(ctx)}} fn is_built(ctx){{ready(ctx)}}
fn is_installed(ctx){{if !is_file(ctx.root+"/{PROBE}/X"){{return needs_work(ctx,"missing probe");}}ctx.outputs=#{{system:ctx.root}};ready(ctx)}}
fn install(ctx){{{verify}mkdir(ctx.root+"/{PROBE}");let body="a";for n in 0..25{{body=body+body;}}{}write_file(ctx.root+"/{PROBE}/large",body);write_file(ctx.root+"/{PROBE}/X","X\n");{}
target_mode(ctx.root,"{PROBE}/large",0o644);target_finalize(ctx.root);ctx.outputs=#{{system:ctx.root}};ctx}}
"#,
        if y { "2" } else { "1" },
        if y {
            "body=\"Y\"+body.sub_string(1);"
        } else {
            ""
        },
        if y {
            format!("write_file(ctx.root+\"/{PROBE}/Y\",\"Y\\n\");")
        } else {
            format!("write_file(ctx.root+\"/{PROBE}/delete-me\",\"delete through Y\\n\");")
        }
    )
}
pub(crate) fn failure_recipe(init: distro_contract::profile::InitSystem) -> String {
    let (path, body, link, mode) = match init {
        distro_contract::profile::InitSystem::Systemd => (
            "etc/systemd/system/recab-p13-failure.service",
            "[Service]\nType=oneshot\nExecStart=/bin/false\n",
            "etc/systemd/system/multi-user.target.wants/recab-p13-failure.service",
            0o644,
        ),
        distro_contract::profile::InitSystem::Openrc => (
            "etc/init.d/recab-p13-failure",
            "#!/sbin/openrc-run\nstart() { return 1; }\n",
            "etc/runlevels/default/recab-p13-failure",
            0o755,
        ),
    };
    let parent = Path::new(path).parent().unwrap().to_str().unwrap();
    let links = Path::new(link).parent().unwrap().to_str().unwrap();
    format!(
        r#"const RECIPE_API=2;const NEEDS=#{{outputs:#{{system:"directory"}}}};
let ctx=#{{name:"recab-p13-failure",version:"1",root:RUNTIME.paths.output+"/system"}};
fn is_acquired(ctx){{ready(ctx)}} fn is_built(ctx){{ready(ctx)}}
fn is_installed(ctx){{if !is_file(ctx.root+"/{path}"){{return needs_work(ctx,"missing service");}}ctx.outputs=#{{system:ctx.root}};ready(ctx)}}
fn install(ctx){{mkdir(ctx.root+"/{parent}");mkdir(ctx.root+"/{links}");write_file(ctx.root+"/{path}",{});target_mode(ctx.root,"{path}",{mode});ln_force("/{path}",ctx.root+"/{link}");target_finalize(ctx.root);ctx.outputs=#{{system:ctx.root}};ctx}}
"#,
        serde_json::to_string(body).unwrap()
    )
}

/// Emit real Recipe sources used by native and booted lifecycle checks.
pub fn write_recipes(dir: &Path, init: distro_contract::profile::InitSystem) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, source) in [
        ("probe.rhai", probe_recipe(false, false)),
        ("probe-Y.rhai", probe_recipe(true, false)),
        ("probe-release.rhai", probe_recipe(false, true)),
        ("failure.rhai", failure_recipe(init)),
    ] {
        std::fs::write(dir.join(name), source)?;
    }
    Ok(())
}

/// The helper must execute in either libc variant without undeclared libraries.
pub fn check_origin(
    bytes: &[u8],
    architecture: &distro_contract::release::Architecture,
) -> Result<()> {
    ensure!(
        architecture == &distro_contract::release::Architecture::X86_64,
        "RECAB_P13 supports x86_64 only"
    );
    ensure!(
        bytes.len() >= 64
            && bytes.len() <= 64 * 1024 * 1024
            && &bytes[..7] == b"\x7fELF\x02\x01\x01",
        "origin must be a bounded ELF64 little-endian executable"
    );
    ensure!(
        u16::from_le_bytes([bytes[18], bytes[19]]) == architecture.machine(),
        "origin architecture differs from release"
    );
    let offset = usize::try_from(u64::from_le_bytes(bytes[32..40].try_into()?))?;
    let stride = usize::from(u16::from_le_bytes(bytes[54..56].try_into()?));
    let count = usize::from(u16::from_le_bytes(bytes[56..58].try_into()?));
    ensure!(
        stride >= 56 && count > 0 && count <= 128,
        "invalid origin ELF program table"
    );
    for n in 0..count {
        let start = offset
            .checked_add(
                n.checked_mul(stride)
                    .ok_or_else(|| anyhow::anyhow!("origin ELF overflow"))?,
            )
            .ok_or_else(|| anyhow::anyhow!("origin ELF overflow"))?;
        let header = bytes
            .get(
                start
                    ..start
                        .checked_add(56)
                        .ok_or_else(|| anyhow::anyhow!("origin ELF overflow"))?,
            )
            .ok_or_else(|| anyhow::anyhow!("truncated origin ELF"))?;
        ensure!(
            u32::from_le_bytes(header[..4].try_into()?) != 3,
            "origin needs a dynamic loader; build the helper explicitly for the guest's static musl target"
        );
    }
    Ok(())
}
