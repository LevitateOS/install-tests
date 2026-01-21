# CLAUDE.md - Install Tests

## ⛔ STOP. READ. THEN ACT.

Every time you think you know where something goes - **stop. Read first.**

Every time you think something is worthless and should be deleted - **stop. Read it first.**

Every time you're about to write code - **stop. Read what already exists first.**

The five minutes you spend reading will save hours of cleanup.

**THIS is the crate for E2E installation tests.** Not `leviso/tests/`. If someone tells you to work on installation tests, THIS is where they mean. Read this crate's source code before writing anything.

---

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
