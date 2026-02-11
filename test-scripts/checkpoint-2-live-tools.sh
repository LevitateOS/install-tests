#!/bin/bash
# Checkpoint 2: Live Tools Validation
#
# Verifies that all expected tools are present and functional in the
# live environment. This actually EXECUTES each tool to ensure:
# - Binary can execute (not just exist in PATH)
# - Required libraries are present (no missing .so files)
# - Environment is properly configured (proc/sys/dev available)
# - Tool is functional (not broken/corrupted)
#
# This is the REAL validation - if a tool passes here, users can actually use it.

set -euo pipefail

# Find script directory and load common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Try multiple locations for common.sh
if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    source "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/checkpoint-tests/common.sh" ]; then
    source "/usr/local/lib/checkpoint-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    echo "Tried:" >&2
    echo "  - $SCRIPT_DIR/lib/common.sh" >&2
    echo "  - /usr/local/lib/checkpoint-tests/common.sh" >&2
    exit 1
fi

# ═══════════════════════════════════════════════════════════════════════════
# Main Test
# ═══════════════════════════════════════════════════════════════════════════

checkpoint_header 2 "Live Tools Validation"

info "This checkpoint verifies that all daily driver tools are present"
info "and FUNCTIONAL (actually executes them, not just checks existence)"
echo

# ═══════════════════════════════════════════════════════════════════════════
# Core Installation Tools
# ═══════════════════════════════════════════════════════════════════════════

section_header "Core Installation Tools"
test_tool "recstrap" "recstrap --help"
test_tool "recfstab" "recfstab --help"
test_tool "recchroot" "recchroot --help"
test_tool "sfdisk" "sfdisk --version"
test_tool "mkfs.ext4" "mkfs.ext4 -V 2>&1 | head -1"
test_tool "mount" "mount --version"

# ═══════════════════════════════════════════════════════════════════════════
# Network & Connectivity
# ═══════════════════════════════════════════════════════════════════════════

section_header "Network & Connectivity"
test_tool "ip" "ip -V"
test_tool "ping" "ping -V"
test_tool "curl" "curl --version"

# ═══════════════════════════════════════════════════════════════════════════
# Hardware Diagnostics
# ═══════════════════════════════════════════════════════════════════════════

section_header "Hardware Diagnostics"
test_tool "lspci" "lspci --version"
test_tool "lsusb" "lsusb --version"
test_tool "smartctl" "smartctl --version"
test_tool "hdparm" "hdparm -V"

# ═══════════════════════════════════════════════════════════════════════════
# Editors & Viewers
# ═══════════════════════════════════════════════════════════════════════════

section_header "Editors & Viewers"
test_tool "vim" "vim --version"
test_tool "less" "less --version"

# ═══════════════════════════════════════════════════════════════════════════
# System Utilities
# ═══════════════════════════════════════════════════════════════════════════

section_header "System Utilities"
test_tool "htop" "htop --version"
test_tool "grep" "grep --version"
test_tool "find" "find --version"

# ═══════════════════════════════════════════════════════════════════════════
# Report Results
# ═══════════════════════════════════════════════════════════════════════════

report_results 2
exit $?
