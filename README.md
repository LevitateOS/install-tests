# install-tests

E2E test runner. Boots LevitateOS ISO in QEMU, runs installation steps, verifies results.

**This is where installation tests go.** Not `leviso/tests/`.

## Status

**Alpha.** Phases 1-5 work. Phase 6 (post-reboot verification) incomplete.

| Phase | Status |
|-------|--------|
| 1. Boot | Works |
| 2. Disk (partition, format, mount) | Works |
| 3. Base System (extract, fstab, chroot) | Works |
| 4. Configuration (timezone, users) | Works |
| 5. Boot Setup (initramfs, bootloader) | Works |
| 6. Post-Reboot Verification | **Incomplete** |

## Usage

```bash
cargo run -- run              # Run phases 1-5
cargo run -- run --phase 2    # Run specific phase
cargo run -- run --step 5     # Run specific step
cargo run -- list             # Show all steps
```

## Requirements

- QEMU with KVM support
- OVMF (UEFI firmware)
- Built LevitateOS ISO (`leviso/output/levitateos.iso`)

## Options

```
--disk-size 16G    # Virtual disk size (default: 8G)
--keep-vm          # Don't kill VM after tests
--phase N          # Run only phase N
--step N           # Run only step N
```

## Test Phases

1. **Boot** - QEMU starts, UEFI loads, reaches shell
2. **Disk** - `fdisk`, `mkfs.fat`, `mkfs.ext4`, `mount`
3. **Base System** - `recstrap`, `recfstab`, `recchroot` setup
4. **Configuration** - timezone, locale, hostname, root password, user
5. **Boot Setup** - dracut initramfs, bootctl install, systemctl enable
6. **Post-Reboot** - (incomplete) reboot to installed system, verify login

## Known Limitations

- Phase 6 not implemented - doesn't verify installed system boots
- Runs in QEMU only - no bare metal testing
- Single-threaded execution

## Building

```bash
cargo build
cargo test    # Unit tests only, not E2E
```

## License

MIT
