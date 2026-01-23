//! QEMU command builder for installation tests.
//!
//! Adapted from leviso's qemu.rs with additions for installation testing.

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
    nographic: bool,
    no_reboot: bool,
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

    /// Build the QEMU command (piped for console control)
    pub fn build_piped(self) -> Command {
        let mut cmd = self.build_base();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        cmd
    }

    fn build_base(self) -> Command {
        let mut cmd = Command::new("qemu-system-x86_64");

        // Start with no default devices to avoid conflicts with explicit drive definitions
        // (QEMU 10+ has stricter drive index handling)
        cmd.arg("-nodefaults");

        // CPU: Skylake-Client for x86-64-v3 support required by Rocky 10
        cmd.args(["-cpu", "Skylake-Client"]);

        // Memory: 2G for installation - don't use toy values
        cmd.args(["-m", "2G"]);

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

        // Display options
        if self.nographic {
            cmd.args(["-nographic", "-serial", "mon:stdio"]);
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
