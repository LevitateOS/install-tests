#!/bin/sh
# Desktop boot: the live desktop came up, the same checks on every variant.
# The display manager runs and has logged the live user into a graphical
# session, no service failed, and the image is the desktop system. The
# harness passes SCENARIO_DISPLAY_MANAGER (the contract's display manager),
# SCENARIO_LIVE_USER (whom it logs in), and SCENARIO_FLAVOUR_FIELD and
# SCENARIO_FLAVOUR (the image's os-release field and its value), and has
# already logged in over SSH as root.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

: "${SCENARIO_DISPLAY_MANAGER:?SCENARIO_DISPLAY_MANAGER is not set}"
: "${SCENARIO_LIVE_USER:?SCENARIO_LIVE_USER is not set}"
: "${SCENARIO_FLAVOUR_FIELD:?SCENARIO_FLAVOUR_FIELD is not set}"
: "${SCENARIO_FLAVOUR:?SCENARIO_FLAVOUR is not set}"

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

report_results "Desktop Boot"
