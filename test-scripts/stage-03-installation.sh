#!/bin/bash
# Stage 03: Installation Validation
#
# Verifies that the installation process completes successfully.
# This runs after performing a scripted installation to disk.

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

stage_header 3 "Installation Validation"

info "This stage verifies the installation completed successfully"
echo

section_header "Installation Artifacts"
test_file_exists "/mnt/sysroot/bin" "Root filesystem extracted"
test_file_exists "/mnt/sysroot/boot/EFI" "EFI boot partition"
test_file_exists "/mnt/sysroot/etc/fstab" "fstab generated"

section_header "Bootloader"
test_command "Bootloader installed" "ls /mnt/sysroot/boot/EFI/BOOT/BOOTX64.EFI"

report_results 3
exit $?
