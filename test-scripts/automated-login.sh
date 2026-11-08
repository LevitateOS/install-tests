#!/bin/sh
# Automated login: the harness logged in as root on the serial getty and
# runs commands.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

scenario_header "Automated Login"

section_header "Login"
test_command "User is root" "[ \$(id -u) -eq 0 ]"
test_command "HOME is set" "[ -n \"\$HOME\" ]"
test_command "Commands run" "ls / >/dev/null"

report_results "Automated Login"
