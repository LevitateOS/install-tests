#!/bin/sh
# Desktop boot: the release ISO boots into the installer, the same checks on
# every variant. No service failed, the live session has its network
# (NetworkManager from the `network` group on the overlay), the install
# screen holds tty1, and the medium carries what the installer's software
# step installs: the default set's list and recipes and the tools that fetch
# its package parts (recupdate, the release record, the production release
# key; the medium itself holds no package cache), plus the recab app for A/B
# installs. Every live medium is
# the base system: the desktop is installed into the target, never run here.
# The harness passes SCENARIO_INSTALL_SPLIT (the install screen's split pane,
# relative to /), SCENARIO_NETWORK_SERVICE (NetworkManager's service name),
# SCENARIO_DEFAULT_SET_DIR and SCENARIO_DEFAULT_SET (the medium's default set
# directory and the recipe names it lists, separated by spaces),
# SCENARIO_RECIPE_CACHE (Recipe's offline package cache, which the medium does
# not carry), SCENARIO_RELEASE_RECORD (the medium's release.json) and
# SCENARIO_RELEASE_KEYS (the folder of its production public keys),
# SCENARIO_AB_ONLY_TREE
# and SCENARIO_AB_DESKTOP_FILES (the recab app's files, below that tree,
# separated by spaces), and has already logged in over SSH as root.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/lib/common.sh"

: "${SCENARIO_INSTALL_SPLIT:?SCENARIO_INSTALL_SPLIT is not set}"
: "${SCENARIO_NETWORK_SERVICE:?SCENARIO_NETWORK_SERVICE is not set}"
: "${SCENARIO_DEFAULT_SET_DIR:?SCENARIO_DEFAULT_SET_DIR is not set}"
: "${SCENARIO_DEFAULT_SET:?SCENARIO_DEFAULT_SET is not set}"
: "${SCENARIO_RECIPE_CACHE:?SCENARIO_RECIPE_CACHE is not set}"
: "${SCENARIO_RELEASE_RECORD:?SCENARIO_RELEASE_RECORD is not set}"
: "${SCENARIO_RELEASE_KEYS:?SCENARIO_RELEASE_KEYS is not set}"

# The install screen's split pane running on tty1.
install_screen_on_tty1() {
    ps -t tty1 -o args= 2>/dev/null | grep -q "/$SCENARIO_INSTALL_SPLIT"
}

scenario_header "Desktop Boot"

# SSH answers before start-up has finished: wait up to 90 s for it and for
# the install screen.
waited=0
while [ "$waited" -lt 90 ]; do
    svc_boot_done && install_screen_on_tty1 && break
    sleep 2
    waited=$((waited + 2))
done

section_header "Boot"
test_command "Start-up finished" "svc_boot_done"
test_command "No service failed" "[ -z \"\$(svc_failed)\" ] || { svc_failed; false; }"
test_command "The live session's network ($SCENARIO_NETWORK_SERVICE) runs" "svc_running $SCENARIO_NETWORK_SERVICE"

section_header "Installer"
test_command "The install screen holds tty1" "install_screen_on_tty1"

# What the software step installs from: the list and each recipe it names. The
# packages come as parts fetched at install time (recupdate fetch-parts), so the
# medium needs the fetcher, the record of the release it was built from and a
# production key, and no package cache of its own.
section_header "Default software set"
test_file_exists "$SCENARIO_DEFAULT_SET_DIR/list" "The default set's list"
for recipe in $SCENARIO_DEFAULT_SET; do
    test_command "$recipe is on the list" "grep -qx '$recipe' '$SCENARIO_DEFAULT_SET_DIR/list'"
    test_file_exists "$SCENARIO_DEFAULT_SET_DIR/recipes/$recipe.rhai" "The $recipe recipe"
done
test_command "The medium carries no package cache" "[ -z \"\$(ls -A '$SCENARIO_RECIPE_CACHE' 2>/dev/null)\" ]"
test_command "recupdate, which fetches the package parts, is installed" "command -v recupdate"
test_file_exists "$SCENARIO_RELEASE_RECORD" "The record of the release the medium was built from"
test_command "A production release key is on the medium" "ls '$SCENARIO_RELEASE_KEYS'/*.pub"

# The recab app is only for an A/B system: the live medium carries it in the
# tree the A/B boot step copies into the target, not on the live system's PATH.
section_header "The recab app, for A/B installs only"
: "${SCENARIO_AB_ONLY_TREE:?SCENARIO_AB_ONLY_TREE is not set}"
for file in ${SCENARIO_AB_DESKTOP_FILES:?SCENARIO_AB_DESKTOP_FILES is not set}; do
    test_command "/$file is in the A/B-only tree" "[ -f '$SCENARIO_AB_ONLY_TREE/$file' ]"
    test_command "/$file is not on the live system" "[ ! -e '/$file' ]"
done

# The Updater app is only for a mutable system: the mirror image of the above.
section_header "The Updater app, for mutable installs only"
: "${SCENARIO_MUTABLE_ONLY_TREE:?SCENARIO_MUTABLE_ONLY_TREE is not set}"
for file in ${SCENARIO_MUTABLE_DESKTOP_FILES:?SCENARIO_MUTABLE_DESKTOP_FILES is not set}; do
    test_command "/$file is in the mutable-only tree" "[ -f '$SCENARIO_MUTABLE_ONLY_TREE/$file' ]"
    test_command "/$file is not on the live system" "[ ! -e '/$file' ]"
done

report_results "Desktop Boot"
