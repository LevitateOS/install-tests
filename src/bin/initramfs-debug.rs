//! initramfs-debug - Test initramfs directly without UKI/UEFI
//!
//! This is a DEBUGGING tool to isolate whether the initramfs works independently
//! of the UKI packaging and systemd-stub loading process.
//!
//! If this test PASSES but the full ISO boot FAILS, the problem is in:
//! - UKI creation (ukify)
//! - systemd-stub (known regression in 258.x)
//! - OVMF/UEFI interaction
//!
//! If this test also FAILS, the problem is in the initramfs itself.
//!
//! NOTE: This bypasses UEFI entirely and is NOT a valid production test.
//!       It's purely for debugging initramfs issues.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    println!("═══════════════════════════════════════════════════════════════════════════");
    println!("  INITRAMFS DEBUG TEST");
    println!("═══════════════════════════════════════════════════════════════════════════");
    println!();
    println!("This test boots the kernel and initramfs DIRECTLY via QEMU's -kernel/-initrd");
    println!("flags, bypassing UEFI/UKI entirely. This isolates initramfs issues from");
    println!("UKI/systemd-stub issues.");
    println!();

    // Kill stale QEMU processes
    kill_stale_qemu();

    // Find required files - use absolute path from manifest dir
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let leviso_dir = manifest_dir.parent().unwrap().parent().unwrap().join("leviso");
    let kernel_path = leviso_dir.join("output/iso-root/boot/vmlinuz");
    let initramfs_path = leviso_dir.join("output/iso-root/boot/initramfs-live.img");
    let iso_path = leviso_dir.join("output/levitateos.iso");

    // Verify all required files exist
    if !kernel_path.exists() {
        bail!(
            "Kernel not found at {}.\n\
             Run 'cargo run -- build' in leviso first.",
            kernel_path.display()
        );
    }
    if !initramfs_path.exists() {
        bail!(
            "Initramfs not found at {}.\n\
             Run 'cargo run -- initramfs' in leviso first.",
            initramfs_path.display()
        );
    }
    if !iso_path.exists() {
        bail!(
            "ISO not found at {}.\n\
             Run 'cargo run -- iso' in leviso first.",
            iso_path.display()
        );
    }

    println!("Using:");
    println!("  Kernel:    {}", kernel_path.display());
    println!("  Initramfs: {}", initramfs_path.display());
    println!("  ISO:       {} (for rootfs)", iso_path.display());
    println!();

    // Build cmdline - same as UKI but without efi=debug (no EFI here)
    let cmdline = "root=LABEL=LEVITATEOS console=ttyS0,115200n8 console=tty0 selinux=0";
    println!("Cmdline: {}", cmdline);
    println!();

    // Build QEMU command for direct kernel boot
    let mut cmd = Command::new("qemu-system-x86_64");

    // Enable KVM if available
    if std::path::Path::new("/dev/kvm").exists() {
        cmd.arg("-enable-kvm");
    }

    cmd.args(["-nodefaults"]);
    cmd.args(["-cpu", "Skylake-Client"]);
    cmd.args(["-m", "4G"]);

    // Direct kernel boot - this bypasses UEFI entirely
    cmd.args(["-kernel", kernel_path.to_str().unwrap()]);
    cmd.args(["-initrd", initramfs_path.to_str().unwrap()]);
    cmd.args(["-append", cmdline]);

    // Attach ISO as CD-ROM (for rootfs access)
    cmd.args(["-device", "virtio-scsi-pci,id=scsi0"]);
    cmd.args(["-device", "scsi-cd,drive=cdrom0,bus=scsi0.0"]);
    cmd.args([
        "-drive",
        &format!(
            "id=cdrom0,if=none,format=raw,readonly=on,file={}",
            iso_path.display()
        ),
    ]);

    // Serial console
    cmd.args(["-nographic", "-serial", "mon:stdio"]);
    cmd.arg("-no-reboot");

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    println!("Starting QEMU with direct kernel boot...");
    println!("═══════════════════════════════════════════════════════════════════════════");
    println!();

    let mut child = cmd.spawn().context("Failed to spawn QEMU")?;
    let stdout = child.stdout.take().context("Failed to get stdout")?;

    // Create channel for output reading
    let (tx, rx): (_, Receiver<String>) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    // Watch for boot progress
    let start = Instant::now();
    let timeout = Duration::from_secs(120);

    let success_patterns = [
        "LevitateOS Live",          // Our issue message
        "Startup finished",         // systemd boot complete
        "login:",                   // Login prompt
        "Welcome to",               // systemd welcome
    ];

    let kernel_start_patterns = [
        "Linux version",            // Kernel started
        "KASLR",                    // Kernel randomization
        "Kernel command line",      // Kernel parsing cmdline
        "Unpacking initramfs",      // Initramfs being unpacked (KEY!)
    ];

    let failure_patterns = [
        "check access for rdinit=/init failed",  // Our known error
        "VFS: Cannot open root device",          // Root not found
        "Kernel panic",                          // Panic
        "not syncing",                           // Panic reason
        "No init found",                         // init not found
    ];

    let mut kernel_started = false;
    let mut initramfs_unpacked = false;
    let mut boot_success = false;
    let mut boot_failed = false;
    let mut failure_reason = String::new();

    while start.elapsed() < timeout {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                println!("{}", line);

                // Check for kernel start
                for pattern in &kernel_start_patterns {
                    if line.contains(pattern) {
                        if *pattern == "Unpacking initramfs" {
                            initramfs_unpacked = true;
                        } else if !kernel_started {
                            kernel_started = true;
                        }
                    }
                }

                // Check for success
                for pattern in &success_patterns {
                    if line.contains(pattern) {
                        boot_success = true;
                        break;
                    }
                }

                // Check for failure
                for pattern in &failure_patterns {
                    if line.contains(pattern) {
                        boot_failed = true;
                        failure_reason = pattern.to_string();
                        break;
                    }
                }

                if boot_success || boot_failed {
                    // Give a moment for more output, then stop
                    std::thread::sleep(Duration::from_secs(2));
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                println!();
                println!("QEMU exited");
                break;
            }
        }
    }

    // Cleanup
    let _ = child.kill();
    let _ = child.wait();

    // Report results
    println!();
    println!("═══════════════════════════════════════════════════════════════════════════");
    println!("  RESULTS");
    println!("═══════════════════════════════════════════════════════════════════════════");
    println!();
    println!("Kernel started:      {}", if kernel_started { "YES" } else { "NO" });
    println!("Initramfs unpacked:  {}", if initramfs_unpacked { "YES" } else { "NO" });
    println!("Boot completed:      {}", if boot_success { "YES" } else { "NO" });
    println!();

    if boot_success {
        println!("SUCCESS: Initramfs works correctly with direct kernel boot!");
        println!();
        println!("DIAGNOSIS: Since direct boot works but UKI boot fails, the problem is:");
        println!("  1. systemd-stub (258.x regression) - not passing initrd to kernel");
        println!("  2. UKI creation (ukify) - initrd not properly embedded");
        println!("  3. OVMF/UEFI interaction - EFI stub not loading initrd");
        println!();
        println!("NEXT STEPS:");
        println!("  1. Check systemd version: rpm -q systemd-ukify");
        println!("  2. Try upgrading systemd packages");
        println!("  3. Check ukify output for warnings");
        println!("  4. Verify UKI has .initrd section: objdump -h levitateos-live.efi");
        Ok(())
    } else if boot_failed {
        println!("FAILURE: {}", failure_reason);
        println!();
        if initramfs_unpacked {
            println!("The initramfs was unpacked but boot failed.");
            println!("This suggests an issue with /init or rootfs mounting.");
        } else if kernel_started {
            println!("The kernel started but initramfs was NOT unpacked!");
            println!("This should NOT happen with direct -initrd boot.");
            println!("Check that the initramfs file is valid:");
            println!("  file {}", initramfs_path.display());
            println!("  gunzip -c {} | cpio -t | head", initramfs_path.display());
        } else {
            println!("The kernel did not start.");
            println!("Check QEMU configuration and kernel compatibility.");
        }
        bail!("Boot failed: {}", failure_reason)
    } else {
        println!("TIMEOUT: No boot completion detected in {} seconds", timeout.as_secs());
        if kernel_started {
            println!("The kernel started but never completed boot.");
            println!("This suggests the system is hanging somewhere.");
        } else {
            println!("The kernel never started.");
        }
        bail!("Timeout waiting for boot")
    }
}

/// Kill any stale QEMU processes from previous test runs
fn kill_stale_qemu() {
    let patterns = [
        "leviso-install-test.qcow2",
        "boot-hypothesis-test.qcow2",
        "levitateos.iso",
        "initramfs-debug",
    ];
    for pattern in patterns {
        let _ = Command::new("pkill")
            .args(["-9", "-f", &format!("qemu-system-x86_64.*{}", pattern)])
            .status();
    }
    std::thread::sleep(Duration::from_millis(100));
}
