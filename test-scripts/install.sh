#!/bin/sh
# Install Validation
#
# Verifies that the installation process completes successfully.
# This runs after performing a scripted installation to disk.

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

scenario_header "Install Validation"

info "This scenario verifies the installation completed successfully"
echo

section_header "Installation Artifacts"
test_file_exists "/mnt/sysroot/bin" "Root filesystem extracted"
test_file_exists "/mnt/sysroot/boot/EFI" "EFI boot partition"
test_file_exists "/mnt/sysroot/etc/fstab" "fstab generated"

section_header "Bootloader"
test_command "Bootloader installed" "ls /mnt/sysroot/boot/EFI/BOOT/BOOTX64.EFI"

report_results "Install"
exit $?
