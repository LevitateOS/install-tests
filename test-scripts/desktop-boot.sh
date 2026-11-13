#!/bin/sh
# Desktop boot: the live desktop ISO boots into the installer, the same
# checks on every variant. No service failed, the image is the desktop
# system, its display manager is installed but not running (the live session
# leaves the desktop to the installed system), and the install screen holds
# tty1. The harness passes SCENARIO_DISPLAY_MANAGER (the contract's display
# manager), SCENARIO_INSTALL_SPLIT (the install screen's split pane, relative
# to /), SCENARIO_FLAVOUR_FIELD and SCENARIO_FLAVOUR (the image's os-release
# field and its value), SCENARIO_DESKTOP_APPS (the desktop apps the system
# carries, each as program:menu-entry-path, separated by spaces) and
# SCENARIO_DESKTOP_HELPERS (the programs the apps run, which have no menu
# entry and no window, separated by spaces) and SCENARIO_DISPLAY_SERVER (the
# display server the variant compiles from source, by its recipe's name, e.g.
# xlibre; empty when it comes from a package), and has already logged in over
# SSH as root.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

: "${SCENARIO_DISPLAY_MANAGER:?SCENARIO_DISPLAY_MANAGER is not set}"
: "${SCENARIO_INSTALL_SPLIT:?SCENARIO_INSTALL_SPLIT is not set}"
: "${SCENARIO_FLAVOUR_FIELD:?SCENARIO_FLAVOUR_FIELD is not set}"
: "${SCENARIO_FLAVOUR:?SCENARIO_FLAVOUR is not set}"
: "${SCENARIO_DESKTOP_APPS:?SCENARIO_DESKTOP_APPS is not set}"
: "${SCENARIO_DESKTOP_HELPERS:?SCENARIO_DESKTOP_HELPERS is not set}"

# The install screen's split pane running on tty1.
install_screen_on_tty1() {
    ps -t tty1 -o args= 2>/dev/null | grep -q "/$SCENARIO_INSTALL_SPLIT"
}

scenario_header "Desktop Boot"

# SSH answers before start-up has finished: wait up to 90 s for it and for
# the install screen.
waited=0
while [ "$waited" -lt 90 ]; do
    svc_boot_done && install_screen_on_tty1 && break
    sleep 2
    waited=$((waited + 2))
done

section_header "Boot"
test_command "Start-up finished" "svc_boot_done"
test_command "No service failed" "[ -z \"\$(svc_failed)\" ] || { svc_failed; false; }"
test_command "The image is the $SCENARIO_FLAVOUR system" "grep -qx '$SCENARIO_FLAVOUR_FIELD=$SCENARIO_FLAVOUR' /etc/os-release"

section_header "Installer"
test_command "$SCENARIO_DISPLAY_MANAGER is installed" "command -v $SCENARIO_DISPLAY_MANAGER"
test_command "$SCENARIO_DISPLAY_MANAGER is not running" "! svc_running $SCENARIO_DISPLAY_MANAGER"
test_command "The install screen holds tty1" "install_screen_on_tty1"

# The desktop apps (gui/): each is on PATH, has its menu entry where the
# desktop's menu looks, and starts. Asked for its version with Qt's offscreen
# platform it needs no display, but it does load the system's own Qt, so a
# program compiled against another Qt or C library fails here.
section_header "Desktop apps"
for app in $SCENARIO_DESKTOP_APPS; do
    program=${app%%:*}
    entry=${app#*:}
    test_command "$program is installed" "command -v $program"
    test_command "$program has its menu entry" "[ -f /$entry ]"
    test_command "$program starts" "QT_QPA_PLATFORM=offscreen $program --version | grep -q '^$program '"
done

# The helpers have no menu entry and no window: each is on PATH and answers
# --version, which needs no display and no agent program.
section_header "Desktop helpers"
for helper in $SCENARIO_DESKTOP_HELPERS; do
    test_command "$helper is installed" "command -v $helper"
    test_command "$helper answers --version" "$helper --version | grep -q '^$helper '"
done

# A display server compiled from source: the one the display manager would
# start is the one built, it runs far enough to name itself (so it links the
# system's libraries), and the drivers built against it are where it loads
# modules from. XLibre loads only modules built for its own ABI, in
# xlibre-25/ under the module path; an X.Org package would put them elsewhere.
case "${SCENARIO_DISPLAY_SERVER-}" in
    "") ;;
    xlibre)
        section_header "Display server (XLibre)"
        modules=/usr/lib/xorg/modules/xlibre-25
        test_command "/usr/bin/X runs the wrapper script" "[ \"\$(readlink /usr/bin/X)\" = Xorg ] && grep -q /usr/libexec/Xorg.wrap /usr/bin/Xorg"
        test_command "The wrapper is setuid root" "[ -u /usr/libexec/Xorg.wrap ] && [ \"\$(stat -c %u /usr/libexec/Xorg.wrap)\" = 0 ]"
        test_command "The X server is XLibre" "/usr/libexec/Xorg -version 2>&1 | grep -q '^XLibre X Server '"
        test_command "The libinput driver is installed" "[ -f $modules/input/libinput_drv.so ]"
        test_command "The modesetting driver is installed" "[ -f $modules/drivers/modesetting_drv.so ]"
        ;;
    *)
        section_header "Display server"
        test_command "a check for display server $SCENARIO_DISPLAY_SERVER exists" "false"
        ;;
esac

report_results "Desktop Boot"
