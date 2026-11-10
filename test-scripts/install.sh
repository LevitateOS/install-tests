#!/bin/sh
# Install: what "the install passed" means, checked on the mounted target in
# the live system. The harness passes the target recpart reported, the
# install mode, the variant's os id, the ESP directory recab puts the kernel
# in, recab's state file and its trial service link.

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
        test_file_exists "$TARGET/boot/loader/entries/$OS_ID.conf" "Boot entry"
        KERNEL_DIR="$TARGET/boot/${SCENARIO_KERNEL_DIR:?SCENARIO_KERNEL_DIR is not set}"
        test_file_exists "$KERNEL_DIR/vmlinuz" "Kernel on the ESP"
        test_file_exists "$KERNEL_DIR/initramfs.img" "Installed initramfs on the ESP"
        ;;
    ab)
        test_file_exists "$TARGET/boot/loader/entries/$OS_ID-a.conf" "Boot entry of slot A"
        test_file_exists "$TARGET/boot/loader/entries/$OS_ID-b.conf" "Boot entry of slot B"
        test_command "recab names slot A known-good" "grep -q '\"known_good_slot\": \"A\"' '$TARGET${SCENARIO_STATE_FILE:?SCENARIO_STATE_FILE is not set}'"
        for slot in a b; do
            entry="$TARGET/boot/loader/entries/$OS_ID-$slot.conf"
            kernel=$(awk '$1=="linux" {print $2}' "$entry")
            initrd=$(awk '$1=="initrd" {print $2}' "$entry")
            case "$kernel:$initrd" in
                /recab/objects/*.linux:/recab/objects/*.initrd)
                    test_file_exists "$TARGET/boot$kernel" "Generation kernel of slot $slot"
                    test_file_exists "$TARGET/boot$initrd" "Generation initramfs of slot $slot" ;;
                *) record_failed "slot $slot lacks its immutable boot objects" ;;
            esac
        done
        ready=$(recab --json status --root "$TARGET" | grep -o '"generation_ready":true' | wc -l)
        test_command "Both initial generations are ready" "test '$ready' -eq 2"
        test_command "recab's trial service is enabled" "test -L '$TARGET/${SCENARIO_TRIAL_LINK:?SCENARIO_TRIAL_LINK is not set}'"
        ;;
    *) record_failed "unknown install mode '$MODE'" ;;
esac

report_results "Install"
