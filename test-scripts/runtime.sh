#!/bin/sh
# Runtime: the installed system's privilege policy. The harness runs every
# contract tool itself (the runtime scenario), and passes the privilege
# tool (SCENARIO_PRIVILEGE) and its configuration file.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

scenario_header "Runtime"

section_header "Privilege Policy"
case "${SCENARIO_PRIVILEGE:?SCENARIO_PRIVILEGE is not set}" in
    sudo) test_command "sudo policy syntax" "visudo -c" ;;
    doas) test_command "doas policy syntax" "doas -C '${SCENARIO_PRIVILEGE_CONFIG:?}'" ;;
    *) record_failed "unknown privilege tool '$SCENARIO_PRIVILEGE'" ;;
esac

report_results "Runtime"
