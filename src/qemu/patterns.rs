//! Error pattern constants for QEMU console monitoring.
//!
//! Patterns are checked during boot and command execution to fail fast
//! when problems are detected.

/// Fatal error patterns that should cause immediate failure.
/// When ANY of these appear in output, stop waiting and return failure.
pub const FATAL_ERROR_PATTERNS: &[&str] = &[
    "dracut[F]:",           // dracut fatal error
    "dracut[E]: FAILED:",   // dracut install failed
    "dracut-install: ERROR:", // dracut-install binary failed
    "FATAL:",               // Generic fatal
    "Kernel panic",         // Kernel panic
    "not syncing",          // Kernel panic continuation
    "Segmentation fault",   // Segfault
    "core dumped",          // Core dump
    "systemd-coredump",     // Systemd detected crash
];

/// Boot error patterns - FAIL IMMEDIATELY when seen.
/// Organized by boot stage for clarity.
pub const BOOT_ERROR_PATTERNS: &[&str] = &[
    // === UEFI STAGE ===
    "No bootable device",           // UEFI found nothing
    "Boot Failed",                  // UEFI boot failed
    "Default Boot Device Missing",  // No default boot
    "Shell>",                       // Dropped to UEFI shell (no bootloader)
    "ASSERT_EFI_ERROR",             // UEFI assertion failed
    "map: Cannot find",             // UEFI can't find device

    // === BOOTLOADER STAGE ===
    "systemd-boot: Failed",         // systemd-boot error
    "loader: Failed",               // Generic loader error
    "vmlinuz: not found",           // Kernel not on ESP
    "initramfs: not found",         // Initramfs not on ESP
    "Error loading",                // Boot file load error
    "File not found",               // Missing boot file

    // === KERNEL STAGE ===
    "Kernel panic",                 // Kernel panic
    "not syncing",                  // Panic continuation
    "VFS: Cannot open root device", // Root not found
    "No init found",                // init missing
    "Attempted to kill init",       // init crashed
    "can't find /init",             // initramfs broken
    "No root device",               // Root device missing
    "SQUASHFS error",               // Squashfs corruption

    // === INIT STAGE ===
    "emergency shell",              // Dropped to emergency
    "Emergency shell",              // Alternate casing
    "emergency.target",             // Systemd emergency
    "rescue.target",                // Systemd rescue mode
    "Failed to start",              // Service start failure (broad)
    "Timed out waiting for device", // Device timeout
    "Dependency failed",            // Systemd dep failure

    // === GENERAL ===
    "FAILED:",                      // Generic failure marker
    "fatal error",                  // Generic fatal
    "Segmentation fault",           // Segfault
    "core dumped",                  // Core dump
];
