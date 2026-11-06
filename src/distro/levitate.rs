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

    fn enable_serial_getty_cmd(&self) -> String {
        "systemctl enable serial-getty@ttyS0.service".to_string()
    }

    fn install_bootloader_cmd(&self) -> &str {
        // In a chroot bootctl cannot detect the ESP mount or reach EFI
        // variables, so both are given explicitly.
        "bootctl install --esp-path=/boot --no-variables"
    }

    fn default_hostname(&self) -> &str {
        "levitateos"
    }

    fn uses_initramfs(&self) -> bool {
        true
    }
}
