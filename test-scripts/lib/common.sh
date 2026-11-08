# Common functions for the scenario scripts: checks that record their result
# and carry on, and one report at the end.
#
# Portable sh: the subset of bash and busybox ash both run (no `echo -e`, no
# `pipefail`; `local` both have). Scripts do not run under `set -e`: every
# check records its result, and the report lists every failure.
#
# The harness puts this file at lib/common.sh beside the script, the init
# backend at lib/init/<init>.sh, and passes the facts as environment:
#   SCENARIO_PASS_MARKER  printed when every check passed (required)
#   SCENARIO_INIT         the init system (systemd, openrc): picks the backend

: "${SCENARIO_PASS_MARKER:?SCENARIO_PASS_MARKER is not set}"
: "${SCENARIO_INIT:?SCENARIO_INIT is not set}"
. "$(dirname "$0")/lib/init/${SCENARIO_INIT}.sh"

PASSED_COUNT=0
FAILED_COUNT=0
FAILED_LIST=""

record_failed() {
    FAILED_COUNT=$((FAILED_COUNT + 1))
    FAILED_LIST="${FAILED_LIST}  - $1
"
}

# Run a check: test_command "description" "command". Its output is shown
# only when it fails.
test_command() {
    local description=$1
    local cmd=$2
    local output
    local code
    output=$(eval "$cmd" 2>&1)
    code=$?
    if [ "$code" -eq 0 ]; then
        printf '  [ok]   %s\n' "$description"
        PASSED_COUNT=$((PASSED_COUNT + 1))
    else
        printf '  [FAIL] %s (exit %s)\n' "$description" "$code"
        [ -n "$output" ] && printf '%s\n' "$output" | head -5 | sed 's/^/         /'
        record_failed "$description (exit $code)"
    fi
    return 0
}

# test_file_exists "/path" "description"
test_file_exists() {
    test_command "$2" "test -e '$1'"
}

# Print the results and the pass marker when nothing failed; the exit status
# is the number of failed checks (0 when all passed).
report_results() {
    local label=$1
    echo
    printf '%s: %s passed, %s failed\n' "$label" "$PASSED_COUNT" "$FAILED_COUNT"
    if [ "$FAILED_COUNT" -eq 0 ]; then
        printf '%s\n' "$SCENARIO_PASS_MARKER"
        return 0
    fi
    printf '%s checks failed:\n%s' "$FAILED_COUNT" "$FAILED_LIST"
    return 1
}

scenario_header() {
    printf '\n== %s ==\n' "$1"
}

section_header() {
    printf '\n%s:\n' "$1"
}
