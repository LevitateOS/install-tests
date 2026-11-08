#!/bin/sh
# Install: what "the install passed" means, checked on the mounted target in
# the live system. The harness passes the target recpart reported, the
# install mode, the variant's os id, the installed initramfs name (empty when
# the installed system boots without one) and recab's trial service link.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

TARGET="${SCENARIO_INSTALL_TARGET:?SCENARIO_INSTALL_TARGET is not set}"
MODE="${SCENARIO_INSTALL_MODE:?SCENARIO_INSTALL_MODE is not set}"
OS_ID="${SCENARIO_OS_ID:?SCENARIO_OS_ID is not set}"

scenario_header "Install"

section_header "Root filesystem"
test_file_exists "$TARGET/bin" "Root filesystem extracted"
test_file_exists "$TARGET/etc/os-release" "os-release present"
test_command "fstab has a root entry" "awk '\$2==\"/\" {ok=1} END{exit(ok?0:1)}' '$TARGET/etc/fstab'"

section_header "Boot"
test_file_exists "$TARGET/boot/EFI/BOOT/BOOTX64.EFI" "Boot loader installed"
test_file_exists "$TARGET/boot/loader/loader.conf" "systemd-boot loader config"
case "$MODE" in
    mutable)
        KERNEL_DIR="$TARGET/boot"
        test_file_exists "$TARGET/boot/loader/entries/$OS_ID.conf" "Boot entry"
        ;;
    ab)
        KERNEL_DIR="$TARGET/boot/$OS_ID-a"
        test_file_exists "$TARGET/boot/loader/entries/$OS_ID-a.conf" "Boot entry of slot A"
        test_file_exists "$TARGET/boot/loader/entries/$OS_ID-b.conf" "Boot entry of slot B"
        test_command "recab names slot A known-good" "grep -q '\"known_good_slot\": \"A\"' '$TARGET/var/lib/recab/state.json'"
        test_command "recab's trial service is enabled" "test -L '$TARGET/${SCENARIO_TRIAL_LINK:?SCENARIO_TRIAL_LINK is not set}'"
        ;;
    *) record_failed "unknown install mode '$MODE'" ;;
esac
test_file_exists "$KERNEL_DIR/vmlinuz" "Kernel on the ESP"
if [ -n "${SCENARIO_INITRAMFS:-}" ]; then
    test_file_exists "$KERNEL_DIR/initramfs.img" "Installed initramfs on the ESP"
fi

report_results "Install"
