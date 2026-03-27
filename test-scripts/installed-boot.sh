#!/bin/sh
# Installed Boot Validation
#
# Verifies that the system boots successfully from disk after installation.
# This runs on the installed system, not the live ISO.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    . "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/scenario-tests/common.sh" ]; then
    . "/usr/local/lib/scenario-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    exit 1
fi

scenario_header "Installed Boot Validation"

info "If you can read this, the installed system booted successfully!"
echo

section_header "Boot Verification"
test_file_exists "/etc/fstab" "fstab present"
test_file_exists "/boot/EFI" "EFI partition mounted"
test_command "Root is writable" "touch /tmp/.scenario-test && rm /tmp/.scenario-test"
test_command "Init system running" "ps aux | grep -v grep | grep -q init"

report_results "Installed Boot"
exit $?
