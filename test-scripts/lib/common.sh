#!/bin/bash
# Common functions for checkpoint test scripts
#
# This library provides shared testing infrastructure used by all checkpoint
# scripts. It handles test execution, result tracking, and reporting.

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

# Test state
PASSED_TESTS=()
FAILED_TESTS=()
BROKEN_TESTS=()

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

    # Run the command and capture exit code and output
    output=$(eval "$cmd" 2>&1)
    exit_code=$?

    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}✓${NC}"
        PASSED_TESTS+=("$tool")
        return 0
    elif [ $exit_code -eq 127 ]; then
        echo -e "${RED}✗ NOT FOUND${NC}"
        FAILED_TESTS+=("$tool")
        return 1
    else
        # Extract first line of error for display
        local error_msg=$(echo "$output" | head -1 | cut -c1-60)
        echo -e "${YELLOW}✗ EXIT $exit_code${NC}"
        if [ -n "$error_msg" ]; then
            echo -e "    ${YELLOW}└─${NC} $error_msg"
        fi
        BROKEN_TESTS+=("$tool:$exit_code")
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
        PASSED_TESTS+=("$description")
        return 0
    else
        echo -e "${RED}✗ NOT FOUND${NC}"
        FAILED_TESTS+=("$description")
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

    output=$(eval "$cmd" 2>&1)
    exit_code=$?

    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}✓${NC}"
        PASSED_TESTS+=("$description")
        return 0
    else
        local error_msg=$(echo "$output" | head -1 | cut -c1-60)
        echo -e "${RED}✗ EXIT $exit_code${NC}"
        if [ -n "$error_msg" ]; then
            echo -e "    ${RED}└─${NC} $error_msg"
        fi
        FAILED_TESTS+=("$description")
        return 1
    fi
}

# Report final results
# Usage: report_results <checkpoint_number>
report_results() {
    local checkpoint=$1

    local total_tests=$((${#PASSED_TESTS[@]} + ${#FAILED_TESTS[@]} + ${#BROKEN_TESTS[@]}))

    echo
    echo "═══════════════════════════════════════════════════════════"
    echo -e "  ${BOLD}Checkpoint $checkpoint Results${NC}"
    echo "═══════════════════════════════════════════════════════════"
    echo -e "${GREEN}Passed:${NC} ${#PASSED_TESTS[@]}/$total_tests tests"

    if [ ${#FAILED_TESTS[@]} -gt 0 ]; then
        echo
        echo -e "${RED}Missing (not in PATH):${NC} ${#FAILED_TESTS[@]} tests"
        for tool in "${FAILED_TESTS[@]}"; do
            echo "  • $tool"
        done
    fi

    if [ ${#BROKEN_TESTS[@]} -gt 0 ]; then
        echo
        echo -e "${YELLOW}Broken (exist but failed):${NC} ${#BROKEN_TESTS[@]} tests"
        for item in "${BROKEN_TESTS[@]}"; do
            local tool="${item%%:*}"
            local code="${item##*:}"
            echo "  • $tool (exit $code)"
        done
    fi

    echo "═══════════════════════════════════════════════════════════"

    if [ ${#FAILED_TESTS[@]} -eq 0 ] && [ ${#BROKEN_TESTS[@]} -eq 0 ]; then
        echo -e "${GREEN}${BOLD}✓ CHECKPOINT $checkpoint PASSED${NC}"
        echo
        echo "All tools are present and functional in this environment."
        return 0
    else
        echo -e "${RED}${BOLD}✗ CHECKPOINT $checkpoint FAILED${NC}"
        echo
        if [ ${#FAILED_TESTS[@]} -gt 0 ]; then
            echo "Some tools are missing from PATH. Check package installation."
        fi
        if [ ${#BROKEN_TESTS[@]} -gt 0 ]; then
            echo "Some tools are installed but broken. Check dependencies and environment."
        fi
        return 1
    fi
}

# Print checkpoint header
# Usage: checkpoint_header <number> <name>
checkpoint_header() {
    local number=$1
    local name=$2

    echo
    echo "═══════════════════════════════════════════════════════════"
    echo -e "  ${CYAN}${BOLD}Checkpoint $number: $name${NC}"
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
