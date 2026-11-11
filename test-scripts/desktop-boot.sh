#!/bin/sh
# Desktop boot: the live desktop came up, the same checks on every variant.
# The display manager runs and has logged the live user into a graphical
# session, no service failed, and the image is the desktop system. The
# harness passes SCENARIO_DISPLAY_MANAGER (the contract's display manager),
# SCENARIO_LIVE_USER (whom it logs in), and SCENARIO_FLAVOUR_FIELD and
# SCENARIO_FLAVOUR (the image's os-release field and its value), and
# SCENARIO_DESKTOP_APPS (the desktop apps the system carries, each as
# program:menu-entry-path, separated by spaces), SCENARIO_DESKTOP_HELPERS (the
# programs the apps run, which have no menu entry and no window, separated by
# spaces), and has already logged in
# over SSH as root.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

: "${SCENARIO_DISPLAY_MANAGER:?SCENARIO_DISPLAY_MANAGER is not set}"
: "${SCENARIO_LIVE_USER:?SCENARIO_LIVE_USER is not set}"
: "${SCENARIO_FLAVOUR_FIELD:?SCENARIO_FLAVOUR_FIELD is not set}"
: "${SCENARIO_FLAVOUR:?SCENARIO_FLAVOUR is not set}"
: "${SCENARIO_DESKTOP_APPS:?SCENARIO_DESKTOP_APPS is not set}"
: "${SCENARIO_DESKTOP_HELPERS:?SCENARIO_DESKTOP_HELPERS is not set}"

# The id of the live user's session, once logind (elogind on OpenRC) has it.
live_session() {
    loginctl list-sessions --no-legend 2>/dev/null |
        awk -v user="$SCENARIO_LIVE_USER" '$3 == user { print $1; exit }'
}

scenario_header "Desktop Boot"

# SSH answers before the display manager has logged the user in: wait up to
# 90 s for start-up to finish and the session to appear.
waited=0
while [ "$waited" -lt 90 ]; do
    svc_boot_done && [ -n "$(live_session)" ] && break
    sleep 2
    waited=$((waited + 2))
done

section_header "Boot"
test_command "Start-up finished" "svc_boot_done"
test_command "No service failed" "[ -z \"\$(svc_failed)\" ] || { svc_failed; false; }"
test_command "The image is the $SCENARIO_FLAVOUR system" "grep -qx '$SCENARIO_FLAVOUR_FIELD=$SCENARIO_FLAVOUR' /etc/os-release"

section_header "Desktop"
test_command "$SCENARIO_DISPLAY_MANAGER is running" "svc_running $SCENARIO_DISPLAY_MANAGER"
test_command "$SCENARIO_LIVE_USER has a session" "[ -n \"\$(live_session)\" ]"
test_command "The session is graphical (x11 or wayland)" \
    "case \"\$(loginctl show-session \"\$(live_session)\" -p Type --value)\" in x11|wayland) true ;; *) false ;; esac"
test_command "$SCENARIO_LIVE_USER's session runs processes" "pgrep -u $SCENARIO_LIVE_USER >/dev/null"

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

report_results "Desktop Boot"
