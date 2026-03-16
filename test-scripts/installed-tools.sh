#!/bin/sh
# Installed Tools Validation
#
# Verifies that daily-driver tools are present and functional on the installed
# system, not the live ISO.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    . "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/stage-tests/common.sh" ]; then
    . "/usr/local/lib/stage-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    exit 1
fi

scenario_header "Installed Tools Validation"

info "Testing daily driver tools on installed system"
echo

section_header "Essential Tools"
test_tool "sudo" "sudo --version"
test_tool "ip" "ip -V"
test_tool "ssh" "ssh -V"
test_tool "mount" "mount --version"
test_tool "umount" "umount --version"
test_tool "dmesg" "dmesg --version || dmesg -h"

section_header "Shell & Core Utils"
test_tool "bash" "bash --version"
test_tool "ash" "ash -c 'echo test'" || test_tool "sh" "sh -c 'echo test'"

report_results "Installed Tools"
exit $?
