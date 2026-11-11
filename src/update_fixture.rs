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

pub(crate) fn profile_recipe() -> &'static str {
    r#"const RECIPE_API=2;
const NEEDS=#{outputs:#{"package":"directory"}};
let ctx=#{name:"recab-p13-user",version:"1",root:RUNTIME.paths.output+"/package"};
fn is_acquired(ctx){ready(ctx)} fn is_built(ctx){ready(ctx)}
fn is_installed(ctx){if !is_file(ctx.root+"/share/recab-p13-user.txt"){return needs_work(ctx,"missing profile");}ctx.outputs=#{"package":ctx.root};ready(ctx)}
fn install(ctx){mkdir(ctx.root+"/share");write_file(ctx.root+"/share/recab-p13-user.txt","independent user profile\n");ctx.outputs=#{"package":ctx.root};ctx}
"#
}

/// Identical file bodies with deliberately changed or removed target metadata.
pub(crate) fn metadata_recipe(revision: u8, release: bool) -> String {
    let needs = if release {
        r#"inputs:#{release:"file"},"#
    } else {
        ""
    };
    let binding = if release {
        "RECAB_RELEASE_BASE"
    } else {
        r#""""#
    };
    let verify = if release {
        r#"verify_sha256(join_path(RUNTIME.paths.inputs,"release"),ctx.recab_release);"#
    } else {
        ""
    };
    let (uid, gid, mode) = if revision == 2 {
        (1003, 1004, 0o644)
    } else {
        (1001, 1002, 0o640)
    };
    let mut xattrs = serde_json::Map::new();
    if revision != 3 {
        let hex = |b: Vec<u8>| b.iter().map(|v| format!("{v:02x}")).collect::<String>();
        let named = if revision == 2 { 1005u32 } else { 1003 };
        let mut acl = 2u32.to_le_bytes().to_vec();
        for (tag, permission, id) in [
            (1u16, 6u16, u32::MAX),
            (2, 4, named),
            (4, 4, u32::MAX),
            (16, 4, u32::MAX),
            (32, if revision == 2 { 4 } else { 0 }, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(permission.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        xattrs.insert("system.posix_acl_access".into(), hex(acl).into());
        xattrs.insert("user.recab-p13".into(), hex(vec![revision]).into());
        let bit = if revision == 2 { 13 } else { 10 };
        let cap = [0x02000001u32, 1u32 << bit, 0, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        xattrs.insert("security.capability".into(), hex(cap).into());
    }
    let metadata = serde_json::json!({"version":1,"entries":{"usr/share/recab-p13-metadata/file":{"kind":"file","uid":uid,"gid":gid,"mode":mode,"xattrs":xattrs}}});
    format!(
        r#"const RECIPE_API=2;
const NEEDS=#{{{needs}outputs:#{{system:"directory"}}}};
let ctx=#{{name:"recab-p13-metadata",version:"{revision}",root:RUNTIME.paths.output+"/system",recab_release:{binding}}};
fn is_acquired(ctx){{ready(ctx)}} fn is_built(ctx){{ready(ctx)}}
fn is_installed(ctx){{if !is_file(ctx.root+"/usr/share/recab-p13-metadata/file"){{return needs_work(ctx,"missing metadata file");}}ctx.outputs=#{{system:ctx.root}};ready(ctx)}}
fn install(ctx){{{verify}mkdir(ctx.root+"/usr/share/recab-p13-metadata");write_file(ctx.root+"/usr/share/recab-p13-metadata/file","unchanged metadata probe\n");write_file(ctx.root+"/.recipe-target-metadata.json",{});target_finalize(ctx.root);ctx.outputs=#{{system:ctx.root}};ctx}}
"#,
        serde_json::to_string(&metadata.to_string()).unwrap()
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
        ("user-profile.rhai", profile_recipe().into()),
        ("metadata-1.rhai", metadata_recipe(1, false)),
        ("metadata-2.rhai", metadata_recipe(2, false)),
        ("metadata-3.rhai", metadata_recipe(3, false)),
        ("metadata-release.rhai", metadata_recipe(2, true)),
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
