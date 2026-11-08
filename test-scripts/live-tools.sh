#!/bin/sh
# Live Tools Validation
#
# Verifies the live environment around the tools: PATH, the block and PCI
# subsystems, kernel modules and the network interfaces. The harness runs
# every contract tool itself (the live-tools scenario).

set -euo pipefail
START_TIME="$(date +%s)"

# Find script directory and load common functions
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Try multiple locations for common.sh
if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    . "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/scenario-tests/common.sh" ]; then
    . "/usr/local/lib/scenario-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    echo "Tried:" >&2
    echo "  - $SCRIPT_DIR/lib/common.sh" >&2
    echo "  - /usr/local/lib/scenario-tests/common.sh" >&2
    exit 1
fi

# ═══════════════════════════════════════════════════════════════════════════
# Main Test
# ═══════════════════════════════════════════════════════════════════════════

scenario_header "Live Tools Validation"

# Every command the contract lists (distro/contract/capabilities.txt, the
# install tools and the privilege tool) is run by the harness before this
# script. This checks what the live environment adds around them.
section_header "Live Environment"
test_command "PATH includes /usr/local/bin" "echo \"\$PATH\" | tr ':' '\n' | grep -qx '/usr/local/bin'"
test_command "block subsystem visible" "test -d /sys/class/block && ls /sys/class/block >/dev/null"
test_command "kernel modules installed for the running kernel" "test -f /lib/modules/\$(uname -r)/modules.dep"
test_command "a modular driver loads (iwlwifi, with cfg80211)" "modprobe iwlwifi && grep -q '^iwlwifi ' /proc/modules"
test_command "network interfaces discoverable" "ip -brief link >/dev/null"
test_command "PCI inventory probe" "lspci -mm >/dev/null"

# ═══════════════════════════════════════════════════════════════════════════
# Report Results
# ═══════════════════════════════════════════════════════════════════════════

report_results "Live Tools"
RESULT=$?
END_TIME="$(date +%s)"
ELAPSED="$((END_TIME - START_TIME))"
info "Live-tools smoke duration: ${ELAPSED}s"
exit $RESULT
