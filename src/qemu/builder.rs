//! QEMU command builder for installation tests.
//!
//! Adapted from leviso's qemu.rs with additions for installation testing.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Builder for QEMU commands - consolidates common configuration patterns.
#[derive(Default)]
pub struct QemuBuilder {
    cpu: Option<String>,
    memory: Option<String>,
    kernel: Option<PathBuf>,
    initrd: Option<PathBuf>,
    append: Option<String>,
    cdrom: Option<PathBuf>,
    disk: Option<PathBuf>,
    ovmf: Option<PathBuf>,
    nographic: bool,
    no_reboot: bool,
    boot_cdrom_first: bool,
}

impl QemuBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set CPU type (default: Skylake-Client for x86-64-v3 support)
    pub fn cpu(mut self, cpu: &str) -> Self {
        self.cpu = Some(cpu.to_string());
        self
    }

    /// Set memory size (e.g., "512M", "1G")
    pub fn memory(mut self, mem: &str) -> Self {
        self.memory = Some(mem.to_string());
        self
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

    /// Boot from CDROM first (for live ISO boot)
    pub fn boot_cdrom(mut self) -> Self {
        self.boot_cdrom_first = true;
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

    /// Build the QEMU command (interactive)
    pub fn build(self) -> Command {
        self.build_base()
    }

    fn build_base(self) -> Command {
        let mut cmd = Command::new("qemu-system-x86_64");

        // Start with no default devices to avoid conflicts with explicit drive definitions
        // (QEMU 10+ has stricter drive index handling)
        cmd.arg("-nodefaults");

        // CPU (default: Skylake-Client for x86-64-v3 support required by Rocky 10)
        let cpu = self.cpu.as_deref().unwrap_or("Skylake-Client");
        cmd.args(["-cpu", cpu]);

        // Memory (default: 2G for installation - don't use toy values)
        let mem = self.memory.as_deref().unwrap_or("2G");
        cmd.args(["-m", mem]);

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

        // UEFI firmware
        if let Some(ovmf) = &self.ovmf {
            cmd.args([
                "-drive",
                &format!("if=pflash,format=raw,readonly=on,file={}", ovmf.display()),
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

        // Boot order (for UEFI, OVMF respects this hint)
        if self.boot_cdrom_first {
            cmd.args(["-boot", "d"]);
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
