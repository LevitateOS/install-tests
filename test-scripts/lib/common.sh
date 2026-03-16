#!/bin/sh
# Common functions for scenario test scripts.
#
# This library provides shared testing infrastructure used by the live/install
# validation scripts. It handles test execution, result tracking, and reporting.

# Colors (if terminal supports it)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    CYAN='\033[0;36m'
    BOLD='\033[1m'
    NC='\033[0m' # No Color
else
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    CYAN=''
    BOLD=''
    NC=''
fi

# Test state (POSIX-compatible; newline-delimited lists)
PASSED_COUNT=0
FAILED_COUNT=0
BROKEN_COUNT=0
PASSED_LIST=""
FAILED_LIST=""
BROKEN_LIST=""

append_list() {
    list_name=$1
    value=$2
    eval "current=\${$list_name}"
    if [ -n "$current" ]; then
        current="$current
$value"
    else
        current="$value"
    fi
    eval "$list_name=\$current"
}

record_passed() {
    PASSED_COUNT=$((PASSED_COUNT + 1))
    append_list PASSED_LIST "$1"
}

record_failed() {
    FAILED_COUNT=$((FAILED_COUNT + 1))
    append_list FAILED_LIST "$1"
}

record_broken() {
    BROKEN_COUNT=$((BROKEN_COUNT + 1))
    append_list BROKEN_LIST "$1:$2"
}

# Test a tool by running a command
# Usage: test_tool "vim" "vim --version"
#
# Exit codes:
#   0 = Tool works (command succeeded)
#   127 = Tool not found (command not found)
#   Other = Tool exists but is broken (command failed)
test_tool() {
    local tool=$1
    local cmd=$2
    local exit_code
    local output

    echo -ne "  ${BLUE}[TEST]${NC} ${tool}... "

    # Run probe command without letting `set -e` abort the whole scenario script.
    set +e
    output=$(eval "$cmd" 2>&1)
    exit_code=$?
    set -e

    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}✓${NC}"
        record_passed "$tool"
        return 0
    elif [ $exit_code -eq 127 ]; then
        echo -e "${RED}✗ NOT FOUND${NC}"
        record_failed "$tool"
        return 1
    else
        # Extract first line of error for display
        local error_msg=$(echo "$output" | head -1 | cut -c1-60)
        echo -e "${YELLOW}✗ EXIT $exit_code${NC}"
        if [ -n "$error_msg" ]; then
            echo -e "    ${YELLOW}└─${NC} $error_msg"
        fi
        record_broken "$tool" "$exit_code"
        return 1
    fi
}

# Test that a file or directory exists
# Usage: test_file_exists "/path/to/file" "description"
test_file_exists() {
    local path=$1
    local description=$2

    echo -ne "  ${BLUE}[TEST]${NC} ${description}... "

    if [ -e "$path" ]; then
        echo -e "${GREEN}✓${NC} ($path)"
        record_passed "$description"
        return 0
    else
        echo -e "${RED}✗ NOT FOUND${NC}"
        record_failed "$description"
        return 1
    fi
}

# Test that a command succeeds
# Usage: test_command "description" "command to run"
test_command() {
    local description=$1
    local cmd=$2
    local exit_code
    local output

    echo -ne "  ${BLUE}[TEST]${NC} ${description}... "

    # Run probe command without letting `set -e` abort the whole scenario script.
    set +e
    output=$(eval "$cmd" 2>&1)
    exit_code=$?
    set -e

    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}✓${NC}"
        record_passed "$description"
        return 0
    else
        local error_msg=$(echo "$output" | head -1 | cut -c1-60)
        echo -e "${RED}✗ EXIT $exit_code${NC}"
        if [ -n "$error_msg" ]; then
            echo -e "    ${RED}└─${NC} $error_msg"
        fi
        record_failed "$description"
        return 1
    fi
}

# Normalize a scenario label into marker-friendly uppercase tokens.
scenario_marker_tokens() {
    local label=$1
    label=$(printf '%s' "$label" | sed \
        -e 's/[[:space:]]\+Validation$//' \
        -e 's/[[:space:]]\+Results$//')
    printf '%s\n' "$label" \
        | tr '[:lower:]-/' '[:upper:]  ' \
        | tr -cs '[:upper:][:digit:]' ' ' \
        | sed -e 's/^ *//' -e 's/ *$//' -e 's/  */ /g'
}

default_pass_marker() {
    local tokens
    tokens=$(scenario_marker_tokens "$1")
    printf '%s PASSED\n' "$tokens"
}

default_fail_marker() {
    local tokens
    tokens=$(scenario_marker_tokens "$1")
    printf '%s FAILED\n' "$tokens"
}

# Report final results
# Usage: report_results <scenario_name> [pass_marker] [fail_marker]
report_results() {
    local label=$1
    local pass_marker=${2:-${SCENARIO_PASS_MARKER:-}}
    local fail_marker=${3:-${SCENARIO_FAIL_MARKER:-}}

    local total_tests=$((PASSED_COUNT + FAILED_COUNT + BROKEN_COUNT))

    [ -n "$pass_marker" ] || pass_marker=$(default_pass_marker "$label")
    [ -n "$fail_marker" ] || fail_marker=$(default_fail_marker "$label")

    echo
    echo "═══════════════════════════════════════════════════════════"
    echo -e "  ${BOLD}${label} Results${NC}"
    echo "═══════════════════════════════════════════════════════════"
    echo -e "${GREEN}Passed:${NC} ${PASSED_COUNT}/$total_tests tests"

    if [ "${FAILED_COUNT}" -gt 0 ]; then
        echo
        echo -e "${RED}Missing (not in PATH):${NC} ${FAILED_COUNT} tests"
        printf '%s\n' "$FAILED_LIST" | while IFS= read -r tool; do
            [ -n "$tool" ] && echo "  • $tool"
        done
    fi

    if [ "${BROKEN_COUNT}" -gt 0 ]; then
        echo
        echo -e "${YELLOW}Broken (exist but failed):${NC} ${BROKEN_COUNT} tests"
        printf '%s\n' "$BROKEN_LIST" | while IFS= read -r item; do
            [ -n "$item" ] || continue
            local tool="${item%%:*}"
            local code="${item##*:}"
            echo "  • $tool (exit $code)"
        done
    fi

    echo "═══════════════════════════════════════════════════════════"

    if [ "${FAILED_COUNT}" -eq 0 ] && [ "${BROKEN_COUNT}" -eq 0 ]; then
        echo -e "${GREEN}${BOLD}✓ ${pass_marker}${NC}"
        echo
        echo "All tools are present and functional in this environment."
        return 0
    else
        echo -e "${RED}${BOLD}✗ ${fail_marker}${NC}"
        echo
        if [ "${FAILED_COUNT}" -gt 0 ]; then
            echo "Some tools are missing from PATH. Check package installation."
        fi
        if [ "${BROKEN_COUNT}" -gt 0 ]; then
            echo "Some tools are installed but broken. Check dependencies and environment."
        fi
        return 1
    fi
}

# Print scenario header
# Usage: scenario_header <name>
scenario_header() {
    local name=$1

    echo
    echo "═══════════════════════════════════════════════════════════"
    echo -e "  ${CYAN}${BOLD}${name}${NC}"
    echo "═══════════════════════════════════════════════════════════"
    echo
}

# Print section header
# Usage: section_header "Section Name"
section_header() {
    local name=$1
    echo
    echo -e "${CYAN}$name:${NC}"
}

# Print informational message
# Usage: info "message"
info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

# Print warning message
# Usage: warn "message"
warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

# Print error message
# Usage: error "message"
error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Print success message
# Usage: success "message"
success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}
