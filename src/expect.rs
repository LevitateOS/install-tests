//! What each scenario expects of a variant, derived from the scenario, the
//! variant's init system and its identity; no variant declares it. Both
//! variants are judged by the same oracle.

use distro_contract::profile::InitSystem;
use distro_contract::{ScenarioId, Variant};

/// Console lines that mean a boot failed, on every variant: firmware, boot
/// loader, kernel and the processes it starts.
const BOOT_FATAL: &[&str] = &[
    "No bootable device",
    "Boot Failed",
    "Default Boot Device Missing",
    "Shell>",
    "ASSERT_EFI_ERROR",
    "map: Cannot find",
    "systemd-boot: Failed",
    "loader: Failed",
    "vmlinuz: not found",
    "initramfs: not found",
    "Error loading",
    "File not found",
    "Kernel panic",
    "not syncing",
    "VFS: Cannot open root device",
    "No init found",
    "Attempted to kill init",
    "can't find /init",
    "No root device",
    "EROFS:",
    "fatal error",
    "Segmentation fault",
    "core dumped",
];

/// Console lines that mean a boot failed under one init system: its own
/// rescue paths. Service failures are not judged on the console: the
/// installed system's init is asked for them after login
/// (`test-scripts/lib/init/*.sh`, `svc_failed`), the same on every variant.
fn init_fatal(init: InitSystem) -> &'static [&'static str] {
    match init {
        InitSystem::Systemd => &[
            "SQUASHFS error",
            "emergency shell",
            "Emergency shell",
            "emergency.target",
            "rescue.target",
            "Timed out waiting for device",
        ],
        InitSystem::Openrc => &[],
    }
}

/// What a boot's console output means.
#[derive(Debug, Clone)]
pub struct BootExpect {
    pub success: Vec<String>,
    pub fatal: Vec<String>,
}

fn fatal(variant: &Variant) -> Vec<String> {
    BOOT_FATAL
        .iter()
        .chain(init_fatal(variant.contract.profile.init))
        .map(|s| s.to_string())
        .collect()
}

/// A live ISO's boot: done when the live shell prints the serial test
/// protocol's readiness marker (scenario boots never run the install screen
/// on the serial console); the live init's rootfs-missing message is fatal.
pub fn live_boot(variant: &Variant) -> BootExpect {
    let mut fatal = fatal(variant);
    fatal.push(recinit::live::ROOTFS_MISSING_MESSAGE.to_string());
    BootExpect {
        success: vec![recqemu::serial::protocol::SHELL_READY_MARKER.to_string()],
        fatal,
    }
}

/// The installed system's boot: done at its getty's login prompt, which
/// names the hostname the install sets (`identity.default_hostname`).
pub fn installed_boot(variant: &Variant) -> BootExpect {
    BootExpect {
        success: vec![variant.contract.identity.login_prompt.clone()],
        fatal: fatal(variant),
    }
}

/// A scenario's guest-side script under `test-scripts/` and the marker it
/// prints when it passes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub script: String,
    pub pass_marker: String,
}

impl Evidence {
    /// `<name>.sh`, which prints `<NAME IN CAPITALS> PASSED`.
    pub fn named(name: &str) -> Self {
        Self {
            script: format!("{name}.sh"),
            pass_marker: format!("{} PASSED", name.replace('-', " ").to_ascii_uppercase()),
        }
    }

    /// The scenario's own script, `<key>.sh`, for the scenarios that run one.
    pub fn of(scenario: ScenarioId) -> Option<Self> {
        match scenario {
            ScenarioId::BuildPreflight | ScenarioId::AbUpdate => None,
            ScenarioId::LiveBoot
            | ScenarioId::LiveTools
            | ScenarioId::Install
            | ScenarioId::InstalledBoot
            | ScenarioId::AutomatedLogin
            | ScenarioId::Runtime => Some(Self::named(scenario.key())),
        }
    }
}

/// The root password the install scenario sets and the installed-system
/// scenarios log in with: the variant id.
pub fn root_password(variant: &Variant) -> &str {
    &variant.id
}
