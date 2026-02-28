#!/bin/bash
# Stage 02: Live Tools Validation
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
START_TIME="$(date +%s)"

# Find script directory and load common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Try multiple locations for common.sh
if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    source "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/stage-tests/common.sh" ]; then
    source "/usr/local/lib/stage-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    echo "Tried:" >&2
    echo "  - $SCRIPT_DIR/lib/common.sh" >&2
    echo "  - /usr/local/lib/stage-tests/common.sh" >&2
    exit 1
fi

# ═══════════════════════════════════════════════════════════════════════════
# Main Test
# ═══════════════════════════════════════════════════════════════════════════

stage_header 2 "Live Tools Validation"

info "This stage verifies that all daily driver tools are present"
info "and FUNCTIONAL (actually executes them, not just checks existence)"
echo

detect_expected_install_experience() {
    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        . /etc/os-release
    else
        ID=""
    fi

    case "${ID:-}" in
        levitateos|acornos) printf '%s\n' "ux" ;;
        ralphos|iuppiteros) printf '%s\n' "automated_ssh" ;;
        *)
            # Conservative fallback: default to headless automation profile.
            printf '%s\n' "automated_ssh"
            ;;
    esac
}

EXPECTED_INSTALL_EXPERIENCE="$(detect_expected_install_experience)"

# ═══════════════════════════════════════════════════════════════════════════
# Core Installation Tools
# ═══════════════════════════════════════════════════════════════════════════

section_header "Core Installation Tools"
test_command "PATH includes /usr/local/bin" "echo \"\$PATH\" | tr ':' '\n' | grep -qx '/usr/local/bin'"
test_file_exists "/usr/local/bin/stage-02-live-tools.sh" "stage-02 test script installed"
test_tool "recstrap" "recstrap --help"
test_tool "recfstab" "recfstab --help"
test_tool "recchroot" "recchroot --help"
test_tool "sfdisk" "sfdisk --version"
test_tool "fdisk" "fdisk --version"
test_tool "lsblk" "lsblk --version"
test_tool "blkid" "blkid --version"
test_tool "wipefs" "wipefs --version"
test_tool "mkfs.ext4" "mkfs.ext4 -V 2>&1 | head -1"
test_tool "mount" "mount --version"
test_command "block subsystem visible" "test -d /sys/class/block && ls /sys/class/block >/dev/null"
test_file_exists "/usr/lib/levitate/stage-02/install-experience" "stage-02 install-experience marker exists"
test_command "stage-02 install-experience matches distro policy" "test \"\$(tr -d '\\n' < /usr/lib/levitate/stage-02/install-experience)\" = \"$EXPECTED_INSTALL_EXPERIENCE\""
test_command "stage-02 install entrypoint script is executable" "test -x /usr/local/bin/stage-02-install-entrypoint"
if [ "$EXPECTED_INSTALL_EXPERIENCE" = "ux" ]; then
    test_file_exists "/etc/profile.d/30-stage-02-install-ux.sh" "stage-02 UX profile hook exists"
    test_command "stage-02 helper probe selects split launcher" "/usr/local/bin/stage-02-install-entrypoint --probe | sed -n 's/^stage02-entrypoint-helper=//p' | head -n1 | grep -E -q '(^|/)levitate-install-docs-split$'"
    test_command "stage-02 split-pane smoke launch works" "STAGE02_ENTRYPOINT_SMOKE=1 /usr/local/bin/stage-02-install-entrypoint | grep -q 'split-smoke:ok'"
fi

# ═══════════════════════════════════════════════════════════════════════════
# Network & Connectivity
# ═══════════════════════════════════════════════════════════════════════════

section_header "Network & Connectivity"
test_tool "ip" "ip -V"
test_tool "ping" "ping -V"
test_tool "curl" "curl --version"
test_command "network interfaces discoverable" "ip -brief link >/dev/null"

# ═══════════════════════════════════════════════════════════════════════════
# Hardware Diagnostics
# ═══════════════════════════════════════════════════════════════════════════

section_header "Hardware Diagnostics"
test_tool "lspci" "lspci --version"
test_tool "lsusb" "lsusb --version"
test_tool "smartctl" "smartctl --version"
test_tool "hdparm" "hdparm -V"
test_command "PCI inventory probe" "lspci -mm >/dev/null"

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
test_tool "awk" "awk --version"
test_tool "sed" "sed --version"
test_command "shell can execute toolbox binaries" "command -v recstrap recfstab recchroot >/dev/null"

# ═══════════════════════════════════════════════════════════════════════════
# Report Results
# ═══════════════════════════════════════════════════════════════════════════

report_results 2
RESULT=$?
END_TIME="$(date +%s)"
ELAPSED="$((END_TIME - START_TIME))"
info "Stage 02 smoke duration: ${ELAPSED}s"
exit $RESULT
