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

section_header "Installed System Separation"
test_command "Recipe setup program is installed" "test -x /usr/libexec/recipe-system-setup"
test_command "Live helper is absent" "test ! -e /usr/local/bin/live-help"
test_command "Live shell profile is absent" "test ! -e /etc/profile.d/20-live-shell.sh"
test_command "Live serial test profile is absent" "test ! -e /etc/profile.d/99-serial-test.sh"
test_command "Live serial autologin is absent" "test ! -e /usr/local/bin/serial-autologin"
test_command "Installed consoles authenticate" "if [ -f /etc/inittab ]; then ! grep -E -- 'serial-autologin|-n -l|--autologin' /etc/inittab; else test ! -e /etc/systemd/system/getty@.service.d/zz-autologin.conf; fi"
test_command "No empty root password" "! grep -q '^root::' /etc/shadow"

report_results "Installed Boot"
exit $?
