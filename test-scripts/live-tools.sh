#!/bin/sh
# Live tools: the live environment around the tools. The harness runs every
# contract tool itself (the live-tools scenario) and then this script.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

scenario_header "Live Tools"

section_header "Live Environment"
test_command "PATH includes /usr/local/bin" "echo \"\$PATH\" | tr ':' '\\n' | grep -qx '/usr/local/bin'"
test_command "block subsystem visible" "test -d /sys/class/block && ls /sys/class/block >/dev/null"
test_command "kernel modules installed for the running kernel" "test -f /lib/modules/\$(uname -r)/modules.dep"
test_command "a modular driver loads (iwlwifi, with cfg80211)" "modprobe iwlwifi && grep -q '^iwlwifi ' /proc/modules"
test_command "network interfaces discoverable" "ip -brief link >/dev/null"
test_command "PCI inventory probe" "lspci -mm >/dev/null"

section_header "Live Overlay"
test_file_exists /live-boot-marker "Live overlay marker"
test_command "Root is overlayfs over /live-overlay and /rootfs" "mount | grep ' type overlay ' | grep -q 'lowerdir=/live-overlay:/rootfs'"

report_results "Live Tools"
