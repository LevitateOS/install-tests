//! boot-test - Isolated test for systemd-boot with kernel on ESP
//!
//! Tests ONLY: "If kernel is on ESP, does systemd-boot work?"
//! Creates minimal bootable disk without full squashfs extraction.
//!
//! This is a hypothesis test. If the kernel STARTS loading, the hypothesis
//! is confirmed: systemd-boot can only read from FAT partitions.
//!
//! # Success Criteria
//!
//! The test PASSES if we see ANY of these in QEMU output:
//! - `Loading kernel...` or similar systemd-boot message
//! - Linux kernel decompression message (`Decompressing Linux...`)
//! - KASLR message (`KASLR using...`)
//! - Kernel boot messages (`Linux version X.X.X...`)
//!
//! The test can FAIL at init (kernel panic looking for root) - that's fine!
//! We just need proof that systemd-boot found and loaded the kernel.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    println!("=== Boot Hypothesis Test ===");
    println!();
    println!("Testing: If kernel is on ESP (FAT32), does systemd-boot find it?");
    println!();

    // Kill stale QEMU processes before starting
    kill_stale_qemu();

    // Find required files
    let leviso_dir = PathBuf::from("../leviso");
    // Use the LevitateOS kernel from the ISO build, NOT the Rocky kernel from downloads
    // The LevitateOS kernel (6.18.0) has virtio drivers built-in; Rocky kernel (6.12.0) has them as modules
    let kernel_path = leviso_dir.join("output/iso-root/boot/vmlinuz");
    let initramfs_path = leviso_dir.join("output/initramfs-tiny.cpio.gz");
    let iso_path = leviso_dir.join("output/levitateos.iso");

    if !kernel_path.exists() {
        bail!("Kernel not found at {}. Run 'cargo run -- build' in leviso first.", kernel_path.display());
    }
    if !initramfs_path.exists() {
        bail!("Initramfs not found at {}. Run 'cargo run -- initramfs' in leviso first.", initramfs_path.display());
    }
    if !iso_path.exists() {
        bail!("ISO not found at {}. Run 'cargo run -- iso' in leviso first.", iso_path.display());
    }

    // Find OVMF
    let ovmf = find_ovmf().context("OVMF not found - UEFI boot required")?;
    println!("Using OVMF: {}", ovmf.display());

    // Create test disk
    let disk_path = std::env::temp_dir().join("boot-hypothesis-test.qcow2");
    if disk_path.exists() {
        std::fs::remove_file(&disk_path)?;
    }
    create_disk(&disk_path, "8G")?;
    println!("Created test disk: {}", disk_path.display());

    // ═══════════════════════════════════════════════════════════════════════
    // PHASE 1: Boot live ISO and prepare minimal bootable disk
    // ═══════════════════════════════════════════════════════════════════════
    println!();
    println!("Phase 1: Preparing minimal bootable disk from live ISO...");

    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-nodefaults", "-cpu", "Skylake-Client", "-m", "2G"]);
    cmd.args(["-kernel", kernel_path.to_str().unwrap()]);
    cmd.args(["-initrd", initramfs_path.to_str().unwrap()]);
    cmd.args(["-append", "console=tty0 console=ttyS0,115200n8 rdinit=/init panic=30"]);
    cmd.args(["-drive", &format!("file={},format=qcow2,if=virtio", disk_path.display())]);
    cmd.args(["-device", "virtio-scsi-pci,id=scsi0"]);
    cmd.args(["-device", "scsi-cd,drive=cdrom0,bus=scsi0.0"]);
    cmd.args(["-drive", &format!("id=cdrom0,if=none,format=raw,readonly=on,file={}", iso_path.display())]);
    cmd.args(["-drive", &format!("if=pflash,format=raw,readonly=on,file={}", ovmf.display())]);
    cmd.args(["-nographic", "-serial", "mon:stdio", "-no-reboot"]);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let mut console = Console::new(&mut child)?;

    println!("Waiting for live ISO to boot...");
    console.wait_for_boot(Duration::from_secs(90))?;
    println!("Live ISO booted!");

    // Small delay for system to settle
    std::thread::sleep(Duration::from_secs(2));

    // Partition disk
    println!("  Partitioning disk...");
    let sfdisk_script = "label: gpt\n,512M,U\n,,L\n";
    console.exec_ok(&format!("echo '{}' | sfdisk /dev/vda", sfdisk_script), Duration::from_secs(30))?;
    console.exec_ok("partprobe /dev/vda 2>/dev/null || sleep 2", Duration::from_secs(10))?;
    console.exec_ok("udevadm settle --timeout=5 2>/dev/null || sleep 2", Duration::from_secs(10))?;

    // Verify partitions
    let lsblk = console.exec_ok("lsblk /dev/vda", Duration::from_secs(5))?;
    if !lsblk.contains("vda1") || !lsblk.contains("vda2") {
        bail!("Partitions not created. lsblk output:\n{}", lsblk);
    }
    println!("  Partitions created.");

    // Format partitions
    println!("  Formatting partitions...");
    console.exec_ok("mkfs.fat -F32 /dev/vda1", Duration::from_secs(30))?;
    console.exec_ok("mkfs.ext4 -F /dev/vda2", Duration::from_secs(60))?;
    println!("  Formatted: vda1=FAT32, vda2=ext4");

    // Mount: ESP at /mnt/boot (KEY: this is the hypothesis - ESP at /boot, not /boot/efi)
    println!("  Mounting partitions (ESP at /mnt/boot)...");
    console.exec_ok("mkdir -p /mnt", Duration::from_secs(5))?;
    console.exec_ok("mount /dev/vda2 /mnt", Duration::from_secs(10))?;
    console.exec_ok("mkdir -p /mnt/boot", Duration::from_secs(5))?;
    console.exec_ok("mount /dev/vda1 /mnt/boot", Duration::from_secs(10))?;
    println!("  Mounted: vda2->/mnt, vda1->/mnt/boot (ESP)");

    // Create minimal root filesystem
    println!("  Creating minimal root structure...");
    console.exec_ok("mkdir -p /mnt/{etc,bin,lib64,usr,var,proc,sys,dev,run}", Duration::from_secs(5))?;

    // Copy kernel and initramfs from ISO to ESP
    // The ISO is mounted at /media/cdrom by the tiny initramfs
    println!("  Copying kernel and initramfs from ISO to ESP...");
    console.exec_ok("cp /media/cdrom/boot/vmlinuz /mnt/boot/vmlinuz", Duration::from_secs(10))?;
    console.exec_ok("cp /media/cdrom/boot/initramfs-live.img /mnt/boot/initramfs.img", Duration::from_secs(10))?;

    // Verify files are on ESP
    let ls_boot = console.exec_ok("ls -la /mnt/boot/", Duration::from_secs(5))?;
    println!("  ESP contents:\n{}", ls_boot);

    // Install systemd-boot
    // Note: bootctl isn't available in the live environment, so we do manual installation
    println!("  Installing systemd-boot manually...");

    // Create EFI directory structure
    console.exec_ok("mkdir -p /mnt/boot/EFI/BOOT", Duration::from_secs(5))?;
    console.exec_ok("mkdir -p /mnt/boot/EFI/systemd", Duration::from_secs(5))?;
    console.exec_ok("mkdir -p /mnt/boot/loader/entries", Duration::from_secs(5))?;

    // Copy systemd-boot EFI binary from squashfs
    // The squashfs contains /usr/lib/systemd/boot/efi/systemd-bootx64.efi
    console.exec_ok("cp /usr/lib/systemd/boot/efi/systemd-bootx64.efi /mnt/boot/EFI/BOOT/BOOTX64.EFI", Duration::from_secs(10))?;
    console.exec_ok("cp /usr/lib/systemd/boot/efi/systemd-bootx64.efi /mnt/boot/EFI/systemd/systemd-bootx64.efi", Duration::from_secs(10))?;

    // Create loader.conf
    let loader_conf = "default levitateos.conf\ntimeout 3\nconsole-mode max\neditor no\n";
    console.write_file("/mnt/boot/loader/loader.conf", loader_conf)?;

    // Get root UUID
    let uuid_output = console.exec_ok("blkid -s UUID -o value /dev/vda2", Duration::from_secs(5))?;
    let root_uuid = uuid_output.trim();
    println!("  Root UUID: {}", root_uuid);

    // Create boot entry
    // Note: paths are relative to ESP root (/)
    let boot_entry = format!(
        "title   LevitateOS\nlinux   /vmlinuz\ninitrd  /initramfs.img\noptions root=UUID={} rw quiet console=ttyS0,115200n8\n",
        root_uuid
    );
    console.write_file("/mnt/boot/loader/entries/levitateos.conf", &boot_entry)?;

    // Verify boot entry
    let entry_content = console.exec_ok("cat /mnt/boot/loader/entries/levitateos.conf", Duration::from_secs(5))?;
    println!("  Boot entry:\n{}", entry_content);

    // Show final ESP structure
    let esp_tree = console.exec_ok("find /mnt/boot -type f", Duration::from_secs(5))?;
    println!("  Final ESP structure:\n{}", esp_tree);

    // Unmount
    println!("  Unmounting...");
    console.exec_ok("umount /mnt/boot", Duration::from_secs(10))?;
    console.exec_ok("umount /mnt", Duration::from_secs(10))?;

    // Shutdown live ISO
    println!("  Shutting down live ISO...");
    let _ = console.exec("poweroff -f", Duration::from_secs(5));
    drop(console);
    // ALWAYS kill in case poweroff didn't work
    let _ = child.kill();
    let _ = child.wait();
    std::thread::sleep(Duration::from_secs(1));

    // ═══════════════════════════════════════════════════════════════════════
    // PHASE 2: Boot from disk and verify kernel loads
    // ═══════════════════════════════════════════════════════════════════════
    println!();
    println!("Phase 2: Booting from disk (systemd-boot -> kernel)...");
    println!();
    println!("Looking for kernel load indicators:");
    println!("  - systemd-boot messages");
    println!("  - 'Linux version X.X.X'");
    println!("  - KASLR messages");
    println!("  - Kernel decompression");
    println!();

    // Boot from disk only (no kernel/initrd override, let UEFI find bootloader)
    let mut cmd2 = Command::new("qemu-system-x86_64");
    cmd2.args(["-nodefaults", "-cpu", "Skylake-Client", "-m", "2G"]);
    cmd2.args(["-drive", &format!("file={},format=qcow2,if=virtio", disk_path.display())]);
    cmd2.args(["-drive", &format!("if=pflash,format=raw,readonly=on,file={}", ovmf.display())]);
    cmd2.args(["-nographic", "-serial", "mon:stdio", "-no-reboot"]);
    cmd2.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());

    let mut child2 = cmd2.spawn().context("Failed to spawn QEMU for disk boot")?;
    let console2 = Console::new(&mut child2)?;

    // Look for kernel boot indicators (60 second timeout)
    let success_patterns = [
        "Linux version",       // Kernel started
        "KASLR",               // Kernel randomization
        "Booting Linux",       // systemd-boot message
        "Loading Linux",       // systemd-boot loading
        "Decompressing Linux", // Kernel decompression
        "Kernel command line", // Kernel parsing cmdline
        "Command line:",       // Alternate cmdline message
    ];

    let error_patterns = [
        "not found",           // Boot file not found
        "No bootable device",  // UEFI didn't find bootloader
        "Failed to start",     // systemd-boot error
        "Boot failed",         // Generic boot failure
    ];

    println!("Watching QEMU output for {} seconds...", 60);
    let start = Instant::now();
    let timeout = Duration::from_secs(60);
    let mut kernel_loaded = false;
    let mut boot_failed = false;
    let mut failure_reason = String::new();

    while start.elapsed() < timeout {
        match console2.rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                // Print all output for debugging
                println!("  {}", line);

                // Check for success
                for pattern in &success_patterns {
                    if line.contains(pattern) {
                        println!();
                        println!("=== KERNEL LOADED! ===");
                        println!("Matched pattern: {}", pattern);
                        kernel_loaded = true;
                        break;
                    }
                }

                // Check for failure
                for pattern in &error_patterns {
                    if line.to_lowercase().contains(&pattern.to_lowercase()) {
                        failure_reason = format!("Boot error: {}", pattern);
                        boot_failed = true;
                        break;
                    }
                }

                if kernel_loaded || boot_failed {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                println!("QEMU exited");
                break;
            }
        }
    }

    // Cleanup
    let _ = child2.kill();
    let _ = child2.wait();
    let _ = std::fs::remove_file(&disk_path);

    // Report results
    println!();
    println!("═══════════════════════════════════════════════════════════════");
    if kernel_loaded {
        println!("HYPOTHESIS CONFIRMED: Kernel loads when on ESP (FAT32)");
        println!();
        println!("This proves:");
        println!("  1. systemd-boot found BOOTX64.EFI on the ESP");
        println!("  2. systemd-boot found and loaded /vmlinuz from ESP");
        println!("  3. The kernel started executing");
        println!();
        println!("The fix is correct: Mount ESP at /boot, not /boot/efi");
        println!("═══════════════════════════════════════════════════════════════");
        Ok(())
    } else if boot_failed {
        println!("BOOT FAILED: {}", failure_reason);
        println!();
        println!("The hypothesis test did not succeed. Debug the boot process.");
        println!("═══════════════════════════════════════════════════════════════");
        bail!("Boot failed: {}", failure_reason)
    } else {
        println!("TIMEOUT: No kernel boot messages detected in 60 seconds");
        println!();
        println!("Possible issues:");
        println!("  - UEFI didn't find bootloader");
        println!("  - systemd-boot didn't find kernel");
        println!("  - Serial console not receiving output");
        println!("═══════════════════════════════════════════════════════════════");
        bail!("Timeout waiting for kernel boot")
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Console helper (simplified from install-tests/src/qemu/console.rs)
// ═══════════════════════════════════════════════════════════════════════════

struct Console {
    stdin: ChildStdin,
    rx: Receiver<String>,
}

impl Console {
    fn new(child: &mut Child) -> Result<Self> {
        let stdin = child.stdin.take().context("Failed to get QEMU stdin")?;
        let stdout = child.stdout.take().context("Failed to get QEMU stdout")?;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Ok(Self { stdin, rx })
    }

    fn wait_for_boot(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    if line.contains("Startup finished")
                        || line.contains("login:")
                        || line.contains("LevitateOS Live")
                    {
                        std::thread::sleep(Duration::from_secs(2));
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("QEMU exited before boot completed");
                }
            }
        }
        bail!("Timeout waiting for boot");
    }

    fn exec(&mut self, command: &str, timeout: Duration) -> Result<(bool, String)> {
        let cmd_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        let start_marker = format!("___START_{}___", cmd_id);
        let done_marker = format!("___DONE_{}___", cmd_id);

        let full_cmd = format!("echo '{}'; {}; echo '{}' $?\n", start_marker, command, done_marker);
        self.stdin.write_all(full_cmd.as_bytes())?;
        self.stdin.flush()?;

        let start = Instant::now();
        let mut output = String::new();
        let mut collecting = false;

        while start.elapsed() < timeout {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    if line.contains(&start_marker) {
                        collecting = true;
                        continue;
                    }
                    if line.contains(&done_marker) {
                        let exit_code: i32 = line
                            .split(&done_marker)
                            .nth(1)
                            .and_then(|s| s.split_whitespace().next())
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(-1);
                        return Ok((exit_code == 0, output));
                    }
                    if collecting {
                        output.push_str(&line);
                        output.push('\n');
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok((false, output));
                }
            }
        }
        Ok((false, output))
    }

    fn exec_ok(&mut self, command: &str, timeout: Duration) -> Result<String> {
        let (success, output) = self.exec(command, timeout)?;
        if !success {
            bail!("Command failed: {}\nOutput: {}", command, output);
        }
        Ok(output)
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<()> {
        let escaped = content
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
            .replace('\n', "\\n");
        let cmd = format!("printf \"{}\" > {}", escaped, path);
        self.exec_ok(&cmd, Duration::from_secs(10))?;
        Ok(())
    }
}

fn find_ovmf() -> Option<PathBuf> {
    let candidates = [
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/OVMF/OVMF_CODE_4M.fd",
        "/usr/share/qemu/OVMF.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
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

fn create_disk(path: &std::path::Path, size: &str) -> Result<()> {
    let status = Command::new("qemu-img")
        .args(["create", "-f", "qcow2", path.to_str().unwrap(), size])
        .status()?;
    if !status.success() {
        bail!("qemu-img create failed");
    }
    Ok(())
}

/// Kill any stale QEMU processes from previous test runs
fn kill_stale_qemu() {
    let patterns = [
        "leviso-install-test.qcow2",
        "boot-hypothesis-test.qcow2",
        "levitateos.iso",
    ];
    for pattern in patterns {
        let _ = Command::new("pkill")
            .args(["-9", "-f", &format!("qemu-system-x86_64.*{}", pattern)])
            .status();
    }
    std::thread::sleep(Duration::from_millis(100));
}
