# install-tests

> **STOP. READ. THEN ACT.** This is the CORRECT location for E2E installation tests. Not `leviso/tests/`. Read this crate's source before writing anything.

E2E test runner that boots LevitateOS in QEMU and verifies the complete installation process.

## Prerequisites

- QEMU with OVMF (UEFI firmware)
- Built leviso (kernel + initramfs)

## Usage

```bash
cargo run -- run              # Run all tests
cargo run -- run --phase 2    # Run specific phase
cargo run -- run --step 5     # Run specific step
cargo run -- list             # Show all steps
```

## Test Phases

1. **Boot** - Kernel boots, systemd starts
2. **Disk** - Partition, format, mount
3. **Base System** - Extract stage3 tarball
4. **Configuration** - Users, network, bootloader
5. **Final Boot** - Reboot into installed system

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
