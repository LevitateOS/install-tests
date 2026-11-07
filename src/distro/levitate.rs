//! LevitateOS: systemd, systemd-boot, glibc.

use super::DistroContext;

pub struct LevitateContext;

impl DistroContext for LevitateContext {
    fn name(&self) -> &str {
        "LevitateOS"
    }

    fn id(&self) -> &str {
        "levitate"
    }

    fn default_hostname(&self) -> &str {
        "levitateos"
    }

    fn uses_initramfs(&self) -> bool {
        true
    }

    fn init_system(&self) -> recab::InitSystem {
        recab::InitSystem::Systemd
    }
}
