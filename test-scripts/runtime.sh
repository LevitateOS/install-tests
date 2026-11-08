#!/bin/sh
# Runtime Validation (the runtime scenario)
#
# Checks the installed system's privilege policy. The harness runs every
# contract tool on the installed system itself (the runtime scenario).

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

scenario_header "Runtime Validation"

# The harness runs every contract tool, the privilege tool among them. This
# checks the privilege tool's policy, by the profile's REC_PRIVILEGE fact.
section_header "Privilege Policy"
. /etc/os-release
case "${REC_PRIVILEGE:-}" in
    sudo) test_command "sudo policy syntax" "visudo -c" ;;
    doas) test_command "doas policy syntax" "doas -C /etc/doas.conf" ;;
    *)
        echo "ERROR: /etc/os-release names no known REC_PRIVILEGE ('${REC_PRIVILEGE:-}')" >&2
        exit 1
        ;;
esac

report_results "Runtime"
exit $?
