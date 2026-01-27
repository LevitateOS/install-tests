//! QEMU command builder for installation tests.
//!
//! Re-exports from `recqemu` and extends with testing-specific features:
//! - Anti-cheat protections (detect UEFI bypass)
//!
//! Process utilities (kill_stale_qemu_processes, acquire_test_lock) are
//! provided by recqemu::process.

use std::path::PathBuf;
use std::process::{Command, Stdio};

// Re-export basics from recqemu
pub use recqemu::{create_disk, find_ovmf, find_ovmf_vars};

// Re-export process utilities from recqemu
pub use recqemu::process::{acquire_test_lock, kill_stale_qemu_processes};

/// Builder for QEMU commands - extends recqemu with testing features.
///
/// Adds anti-cheat protections that panic if you try to bypass UEFI boot.
#[derive(Default, Clone)]
pub struct QemuBuilder {
    inner: recqemu::QemuBuilder,
    // Testing-specific fields
    has_uefi: bool,
    has_kernel: bool,
}

impl QemuBuilder {
    pub fn new() -> Self {
        Self {
            inner: recqemu::QemuBuilder::new().nodefaults(),
            has_uefi: false,
            has_kernel: false,
        }
    }

    /// Set kernel for direct boot (TESTING ONLY - bypasses bootloader).
    pub fn kernel(mut self, path: PathBuf) -> Self {
        self.has_kernel = true;
        self.inner = self.inner.kernel(path);
        self
    }

    /// Set initrd for direct boot.
    pub fn initrd(mut self, path: PathBuf) -> Self {
        self.inner = self.inner.initrd(path);
        self
    }

    /// Set kernel command line arguments.
    pub fn append(mut self, args: &str) -> Self {
        self.inner = self.inner.append(args);
        self
    }

    /// Set ISO for CD-ROM (exposed as /dev/sr0 via virtio-scsi).
    pub fn cdrom(mut self, path: PathBuf) -> Self {
        self.inner = self.inner.cdrom(path);
        self
    }

    /// Add virtio disk.
    pub fn disk(mut self, path: PathBuf) -> Self {
        self.inner = self.inner.disk(path);
        self
    }

    /// Enable UEFI boot with OVMF firmware.
    pub fn uefi(mut self, ovmf_path: PathBuf) -> Self {
        self.has_uefi = true;
        self.inner = self.inner.uefi(ovmf_path);
        self
    }

    /// Set UEFI variable storage (writable, for boot entries to persist).
    pub fn uefi_vars(mut self, ovmf_vars_path: PathBuf) -> Self {
        self.inner = self.inner.uefi_vars(ovmf_vars_path);
        self
    }

    /// Set boot order (e.g., "dc" = cdrom first, then disk; "c" = disk only).
    pub fn boot_order(mut self, order: &str) -> Self {
        self.inner = self.inner.boot_order(order);
        self
    }

    /// Enable QEMU user-mode networking (provides DHCP, DNS, NAT to guest).
    pub fn with_user_network(mut self) -> Self {
        self.inner = self.inner.user_network();
        self
    }

    /// Disable graphics, use serial console.
    pub fn nographic(mut self) -> Self {
        self.inner = self.inner.nographic();
        self
    }

    /// Don't reboot on exit.
    pub fn no_reboot(mut self) -> Self {
        self.inner = self.inner.no_reboot();
        self
    }

    /// Set QMP Unix socket path for QMP control mode.
    pub fn qmp_socket(mut self, path: PathBuf) -> Self {
        self.inner = self.inner.qmp_socket(path);
        self
    }

    /// Set VNC display number for optional live viewing.
    pub fn vnc_display(mut self, display: u16) -> Self {
        self.inner = self.inner.vnc_display(display);
        self
    }

    /// Build the QEMU command (piped for console control).
    ///
    /// # Panics
    ///
    /// Panics if both `.uefi()` and `.kernel()` are set - this combination
    /// bypasses UEFI firmware while appearing to use it (architectural cheating).
    pub fn build_piped(self) -> Command {
        self.check_anti_cheat();

        let mut cmd = self.inner.build();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        cmd
    }

    /// Build the QEMU command for QMP control mode.
    pub fn build_qmp(self) -> Command {
        self.check_anti_cheat();

        let mut cmd = self.inner.build();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        cmd
    }

    /// Build QEMU command for direct kernel boot debugging (bypasses UEFI/UKI).
    ///
    /// This is for debugging initramfs issues in isolation.
    ///
    /// # Panics
    ///
    /// Panics if `.uefi()` is set (this method is for non-UEFI debug only).
    pub fn build_direct_boot_debug(self) -> Command {
        if self.has_uefi {
            panic!(
                "Direct boot debug cannot use .uefi() - it bypasses UEFI entirely.\n\
                 Remove the .uefi() call to use direct kernel boot."
            );
        }

        let mut cmd = self.inner.build();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        cmd
    }

    /// Check for architectural anti-cheat violations.
    fn check_anti_cheat(&self) {
        if self.has_uefi && self.has_kernel {
            panic!(
                "\n{border}\n\
                ARCHITECTURAL CHEAT BLOCKED\n\
                {border}\n\n\
                Using .uefi() with .kernel() bypasses UEFI firmware entirely.\n\
                The -kernel flag makes QEMU load the kernel directly, skipping:\n\
                  - OVMF firmware execution\n\
                  - Boot entry resolution\n\
                  - systemd-boot loading\n\n\
                To test real UEFI boot:\n\
                  - Remove .kernel() and .initrd()\n\
                  - Use .cdrom() or .disk() with .boot_order()\n\
                  - Let OVMF discover and load the bootloader\n\n\
                {border}\n",
                border = "!".repeat(60)
            );
        }
    }
}
