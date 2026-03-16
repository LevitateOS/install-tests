#!/bin/sh
# OpenSSH preflight validation
#
# Focused checks for first-class live-boot SSH behavior.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [ -f "$SCRIPT_DIR/lib/common.sh" ]; then
    . "$SCRIPT_DIR/lib/common.sh"
elif [ -f "/usr/local/lib/stage-tests/common.sh" ]; then
    . "/usr/local/lib/stage-tests/common.sh"
else
    echo "ERROR: Cannot find common.sh library" >&2
    exit 1
fi

scenario_header "OpenSSH Preflight"

section_header "OpenSSH Runtime Checks"
test_command "OpenSSH daemon binary exists" "command -v sshd >/dev/null 2>&1"
test_command "OpenSSH config validates" "sshd -t"

test_command "Kernel cmdline disables anaconda sshd conflict" "grep -qw 'inst.sshd=0' /proc/cmdline"

test_command "Root authorized_keys is injectable" "test -d /root/.ssh || mkdir -p /root/.ssh"

if command -v systemctl >/dev/null 2>&1; then
    section_header "systemd Service Wiring"
    test_command "sshd.service unit exists" "test -f /usr/lib/systemd/system/sshd.service"
    test_command "sshd keygen unit exists" "test -f /usr/lib/systemd/system/sshd-keygen@.service"
    test_command "sshd is enabled" "systemctl is-enabled sshd.service | grep -q '^enabled$'"
    test_command "sshd is active" "systemctl is-active sshd.service | grep -q '^active$'"
    test_command "tmpfiles policy creates /run/sshd" "test -f /usr/lib/tmpfiles.d/sshd.conf -o -f /etc/tmpfiles.d/sshd-local.conf"

    if systemctl list-unit-files 2>/dev/null | grep -q '^anaconda-sshd.service'; then
        test_command "anaconda-sshd is not active" "! systemctl is-active --quiet anaconda-sshd.service"
    fi
elif command -v rc-service >/dev/null 2>&1; then
    section_header "OpenRC Service Wiring"
    test_command "OpenRC network interfaces config exists" "test -f /etc/network/interfaces"
    test_command "OpenRC networking runlevel symlink exists" "test -L /etc/runlevels/boot/networking"
    test_command "OpenRC dhcpcd runlevel symlink exists" "test -L /etc/runlevels/default/dhcpcd"
    test_command "OpenRC networking service is running" "rc-service networking status"
    test_command "OpenRC dhcpcd service is running" "rc-service dhcpcd status"
    test_command "OpenRC default route is present" "ip route | grep -q '^default '"
    test_command "OpenSSH init script exists" "test -f /etc/init.d/sshd"
    test_command "OpenSSH runlevel symlink exists" "test -L /etc/runlevels/default/sshd"
    test_command "OpenSSH service is running" "rc-service sshd status"
else
    error "No supported init service manager found for SSH checks"
    record_failed "OpenSSH service checks unavailable"
fi

report_results "OpenSSH Preflight"
exit $?
