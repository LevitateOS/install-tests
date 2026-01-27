//! Error pattern constants for QEMU console monitoring.
//!
//! Patterns are checked during boot and command execution to fail fast
//! when problems are detected.

/// Fatal error patterns that should cause immediate failure.
/// When ANY of these appear in output, stop waiting and return failure.
pub const FATAL_ERROR_PATTERNS: &[&str] = &[
    "FATAL:",               // Generic fatal
    "Kernel panic",         // Kernel panic
    "not syncing",          // Kernel panic continuation
    "Segmentation fault",   // Segfault
    "core dumped",          // Core dump
    "systemd-coredump",     // Systemd detected crash
];

/// Critical boot error patterns - FAIL IMMEDIATELY when seen.
/// These indicate the boot will never succeed.
pub const CRITICAL_BOOT_ERRORS: &[&str] = &[
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
    "SQUASHFS error",               // Squashfs corruption (legacy)
    "EROFS:",                       // EROFS filesystem error

    // === INIT STAGE (critical) ===
    "emergency shell",              // Dropped to emergency
    "Emergency shell",              // Alternate casing
    "emergency.target",             // Systemd emergency
    "rescue.target",                // Systemd rescue mode
    "Timed out waiting for device", // Device timeout

    // === GENERAL ===
    "fatal error",                  // Generic fatal
    "Segmentation fault",           // Segfault
    "core dumped",                  // Core dump
];

/// Boot error patterns - for live ISO boot, FAIL IMMEDIATELY on these.
/// For installed system, we use a more lenient approach to capture diagnostics.
pub const BOOT_ERROR_PATTERNS: &[&str] = &[
    // Include all critical errors
    "No bootable device",
    "Boot Failed",
    "Default Boot Device Missing",
    "Shell>",
    "ASSERT_EFI_ERROR",
    "map: Cannot find",
    "systemd-boot: Failed",
    "loader: Failed",
    "vmlinuz: not found",
    "initramfs: not found",
    "Error loading",
    "File not found",
    "Kernel panic",
    "not syncing",
    "VFS: Cannot open root device",
    "No init found",
    "Attempted to kill init",
    "can't find /init",
    "No root device",
    "SQUASHFS error",
    "EROFS:",                       // EROFS filesystem error
    "emergency shell",
    "Emergency shell",
    "emergency.target",
    "rescue.target",
    "Failed to start",              // For live ISO, any service failure is bad
    "Timed out waiting for device",
    "Dependency failed",
    "FAILED:",
    "fatal error",
    "Segmentation fault",
    "core dumped",
];

/// Service failure patterns - track these during installed system boot
/// so we can capture diagnostics instead of failing immediately.
pub const SERVICE_FAILURE_PATTERNS: &[&str] = &[
    "Failed to start",
    "[FAILED]",
    "Dependency failed",
];
