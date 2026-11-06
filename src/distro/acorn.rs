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

    fn enable_serial_getty_cmd(&self) -> String {
        "grep -q 'ttyS0' /etc/inittab || echo 'ttyS0::respawn:/sbin/getty -L 115200 ttyS0 vt100' >> /etc/inittab".to_string()
    }

    /// No bootctl without systemd: copy systemd-boot into place.
    fn install_bootloader_cmd(&self) -> &str {
        "sh -c 'set -eu; mkdir -p /boot/EFI/BOOT /boot/EFI/systemd; cp /usr/lib/systemd/boot/efi/systemd-bootx64.efi /boot/EFI/BOOT/BOOTX64.EFI; cp /usr/lib/systemd/boot/efi/systemd-bootx64.efi /boot/EFI/systemd/systemd-bootx64.efi'"
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
