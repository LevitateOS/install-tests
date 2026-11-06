# CLAUDE.md - install-tests

## What is install-tests?

E2E test runner for LevitateOS installation. Boots ISO in QEMU, performs full installation, reboots into installed system, verifies success.

**This is THE crate for installation tests.**

## What Belongs Here

- QEMU-based installation tests
- Full installation flow verification
- Boot → Install → Reboot → Verify tests

## What Does NOT Belong Here

| Don't put here | Put it in |
|----------------|-----------|
| Built-artifact validation | `testing/fsdbg/` |
| Rootfs/UX tests | `testing/rootfs-tests/` |
| Hardware compatibility | `testing/hardware-compat/` |

## Commands

```bash
cargo build
cargo check # Unit tests are prohibited; use focused integration/runtime checks.
cargo run    # Run the test suite
```

## Test Philosophy

**ONE test. ONE QEMU. ONE installation. ONE reboot. ONE verification.**

The test does what users do:
1. Boot ISO in QEMU
2. Partition, format and mount the disk (recpart, over SSH)
3. Extract rootfs (recstrap)
4. Generate fstab (recfstab)
5. Install bootloader (bootctl in chroot, via recchroot)
6. Reboot into installed system
7. Verify system works

If this test passes, users CAN install. If it fails, fix the installation process.
