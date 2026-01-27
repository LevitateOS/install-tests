//! QEMU command builder for installation tests.
//!
//! Adapted from leviso's qemu.rs with additions for installation testing.
//! Supports both serial console (piped) and QMP (socket) control modes.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Builder for QEMU commands - consolidates common configuration patterns.
#[derive(Default)]
pub struct QemuBuilder {
    kernel: Option<PathBuf>,
    initrd: Option<PathBuf>,
    append: Option<String>,
    cdrom: Option<PathBuf>,
    disk: Option<PathBuf>,
    ovmf: Option<PathBuf>,
    ovmf_vars: Option<PathBuf>, // UEFI variable storage (writable)
    boot_order: Option<String>, // BIOS/UEFI boot order (e.g., "dc" = cdrom first, then disk)
    user_network: bool,         // Enable QEMU user-mode network (for IP address testing)
    nographic: bool,
    no_reboot: bool,
    // QMP support
    qmp_socket: Option<PathBuf>, // QMP Unix socket path
    vnc_display: Option<u16>,    // VNC display number (optional, for live viewing)
}

impl QemuBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set kernel for direct boot
    pub fn kernel(mut self, path: PathBuf) -> Self {
        self.kernel = Some(path);
        self
    }

    /// Set initrd for direct boot
    pub fn initrd(mut self, path: PathBuf) -> Self {
        self.initrd = Some(path);
        self
    }

    /// Set kernel command line arguments
    pub fn append(mut self, args: &str) -> Self {
        self.append = Some(args.to_string());
        self
    }

    /// Set ISO for CD-ROM (exposed as /dev/sr0 via virtio-scsi)
    pub fn cdrom(mut self, path: PathBuf) -> Self {
        self.cdrom = Some(path);
        self
    }

    /// Add virtio disk
    pub fn disk(mut self, path: PathBuf) -> Self {
        self.disk = Some(path);
        self
    }

    /// Enable UEFI boot with OVMF firmware
    pub fn uefi(mut self, ovmf_path: PathBuf) -> Self {
        self.ovmf = Some(ovmf_path);
        self
    }

    /// Set UEFI variable storage (writable, for boot entries to persist)
    pub fn uefi_vars(mut self, ovmf_vars_path: PathBuf) -> Self {
        self.ovmf_vars = Some(ovmf_vars_path);
        self
    }

    /// Set boot order (e.g., "dc" = cdrom first, then disk; "c" = disk only)
    pub fn boot_order(mut self, order: &str) -> Self {
        self.boot_order = Some(order.to_string());
        self
    }

    /// Enable QEMU user-mode networking (provides DHCP, DNS, NAT to guest)
    pub fn with_user_network(mut self) -> Self {
        self.user_network = true;
        self
    }

    /// Disable graphics, use serial console
    pub fn nographic(mut self) -> Self {
        self.nographic = true;
        self
    }

    /// Don't reboot on exit
    pub fn no_reboot(mut self) -> Self {
        self.no_reboot = true;
        self
    }

    /// Set QMP Unix socket path for QMP control mode.
    ///
    /// When set, QEMU will listen on this socket for QMP commands.
    /// Use with `build_qmp()` instead of `build_piped()`.
    pub fn qmp_socket(mut self, path: PathBuf) -> Self {
        self.qmp_socket = Some(path);
        self
    }

    /// Set VNC display number for optional live viewing.
    ///
    /// Display 0 = port 5900, display 1 = port 5901, etc.
    /// Only useful with QMP mode for visual testing.
    pub fn vnc_display(mut self, display: u16) -> Self {
        self.vnc_display = Some(display);
        self
    }

    /// Build the QEMU command (piped for console control)
    ///
    /// # Panics
    ///
    /// Panics if both `.uefi()` and `.kernel()` are set - this combination
    /// bypasses UEFI firmware while appearing to use it (architectural cheating).
    pub fn build_piped(self) -> Command {
        // ARCHITECTURAL ANTI-CHEAT: Detect invalid combinations that bypass UEFI
        if self.ovmf.is_some() && self.kernel.is_some() {
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

        let mut cmd = self.build_base(false);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        cmd
    }

    /// Build the QEMU command for QMP control mode.
    ///
    /// Unlike `build_piped()`, this configures QEMU for QMP control:
    /// - QMP socket for sending commands
    /// - Optional VNC for screenshot capture
    /// - Serial log to file instead of stdio
    ///
    /// # Panics
    ///
    /// Panics if `qmp_socket()` was not called.
    pub fn build_qmp(self) -> Command {
        if self.qmp_socket.is_none() {
            panic!("QMP mode requires qmp_socket() to be set");
        }

        // ARCHITECTURAL ANTI-CHEAT: Detect invalid combinations that bypass UEFI
        if self.ovmf.is_some() && self.kernel.is_some() {
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

        let mut cmd = self.build_base(true);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        cmd
    }

    /// Build QEMU command for direct kernel boot debugging (bypasses UEFI/UKI).
    ///
    /// This is for debugging initramfs issues in isolation. It boots the kernel
    /// and initramfs directly via QEMU's -kernel/-initrd flags, completely
    /// bypassing OVMF, systemd-boot, and UKI loading.
    ///
    /// Use this to verify the initramfs works independently of UKI packaging.
    ///
    /// # Panics
    ///
    /// Panics if `.kernel()` or `.initrd()` or `.append()` are not set.
    /// Panics if `.uefi()` is set (this method is for non-UEFI debug only).
    pub fn build_direct_boot_debug(self) -> Command {
        if self.kernel.is_none() {
            panic!("Direct boot debug requires .kernel() to be set");
        }
        if self.initrd.is_none() {
            panic!("Direct boot debug requires .initrd() to be set");
        }
        if self.append.is_none() {
            panic!("Direct boot debug requires .append() to be set");
        }
        if self.ovmf.is_some() {
            panic!(
                "Direct boot debug cannot use .uefi() - it bypasses UEFI entirely.\n\
                 Remove the .uefi() call to use direct kernel boot."
            );
        }

        let mut cmd = self.build_base(false);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        cmd
    }

    fn build_base(self, qmp_mode: bool) -> Command {
        let mut cmd = Command::new("qemu-system-x86_64");

        // Enable KVM if available for faster tests
        // This is a performance optimization, not a workaround for bugs
        if std::path::Path::new("/dev/kvm").exists() {
            cmd.arg("-enable-kvm");
        }

        // Start with no default devices to avoid conflicts with explicit drive definitions
        // (QEMU 10+ has stricter drive index handling)
        cmd.arg("-nodefaults");

        // CPU: Skylake-Client for x86-64-v3 support required by Rocky 10
        cmd.args(["-cpu", "Skylake-Client"]);

        // Memory: 4G for installation - don't use toy values
        // Increased from 2G to match production requirements
        cmd.args(["-m", "4G"]);

        // Direct kernel boot
        if let Some(kernel) = &self.kernel {
            cmd.args(["-kernel", kernel.to_str().unwrap()]);
        }
        if let Some(initrd) = &self.initrd {
            cmd.args(["-initrd", initrd.to_str().unwrap()]);
        }
        if let Some(append) = &self.append {
            cmd.args(["-append", append]);
        }

        // CD-ROM (use virtio-scsi for better compatibility with modern kernels)
        if let Some(cdrom) = &self.cdrom {
            // Add virtio-scsi controller and attach CD-ROM as SCSI device
            cmd.args([
                "-device", "virtio-scsi-pci,id=scsi0",
                "-device", "scsi-cd,drive=cdrom0,bus=scsi0.0",
                "-drive", &format!("id=cdrom0,if=none,format=raw,readonly=on,file={}", cdrom.display()),
            ]);
        }

        // Virtio disk
        if let Some(disk) = &self.disk {
            cmd.args([
                "-drive",
                &format!("file={},format=qcow2,if=virtio", disk.display()),
            ]);
        }

        // UEFI firmware (CODE is read-only, VARS is writable for boot entries)
        if let Some(ovmf) = &self.ovmf {
            cmd.args([
                "-drive",
                &format!("if=pflash,format=raw,readonly=on,file={}", ovmf.display()),
            ]);
        }
        if let Some(ovmf_vars) = &self.ovmf_vars {
            cmd.args([
                "-drive",
                &format!("if=pflash,format=raw,file={}", ovmf_vars.display()),
            ]);
        }

        // Boot order (e.g., "dc" = cdrom first, then disk)
        if let Some(order) = &self.boot_order {
            cmd.args(["-boot", order]);
        }

        // User-mode networking (provides DHCP, DNS, NAT to guest)
        if self.user_network {
            cmd.args(["-netdev", "user,id=net0"]);
            cmd.args(["-device", "virtio-net-pci,netdev=net0"]);
        }

        // Display and control options depend on mode
        if qmp_mode {
            // QMP mode: use socket for control, optionally VNC for viewing
            if let Some(socket) = &self.qmp_socket {
                cmd.args([
                    "-qmp",
                    &format!("unix:{},server,nowait", socket.display()),
                ]);
            }

            if let Some(display) = self.vnc_display {
                // VNC for optional live viewing/screenshots
                cmd.args(["-vnc", &format!(":{}", display)]);
            } else {
                // No display
                cmd.arg("-display");
                cmd.arg("none");
            }

            // Serial to file for debugging (optional)
            cmd.args(["-serial", "file:/tmp/qemu-serial.log"]);
        } else {
            // Serial mode: nographic with stdio
            if self.nographic {
                cmd.args(["-nographic", "-serial", "mon:stdio"]);
            }
        }

        // Reboot behavior
        if self.no_reboot {
            cmd.arg("-no-reboot");
        }

        cmd
    }
}

/// Find OVMF firmware for UEFI boot
pub fn find_ovmf() -> Option<PathBuf> {
    // Common OVMF locations across distros
    let candidates = [
        // Fedora/RHEL
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/OVMF/OVMF_CODE.fd",
        // Debian/Ubuntu
        "/usr/share/OVMF/OVMF_CODE_4M.fd",
        "/usr/share/qemu/OVMF.fd",
        // Arch
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
        // NixOS
        "/run/libvirt/nix-ovmf/OVMF_CODE.fd",
    ];

    for path in candidates {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Find OVMF variable storage template
pub fn find_ovmf_vars() -> Option<PathBuf> {
    // Common OVMF_VARS locations across distros
    let candidates = [
        // Fedora/RHEL
        "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        "/usr/share/OVMF/OVMF_VARS.fd",
        // Debian/Ubuntu
        "/usr/share/OVMF/OVMF_VARS_4M.fd",
        "/usr/share/qemu/OVMF_VARS.fd",
        // Arch
        "/usr/share/edk2-ovmf/x64/OVMF_VARS.fd",
        // NixOS
        "/run/libvirt/nix-ovmf/OVMF_VARS.fd",
    ];

    for path in candidates {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Create a fresh qcow2 disk image
pub fn create_disk(path: &std::path::Path, size: &str) -> anyhow::Result<()> {
    let status = Command::new("qemu-img")
        .args(["create", "-f", "qcow2", path.to_str().unwrap(), size])
        .status()?;

    if !status.success() {
        anyhow::bail!("qemu-img create failed");
    }
    Ok(())
}

/// Kill any stale QEMU processes from previous test runs.
///
/// This prevents memory leaks from zombie QEMU instances that weren't properly cleaned up.
/// Called before spawning new QEMU to ensure a clean state.
pub fn kill_stale_qemu_processes() {
    // Find QEMU processes that match our test patterns
    let patterns = [
        "leviso-install-test.qcow2",
        "boot-hypothesis-test.qcow2",
        "levitateos.iso",
    ];

    // Use pkill to kill matching processes
    for pattern in patterns {
        let _ = Command::new("pkill")
            .args(["-9", "-f", &format!("qemu-system-x86_64.*{}", pattern)])
            .status();
    }

    // Give processes time to die
    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// Acquire an exclusive lock for QEMU tests.
///
/// Returns a file handle that must be kept alive for the duration of the test.
/// When dropped, the lock is released.
pub fn acquire_test_lock() -> anyhow::Result<std::fs::File> {
    use std::fs::OpenOptions;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let lock_path = std::path::Path::new("/tmp/leviso-install-test.lock");

    #[cfg(unix)]
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o644)
        .open(lock_path)?;

    #[cfg(not(unix))]
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(lock_path)?;

    // Try to acquire exclusive lock (non-blocking)
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();

        // LOCK_EX | LOCK_NB = exclusive, non-blocking
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        if result != 0 {
            anyhow::bail!(
                "Another install-test is already running. \
                 Kill it with: pkill -9 -f 'qemu-system-x86_64.*leviso'"
            );
        }
    }

    Ok(file)
}
