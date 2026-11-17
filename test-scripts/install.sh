#!/bin/sh
# Install: what "the install passed" means, checked on the mounted target in
# the live system. The harness passes the target recpart reported, the
# install mode, the variant's os id, the ESP directory recab puts the kernel
# in (mutable), recab's installation record, its recab-started service link,
# the system's flavour (desktop: the release medium installed the default
# software set through Recipe), the receipt Recipe keeps in the root and the
# recab desktop app's files.

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

if [ "${SCENARIO_FLAVOUR:?SCENARIO_FLAVOUR is not set}" = desktop ]; then
    section_header "Default software set (Recipe)"
    RECEIPT="$TARGET/${SCENARIO_RECIPE_RECEIPT:?SCENARIO_RECIPE_RECEIPT is not set}"
    test_file_exists "$RECEIPT" "Recipe's system receipt"
    for recipe in ${SCENARIO_DEFAULT_SET:?SCENARIO_DEFAULT_SET is not set}; do
        test_command "$recipe is in the receipt" "grep -q '\"$recipe\"' '$RECEIPT'"
    done
    test_command "No package cache in the installed system" "test ! -e '$TARGET${SCENARIO_RECIPE_CACHE:?SCENARIO_RECIPE_CACHE is not set}'"
    test_command "No default-set medium tree in the installed system" "test ! -e '$TARGET${SCENARIO_DEFAULT_SET_DIR:?SCENARIO_DEFAULT_SET_DIR is not set}'"
fi

section_header "Boot"
case "$(uname -m)" in
    x86_64) EFI_BOOT=BOOTX64.EFI ;;
    *) record_failed "unsupported guest architecture"; EFI_BOOT=unsupported ;;
esac
test_file_exists "$TARGET/boot/EFI/BOOT/$EFI_BOOT" "Boot loader installed"
test_file_exists "$TARGET/boot/loader/loader.conf" "systemd-boot loader config"
case "$MODE" in
    mutable)
        test_file_exists "$TARGET/boot/loader/entries/$OS_ID.conf" "Boot entry"
        KERNEL_DIR="$TARGET/boot/${SCENARIO_KERNEL_DIR:?SCENARIO_KERNEL_DIR is not set}"
        test_file_exists "$KERNEL_DIR/vmlinuz" "Kernel on the ESP"
        test_file_exists "$KERNEL_DIR/initramfs.img" "Installed initramfs on the ESP"
        test_command "No recab on a mutable install" "test ! -e '$TARGET/usr/bin/recab'"
        test_command "No recab-started service on a mutable install" "test ! -e '$TARGET/usr/lib/systemd/system/recab-started.service' && test ! -e '$TARGET/etc/init.d/recab-started'"
        test_command "No /usr/lib/recab on a mutable install" "test ! -e '$TARGET/usr/lib/recab'"
        for file in ${SCENARIO_AB_DESKTOP_FILES:?SCENARIO_AB_DESKTOP_FILES is not set}; do
            test_command "No /$file (the recab app) on a mutable install" "test ! -e '$TARGET/$file'"
        done
        if [ "$SCENARIO_FLAVOUR" = desktop ]; then
            for file in ${SCENARIO_MUTABLE_DESKTOP_FILES:?SCENARIO_MUTABLE_DESKTOP_FILES is not set}; do
                test_file_exists "$TARGET/$file" "/$file (the Updater app) on a mutable install"
            done
        fi
        test_command "The ESP is mounted at /boot by fstab" "awk '\$2==\"/boot\" && \$3==\"vfat\" {ok=1} END{exit(ok?0:1)}' '$TARGET/etc/fstab'"
        ;;
    ab)
        INSTALLATION_FILE="${SCENARIO_INSTALLATION_FILE:?SCENARIO_INSTALLATION_FILE is not set}"
        STARTED_LINK="${SCENARIO_STARTED_LINK:?SCENARIO_STARTED_LINK is not set}"
        test_command "The A/B installation verifies" "recab --json verify-installation --root '$TARGET'"
        for slot in a b; do
            test_file_exists "$TARGET/boot/loader/entries/$OS_ID-$slot.conf" "Boot entry of root $slot"
            test_file_exists "$TARGET/boot/$OS_ID-$slot/vmlinuz" "Kernel of root $slot on the ESP"
            test_file_exists "$TARGET/boot/$OS_ID-$slot/initramfs.img" "Installed initramfs of root $slot on the ESP"
        done
        test_command "Root A is the boot default" "grep -qx 'default $OS_ID-a.conf' '$TARGET/boot/loader/loader.conf'"
        test_file_exists "$TARGET$INSTALLATION_FILE" "recab's installation record"
        test_file_exists "$TARGET/usr/bin/recab" "recab is in root A"
        test_command "recab-started is enabled in root A" "test -L '$TARGET/$STARTED_LINK'"
        test_file_exists "$TARGET/usr/lib/recab/immutable" "Root A's slot marker"
        for file in ${SCENARIO_MUTABLE_DESKTOP_FILES:?SCENARIO_MUTABLE_DESKTOP_FILES is not set}; do
            test_command "No /$file (the Updater app) on an A/B install" "test ! -e '$TARGET/$file'"
        done
        if [ "$SCENARIO_FLAVOUR" = desktop ]; then
            for file in ${SCENARIO_AB_DESKTOP_FILES:?SCENARIO_AB_DESKTOP_FILES is not set}; do
                test_file_exists "$TARGET/$file" "/$file (the recab app) in root A"
            done
        fi
        # Root B is a copy of root A: mount it read-only beside A and compare.
        ROOT_B=$(tr -d ' \n' < "$TARGET$INSTALLATION_FILE" | sed -n 's/.*"roots":\["[^"]*","\([^"]*\)".*/\1/p')
        B_MOUNT=/run/scenario-root-b
        if [ -n "$ROOT_B" ] && mkdir -p "$B_MOUNT" && mount -o ro "/dev/disk/by-partuuid/$ROOT_B" "$B_MOUNT"; then
            recab --json compare --roots "$TARGET" "$B_MOUNT" > /tmp/scenario-compare.json
            test_command "Root B is a copy of root A" "grep -Eq '\"differences\": *\[\]' /tmp/scenario-compare.json && grep -Eq '\"more\": *0' /tmp/scenario-compare.json"
            test_command "recab-started is enabled in root B" "test -L '$B_MOUNT/$STARTED_LINK'"
            umount "$B_MOUNT"
        else
            record_failed "root B (PARTUUID '$ROOT_B') could not be mounted to compare it with root A"
        fi
        ;;
    *) record_failed "unknown install mode '$MODE'" ;;
esac

report_results "Install"
