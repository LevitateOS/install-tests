#!/bin/sh
# Installed boot: the installed system came up healthy and is not a live
# system. Runs on the installed system after a root login.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

scenario_header "Installed Boot"

section_header "Boot"
test_command "Start-up finished" "svc_boot_done"
test_command "No service failed" "[ -z \"\$(svc_failed)\" ] || { svc_failed; false; }"
test_command "PID 1 is the init system" "[ \"\$(cat /proc/1/comm)\" = \"$(svc_pid1)\" ]"
test_file_exists /etc/fstab "fstab present"
test_command "The ESP is mounted at /boot" "findmnt -n /boot >/dev/null"
test_command "Temporary state is writable" "touch /tmp/.scenario-test && rm /tmp/.scenario-test"

if [ -f /usr/lib/recab/immutable ]; then
    section_header "Immutable generation"
    test_command "Already committed generation and sealed boot set verify" "recab --json status | grep -q '\"committed\":true' && recab --json commit"
    test_command "Root mount is read-only" "findmnt -n -o OPTIONS / | tr ',' '\\n' | grep -qx ro"
    test_command "Root device is protected" '[ "$(blockdev --getro "$(findmnt -n -o SOURCE /)")" = 1 ]'
    test_command "Configuration uses the immutable root" '[ "$(findmnt -n -T /etc -o TARGET)" = / ]'
    test_command "System writes are refused" 'if (printf forbidden > /etc/.scenario-refused) 2>/dev/null; then false; else true; fi'
fi

section_header "Installed System Separation"
test_command "Recipe setup program is installed" "test -x /usr/libexec/recipe-system-setup"
test_command "Live helper is absent" "test ! -e /usr/local/bin/live-help"
test_command "Live shell profile is absent" "test ! -e /etc/profile.d/20-live-shell.sh"
test_command "Live serial test profile is absent" "test ! -e /etc/profile.d/99-serial-test.sh"
test_command "Live serial autologin is absent" "test ! -e /usr/local/bin/serial-autologin"
test_command "Consoles ask for a password" "! svc_autologin"
test_command "No empty root password" "! grep -q '^root::' /etc/shadow"

report_results "Installed Boot"
