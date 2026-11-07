//! AcornOS: OpenRC, systemd-boot, musl and busybox.

use super::DistroContext;

pub struct AcornContext;

impl DistroContext for AcornContext {
    fn name(&self) -> &str {
        "AcornOS"
    }

    fn id(&self) -> &str {
        "acorn"
    }

    /// OpenRC's early boot can stay quiet for longer than systemd's.
    fn live_boot_stall_timeout_secs(&self) -> u64 {
        180
    }

    fn default_hostname(&self) -> &str {
        "acornos"
    }

    /// AcornOS boots the installed kernel without an initramfs.
    fn uses_initramfs(&self) -> bool {
        false
    }

    fn init_system(&self) -> recab::InitSystem {
        recab::InitSystem::OpenRc
    }
}
