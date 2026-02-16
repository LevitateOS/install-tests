# Stage Test Scripts

This directory contains **stage test scripts** that validate each stage of the installation and boot process. These scripts are **pre-installed on every ISO** and can be run both manually and automatically.

## Philosophy: Video Game Savepoints

Like savepoints in video games, each stage represents a verified state you can "load" and inspect:

```
Stage 01: Fresh boot          → [SAVE] → interactive shell
Stage 02: Boot + tool tests   → [SAVE] → interactive shell
Stage 03: Boot + installation → [SAVE] → interactive shell
```

## Directory Structure

```
test-scripts/
├── lib/
│   └── common.sh                      # Shared testing functions
├── stage-01-live-boot.sh          # Verify live boot works
├── stage-02-live-tools.sh         # Verify all tools functional
├── stage-03-installation.sh       # Verify installation completed
├── stage-04-installed-boot.sh     # Verify boots from disk
├── stage-05-automated-login.sh    # Verify login works
├── stage-06-daily-driver.sh       # Verify daily driver tools
└── README.md                          # This file
```

## Usage

### Manual Testing (On the Live ISO)

```bash
# Inside QEMU or on real hardware
stage-02-live-tools.sh

# Debug mode (see every command)
bash -x stage-02-live-tools.sh

# Read the script
cat /usr/local/bin/stage-02-live-tools.sh
```

### Automated Testing (From Host)

```bash
# Interactive mode - drops you at shell after test
just stage 2 acorn

# Automated mode - runs test, reports result, exits
just test 2 acorn
```

## Stage Descriptions

### Stage 01: Live Boot
**Purpose:** Verify the live ISO boots successfully
**Tests:** Basic filesystem checks, shell functionality
**Environment:** Live ISO

### Stage 02: Live Tools
**Purpose:** Verify all daily driver tools work in live environment
**Tests:** EXECUTES each tool (not just checks existence)
**Environment:** Live ISO
**Tools tested:**
- Installation tools (recstrap, recfstab, sfdisk, mkfs.ext4, etc.)
- Network tools (ip, ping, curl)
- Hardware diagnostics (lspci, lsusb, smartctl, hdparm)
- Editors (vim, less)
- System utilities (htop, grep, find)

### Stage 03: Installation
**Purpose:** Verify scripted installation to disk succeeds
**Tests:** Root filesystem extracted, bootloader installed, fstab created
**Environment:** Live ISO (post-installation)

### Stage 04: Installed Boot
**Purpose:** Verify system boots from disk after installation
**Tests:** Basic boot checks, filesystems mounted
**Environment:** Installed system

### Stage 05: Automated Login
**Purpose:** Verify automated login works
**Tests:** Can login as root, run commands
**Environment:** Installed system

### Stage 06: Daily Driver Tools
**Purpose:** Verify all tools work on installed system
**Tests:** sudo, ssh, networking, shell, etc.
**Environment:** Installed system

## Writing Test Scripts

### Template

```bash
#!/bin/bash
set -euo pipefail

# Load common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib/common.sh" || source "/usr/local/lib/stage-tests/common.sh"

# Header
stage_header N "Stage Name"

# Tests
section_header "Test Category"
test_tool "tool-name" "tool-name --version"
test_file_exists "/path/to/file" "description"
test_command "description" "command to run"

# Report
report_results N
exit $?
```

### Available Functions (from `lib/common.sh`)

#### Testing Functions
- `test_tool <name> <command>` - Test a tool by executing it
- `test_file_exists <path> <description>` - Check file/directory exists
- `test_command <description> <command>` - Test arbitrary command succeeds

#### Output Functions
- `stage_header <number> <name>` - Print stage header
- `section_header <name>` - Print section header
- `info <message>` - Print info message
- `warn <message>` - Print warning
- `error <message>` - Print error
- `success <message>` - Print success message
- `report_results <stage>` - Print final results (pass/fail)

#### State Variables
- `PASSED_TESTS[]` - Array of tests that passed
- `FAILED_TESTS[]` - Array of tests that failed (tool not found)
- `BROKEN_TESTS[]` - Array of tests that are broken (tool exists but failed)

### Exit Codes

Scripts exit with:
- `0` - All tests passed
- `1` - One or more tests failed

### Example Output

```
═══════════════════════════════════════════════════════════
  Stage 02: Live Tools Validation
═══════════════════════════════════════════════════════════

Core Installation Tools:
  [TEST] recstrap... ✓
  [TEST] recfstab... ✓
  [TEST] vim... ✓
  [TEST] htop... ✗ NOT FOUND

═══════════════════════════════════════════════════════════
  Stage 02 Results
═══════════════════════════════════════════════════════════
Passed: 17/18 tests

Missing (not in PATH): 1 tests
  • htop

═══════════════════════════════════════════════════════════
✗ STAGE 02 FAILED

Some tools are missing from PATH. Check package installation.
```

## Integration with Build System

These scripts are automatically installed on every ISO during the build process:

- **Source:** `testing/install-tests/test-scripts/`
- **Destination (on ISO):** `/usr/local/bin/stage-*.sh`
- **Libraries:** `/usr/local/lib/stage-tests/`

See `AcornOS/src/component/definitions.rs` (STAGE_TESTS component) for build integration.

Other build integrations:
- `IuppiterOS/src/component/definitions.rs` (STAGE_TESTS component)
- `leviso/src/component/definitions.rs` (FINAL component installs via `CustomOp::InstallStageTests`)

## CI/Automation

For CI and automated testing, the Rust-based test harness in `testing/install-tests/` can:
1. Boot QEMU
2. Trigger these scripts via serial console
3. Parse output and determine pass/fail
4. Kill QEMU and report results

This provides the best of both worlds:
- **Scripts on ISO:** Manual testing, always available
- **Rust harness:** Automated testing, CI integration

## Design Principles

✅ **Always available** - Scripts shipped on every ISO
✅ **Manual testing** - Users can run scripts directly
✅ **Transparent** - Clear what each test does
✅ **Real execution** - Actually runs tools, not just checks existence
✅ **Detailed reporting** - Distinguishes missing vs broken tools
✅ **Interactive debugging** - Can inspect post-test state

## Next Steps

1. ✅ Phase 1: Test scripts created (this directory)
2. ✅ Phase 2: Integrate into ISO builds
3. ⏳ Phase 3: Add interactive stage mode
4. ⏳ Phase 4: Update justfile commands
5. ⏳ Phase 5: Add auto-run support
6. ⏳ Phase 6: Documentation and testing
