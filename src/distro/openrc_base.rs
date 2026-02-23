//! Shared OpenRC base for AcornOS and IuppiterOS.
//!
//! Both distros use OpenRC + systemd-boot + musl + busybox + ash.
//! This struct provides all identical methods; variant structs delegate here.

/// Shared OpenRC methods. Compose into AcornContext/IuppiterContext.
pub struct OpenRcBase;

impl OpenRcBase {
    pub fn live_boot_stall_timeout_secs(&self) -> u64 {
        180
    }

    pub fn boot_error_patterns(&self) -> &[&str] {
        &[
            // === UEFI STAGE ===
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            // === BOOTLOADER STAGE ===
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            // === KERNEL STAGE ===
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "EROFS:",
            // === OPENRC INIT STAGE ===
            "ERROR: cannot start",
            "Rootfs payload partition not found",
            "ERROR: ",
            // === GENERAL ===
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    pub fn critical_boot_errors(&self) -> &[&str] {
        &[
            // === UEFI STAGE ===
            "No bootable device",
            "Boot Failed",
            "Default Boot Device Missing",
            "Shell>",
            "ASSERT_EFI_ERROR",
            "map: Cannot find",
            // === BOOTLOADER STAGE ===
            "systemd-boot: Failed",
            "loader: Failed",
            "vmlinuz: not found",
            "initramfs: not found",
            "Error loading",
            "File not found",
            // === KERNEL STAGE ===
            "Kernel panic",
            "not syncing",
            "VFS: Cannot open root device",
            "No init found",
            "Attempted to kill init",
            "can't find /init",
            "No root device",
            "EROFS:",
            "Rootfs payload partition not found",
            // === GENERAL ===
            "fatal error",
            "Segmentation fault",
            "core dumped",
        ]
    }

    pub fn service_failure_patterns(&self) -> &[&str] {
        &["ERROR: cannot start", "* ERROR:", "crashed"]
    }

    pub fn enable_service_cmd(&self, service: &str, runlevel: &str) -> String {
        format!("rc-update add {} {}", service, runlevel)
    }

    pub fn check_service_exists_cmd(&self, service: &str) -> String {
        format!("test -f /etc/init.d/{} && echo {}", service, service)
    }

    pub fn check_service_status_cmd(&self, service: &str) -> String {
        format!("rc-service {} status", service)
    }

    pub fn list_failed_services_cmd(&self) -> String {
        "rc-status --crashed 2>/dev/null || rc-status -a | grep -E 'stopped|crashed'".to_string()
    }

    pub fn enable_serial_getty_cmd(&self) -> String {
        "grep -q 'ttyS0' /etc/inittab || echo 'ttyS0::respawn:/sbin/getty -L 115200 ttyS0 vt100' >> /etc/inittab".to_string()
    }

    pub fn expected_pid1_name(&self) -> &str {
        "init"
    }

    pub fn check_target_reached_cmd(&self) -> &str {
        "rc-status default 2>/dev/null | grep -q started && echo 'default_reached'"
    }

    pub fn target_reached_expected(&self) -> &str {
        "default_reached"
    }

    pub fn count_failed_services_cmd(&self) -> &str {
        "rc-status --crashed 2>/dev/null | wc -l || echo 0"
    }

    pub fn check_network_service_cmd(&self) -> &str {
        "rc-service networking status 2>/dev/null | grep -q started && echo 'active'"
    }

    pub fn install_bootloader_cmd(&self) -> &str {
        "bootctl install --esp-path=/boot --no-variables"
    }

    pub fn chroot_shell(&self) -> &str {
        "/bin/ash"
    }

    pub fn init_system_name(&self) -> &str {
        "OpenRC"
    }

    pub fn boot_target_name(&self) -> &str {
        "default runlevel"
    }
}
