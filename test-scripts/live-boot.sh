#!/bin/sh
# Live boot: the live system is up and serves SSH, the same checks on every
# variant. The harness passes the services the contract says the live system
# runs (SCENARIO_SERVICES, init-system unit names) and has already logged in
# over SSH as root.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

scenario_header "Live Boot"

section_header "System"
test_file_exists /proc/cmdline "Kernel command line"
test_command "Root filesystem is writable" "touch /tmp/.scenario-test && rm /tmp/.scenario-test"
test_command "PID 1 is the init system" "[ \"\$(cat /proc/1/comm)\" = \"$(svc_pid1)\" ]"

section_header "Services"
for service in ${SCENARIO_SERVICES:?SCENARIO_SERVICES is not set}; do
    test_command "$service is enabled" "svc_enabled $service"
    test_command "$service is running" "svc_running $service"
done
test_command "sshd configuration validates" "sshd -t"
test_command "root has an SSH directory" "test -d /root/.ssh"

section_header "Network"
test_command "A default route exists" "ip route | grep -q '^default '"

report_results "Live Boot"
