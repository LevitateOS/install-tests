# install-tests

> **STOP. READ. THEN ACT.** This is the CORRECT location for E2E installation tests. Not `leviso/tests/`. Read this crate's source before writing anything.

E2E test runner that boots LevitateOS in QEMU and verifies the complete installation process.

## Status

| Metric | Value |
|--------|-------|
| Stage | Alpha |
| Target | x86_64 Linux (QEMU + OVMF) |
| Last verified | 2026-01-23 |

### Works

- 6-phase installation test sequence
- QEMU boot with UEFI firmware
- Phase/step selection for debugging

### Incomplete / Stubbed

- Post-reboot verification (Phase 6)

### Known Issues

- See parent repo issues

---

## Author

<!-- HUMAN WRITTEN - DO NOT MODIFY -->

[Waiting for human input]

<!-- END HUMAN WRITTEN -->

---

## Prerequisites

- QEMU with OVMF (UEFI firmware)
- Built leviso (kernel + initramfs)

## Usage

```bash
cargo run -- run              # Run all tests (phases 1-6)
cargo run -- run --phase 2    # Run specific phase (1-6)
cargo run -- run --step 5     # Run specific step
cargo run -- list             # Show all steps by phase
```

## Test Phases

1. **Boot** - Verify UEFI, sync clock
2. **Disk** - Partition, format, mount
3. **Base System** - Mount install media, extract tarball, generate fstab, setup chroot
4. **Configuration** - Timezone, locale, hostname, root password, create user
5. **Boot Setup** - Generate initramfs, install bootloader, enable services
6. **Post-Reboot Verification** - Verify system boots from disk, user login, networking, sudo

## Options

```bash
--disk-size 16G    # Custom disk size
--keep-vm          # Keep VM running for debugging
```

## Development

```bash
cargo build        # Build
cargo test         # Run unit tests
cargo clippy       # Lint
```

## License

MIT
