#!/bin/bash
# Stage 04: Installed Boot Validation
#
# Verifies that the system boots successfully from disk after installation.
# This runs on the INSTALLED system (not live ISO).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    source "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/stage-tests/common.sh" ]; then
    source "/usr/local/lib/stage-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    exit 1
fi

stage_header 4 "Installed Boot Validation"

info "If you can read this, the installed system booted successfully!"
echo

section_header "Boot Verification"
test_file_exists "/etc/fstab" "fstab present"
test_file_exists "/boot/EFI" "EFI partition mounted"
test_command "Root is writable" "touch /tmp/.stage-test && rm /tmp/.stage-test"
test_command "Init system running" "ps aux | grep -v grep | grep -q init"

report_results 4
exit $?
