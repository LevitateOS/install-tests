#!/bin/bash
# Checkpoint 1: Live Boot Validation
#
# Verifies that the live ISO boots successfully and reaches a usable shell.
# This is the simplest checkpoint - if this script runs, the boot succeeded!

set -euo pipefail

# Find script directory and load common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    source "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/checkpoint-tests/common.sh" ]; then
    source "/usr/local/lib/checkpoint-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    exit 1
fi

# ═══════════════════════════════════════════════════════════════════════════
# Main Test
# ═══════════════════════════════════════════════════════════════════════════

checkpoint_header 1 "Live Boot Validation"

info "If you can read this, the live ISO booted successfully!"
echo

# Basic sanity checks
section_header "Basic System Checks"
test_file_exists "/proc/cmdline" "Kernel command line"
test_file_exists "/sys" "/sys filesystem"
test_file_exists "/dev" "/dev filesystem"
test_command "Shell is functional" "echo 'test' | grep 'test'"
test_command "Root filesystem is writable" "touch /tmp/.checkpoint-test && rm /tmp/.checkpoint-test"

# Report results
report_results 1
exit $?
