# install-tests

E2E test runner. Boots LevitateOS ISO in QEMU, runs installation steps, verifies results.

**This is where installation tests go.** Not `leviso/tests/`.

## Status

**Alpha.** All 6 phases implemented. 24 test steps.

| Phase | Steps | Description |
|-------|-------|-------------|
| 1. Boot | 1-2 | UEFI verification, clock sync |
| 2. Disk | 3-6 | Partition, format, mount |
| 3. Base System | 7-10 | recstrap extract, fstab, chroot |
| 4. Configuration | 11-15 | Timezone, locale, hostname, passwords, user |
| 5. Bootloader | 16-18 | Initramfs, systemd-boot, services |
| 6. Post-Reboot | 19-24 | Boots installed system, verifies login/network/sudo |

## Binaries

Two binaries are provided:

| Binary | Purpose |
|--------|---------|
| `install-tests` | Full E2E installation test (24 steps) |
| `boot-test` | Isolated hypothesis test for systemd-boot ESP layout |

## Usage

```bash
# Full installation test (all 24 steps)
cargo run --bin install-tests -- run

# Run specific phase (1-6)
cargo run --bin install-tests -- run --phase 2

# Run specific step (1-24)
cargo run --bin install-tests -- run --step 5

# List all steps with descriptions
cargo run --bin install-tests -- list

# Isolated boot hypothesis test
cargo run --bin boot-test
```

## Options

```
--step <N>              Run only step N (1-24)
--phase <N>             Run only phase N (1-6)
--leviso-dir <PATH>     Path to leviso directory (default: ../leviso)
--iso <PATH>            Path to ISO file (default: <leviso_dir>/output/levitateos.iso)
--disk-size <SIZE>      Virtual disk size (default: 8G)
--keep-vm               Keep VM running after tests (for debugging)
```

## Requirements

- QEMU with KVM support
- OVMF (UEFI firmware)
- OVMF_VARS (writable EFI variable storage)
- Built LevitateOS ISO (`leviso/output/levitateos.iso`)
- Built initramfs (`leviso/output/initramfs-tiny.cpio.gz`)
- Extracted kernel (`leviso/downloads/iso-contents/images/pxeboot/vmlinuz`)

## Code Structure

```
src/
├── main.rs              # install-tests CLI
├── bin/
│   └── boot-test.rs     # Isolated boot hypothesis test
├── qemu/
│   ├── mod.rs           # QEMU utilities
│   ├── builder.rs       # QemuBuilder for command construction
│   ├── console.rs       # Console I/O handling
│   ├── exec.rs          # Command execution in guest
│   ├── boot.rs          # Boot detection logic
│   ├── sync.rs          # Test lock and process cleanup
│   └── utils.rs         # OVMF finding, disk creation
└── steps/
    ├── mod.rs           # Step trait, all_steps()
    ├── phase1_boot.rs   # Steps 1-2
    ├── phase2_disk.rs   # Steps 3-6
    ├── phase3_base.rs   # Steps 7-10
    ├── phase4_config.rs # Steps 11-15
    ├── phase5_boot.rs   # Steps 16-18
    └── phase6_verify.rs # Steps 19-24
```

## Known Limitations

- Runs in QEMU only - no bare metal testing
- Single-threaded execution
- Requires exclusive lock (one test at a time)

## Building

```bash
cargo build
cargo test    # Unit tests only, not E2E
```

## License

MIT
