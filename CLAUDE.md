# CLAUDE.md - Install Tests

## What is install-tests?

E2E test runner for LevitateOS installation process. Boots the ISO in QEMU and verifies installation steps work correctly.

LevitateOS is a **daily driver Linux distribution competing with Arch Linux**. The installation experience should match or exceed Arch's - users partition, format, extract tarball, configure, and boot into a working system.

## Development

```bash
# Build
cargo build

# Run tests
cargo test

# Check with clippy
cargo clippy

# Run the test suite
cargo run
```

## Common Mistakes

1. **Long timeouts** - Keep timeouts minimal (100-200ms), fail fast
2. **Host dependencies** - Tests should work in CI without special setup
3. **Flaky tests** - If a test is timing-sensitive, add proper synchronization

## Test Structure

Tests should verify:
- Boot process completes
- Installation steps can be executed
- Expected system state after installation
