//! What each scenario expects of a variant, derived from the scenario, the
//! variant's init system and its identity; no variant declares it.

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

/// What the console says under one init system. The two strictnesses differ
/// as the variants had them (one oracle for both is G26's).
struct InitPatterns {
    /// Fatal on every boot.
    boot_fatal: &'static [&'static str],
    /// Fatal on a live boot only.
    live_fatal: &'static [&'static str],
    /// Besides the login prompt, lines that mean the installed system is up.
    installed_success: &'static [&'static str],
    /// Lines that report a failed service on the installed system: recorded, not fatal.
    installed_service_failures: &'static [&'static str],
}

fn init_patterns(init: InitSystem) -> InitPatterns {
    match init {
        InitSystem::Systemd => InitPatterns {
            boot_fatal: &[
                "SQUASHFS error",
                "emergency shell",
                "Emergency shell",
                "emergency.target",
                "rescue.target",
                "Timed out waiting for device",
            ],
            live_fatal: &["Failed to start sshd.service", "sshd.service: Failed with result"],
            installed_success: &["multi-user.target"],
            installed_service_failures: &["Failed to start", "[FAILED]", "Dependency failed"],
        },
        InitSystem::Openrc => InitPatterns {
            boot_fatal: &[],
            // Any OpenRC service error, sshd's among them.
            live_fatal: &["ERROR: "],
            installed_success: &["login:"],
            installed_service_failures: &["ERROR: cannot start", "* ERROR:", "crashed"],
        },
    }
}

/// What a boot's console output means. The serial test protocol's readiness
/// marker always means success (recqemu adds it).
#[derive(Debug, Clone)]
pub struct BootExpect {
    pub success: Vec<String>,
    pub fatal: Vec<String>,
    pub service_failures: Vec<String>,
}

fn strings(lists: &[&[&str]]) -> Vec<String> {
    lists.iter().flat_map(|list| list.iter()).map(|s| s.to_string()).collect()
}

/// A live ISO's boot: the live init's rootfs-missing message is fatal too.
pub fn live_boot(variant: &Variant) -> BootExpect {
    let init = init_patterns(variant.contract.profile.init);
    let mut fatal = strings(&[BOOT_FATAL, init.boot_fatal, init.live_fatal]);
    fatal.push(recinit::live::ROOTFS_MISSING_MESSAGE.to_string());
    BootExpect {
        success: Vec::new(),
        fatal,
        service_failures: Vec::new(),
    }
}

/// A live-tools boot, whose serial console may run the install screen
/// instead of a shell: a login prompt or the live issue text the live
/// overlay writes also mean it is up.
pub fn live_tools(variant: &Variant) -> BootExpect {
    let mut expect = live_boot(variant);
    expect.success.push(" login:".to_string());
    expect
        .success
        .push(distro_builder::install_ux::LIVE_LOGIN_HINT.to_string());
    expect
}

/// The installed system's boot: its login prompt, or the init system's
/// sign of being up.
pub fn installed_boot(variant: &Variant) -> BootExpect {
    let init = init_patterns(variant.contract.profile.init);
    let identity = &variant.contract.identity;
    let mut success = vec![identity.login_prompt.clone()];
    success.extend(strings(&[init.installed_success]));
    if variant.contract.profile.init == InitSystem::Openrc {
        // AcornOS's installed issue text.
        success.push(format!("Welcome to {}", identity.os_name));
    }
    BootExpect {
        success,
        fatal: strings(&[BOOT_FATAL, init.boot_fatal]),
        service_failures: strings(&[init.installed_service_failures]),
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
