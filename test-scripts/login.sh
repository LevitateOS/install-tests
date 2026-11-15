#!/bin/sh
# Login: reclogin is the login screen of the installed desktop system. Runs
# on the installed system after a root login on the serial console; the
# harness runs it only when /usr/bin/reclogind exists (otherwise the
# scenario is skipped).
#   SCENARIO_LOGIN_CONSOLE  the console the login screen takes (7)
#   SCENARIO_TEXT_CONSOLE   the console that keeps its text login (1)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

: "${SCENARIO_LOGIN_CONSOLE:?SCENARIO_LOGIN_CONSOLE is not set}"
: "${SCENARIO_TEXT_CONSOLE:?SCENARIO_TEXT_CONSOLE is not set}"

RECLOGIND=/usr/bin/reclogind
GREETER=/usr/bin/reclogin-greeter

# The serial login can come before the login screen is up: wait up to 60
# seconds for the command $1, then run it once more for its output.
eventually() {
    i=0
    while [ "$i" -lt 60 ]; do
        eval "$1" >/dev/null 2>&1 && return 0
        sleep 1
        i=$((i + 1))
    done
    eval "$1"
}

# The pids whose program is $1 (an absolute path), one per line. By the
# program, not the command name: the kernel cuts names to 15 characters.
pids_of() {
    for d in /proc/[0-9]*; do
        [ "$(readlink "$d/exe" 2>/dev/null)" = "$1" ] && echo "${d#/proc/}"
    done
    return 0
}

# Every greeter runs with real and effective user id $1, and there is one.
greeters_run_as() {
    found=""
    for pid in $(pids_of "$GREETER"); do
        found=yes
        ids=$(awk '/^Uid:/ { print $2, $3 }' "/proc/$pid/status")
        [ "$ids" = "$1 $1" ] || { echo "greeter $pid runs as uid/euid $ids"; return 1; }
    done
    [ -n "$found" ]
}

# A text login (getty, agetty or login) has tty$1 as its input.
text_login_on() {
    for d in /proc/[0-9]*; do
        [ "$(readlink "$d/fd/0" 2>/dev/null)" = "/dev/tty$1" ] || continue
        case "$(cat "$d/comm" 2>/dev/null)" in
            getty|agetty|mingetty|login) return 0 ;;
        esac
    done
    echo "no getty or login reads /dev/tty$1"
    return 1
}

scenario_header "Login"

section_header "reclogin"
test_command "The account reclogin exists" "id -u reclogin"
test_command "The login screen's service is enabled" "svc_enabled reclogin"
test_command "The login screen's service runs" "eventually 'svc_running reclogin'"
test_command "reclogind runs" "eventually '[ -n \"\$(pids_of $RECLOGIND)\" ]'"
test_command "reclogin-greeter runs" "eventually '[ -n \"\$(pids_of $GREETER)\" ]'"
test_command "The greeter runs as reclogin" "greeters_run_as \"\$(id -u reclogin)\""
test_command "Console $SCENARIO_LOGIN_CONSOLE is the shown console" \
    "eventually '[ \"\$(cat /sys/class/tty/tty0/active)\" = tty$SCENARIO_LOGIN_CONSOLE ]'"

section_header "No SDDM"
test_command "No SDDM process" "! grep -qs '^sddm' /proc/[0-9]*/comm"
for path in /usr/bin/sddm /usr/bin/sddm-greeter /usr/bin/sddm-greeter-qt6 /usr/libexec/sddm-helper \
    /usr/lib/systemd/system/sddm.service /etc/init.d/sddm /etc/sddm.conf /etc/sddm.conf.d \
    /usr/share/sddm /var/lib/sddm; do
    test_command "No $path" "test ! -e $path"
done

section_header "Text logins"
test_command "Console $SCENARIO_TEXT_CONSOLE offers a text login" "eventually 'text_login_on $SCENARIO_TEXT_CONSOLE'"

report_results "Login"
