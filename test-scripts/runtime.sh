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

section_header "Recipe and Ginux runtime"
recipe_runtime() {
    local work
    work=$(mktemp -d /var/tmp/recipe-runtime.XXXXXX) || return 1
    cat > "$work/probe.rhai" <<'RHAI'
const RECIPE_API = 2;
const NEEDS = #{ outputs: #{ proof: "file" } };
let ctx = #{ name: "scenario-native-worker", version: "1", path: RUNTIME.paths.output + "/native-worker-ok" };
fn is_acquired(ctx) { ready(ctx) }
fn is_built(ctx) { ready(ctx) }
fn is_installed(ctx) {
    if !is_file(ctx.path) { return needs_work(ctx, "missing worker output"); }
    ctx.outputs = #{ proof: ctx.path }; ready(ctx)
}
fn install(ctx) {
    write_file(ctx.path, RUNTIME.target.arch + "\n");
    ctx.outputs = #{ proof: ctx.path }; ctx
}
RHAI
    if ! (cd "$work" && recipe --json --no-activate --build-dir "$work/build" --state-dir "$work/state" install "$work/probe.rhai") > "$work/report.json" 2> "$work/stderr"; then
        cat "$work/stderr"; return 1
    fi
    set -- "$work"/state/exports/*/proof
    [ "$#" -eq 1 ] && [ -f "$1" ] && [ "$(cat "$1")" = "$(uname -m)" ]
}
test_command "Native worker executes through the installed Recipe/Ginux/runtime image" "recipe_runtime"

report_results "Runtime"
