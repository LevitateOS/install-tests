#!/bin/sh
# Automated Login Validation
#
# Verifies that automated login works on the installed system.
# This tests that the harness can login and run commands.

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

scenario_header "Automated Login Validation"

info "Testing that automated login is functional"
echo

section_header "Login Tests"
test_command "User is root" "[ \$(id -u) -eq 0 ]"
test_command "Shell is functional" "echo 'test' | grep 'test'"
test_command "Environment variables set" "[ -n \"\$HOME\" ]"
test_command "Can run commands" "ls / >/dev/null"

report_results "Automated Login"
exit $?
