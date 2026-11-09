#!/bin/sh
# Host-side driver for an SSH-allocated guest PTY and the actual live UI.
# Guest /run markers are checked separately by the harness; no guest script(1)
# or extra package is needed. Disk installation is a separate scenario.
set -eu
guest_work=$1
terminal_command=$2
shift 2
work=$(mktemp -d)
pid=
finish() {
    code=$?
    trap - EXIT HUP INT TERM
    if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || :; wait "$pid" 2>/dev/null || :; fi
    printf '\nINSTALL_PTY_TRANSCRIPT_BEGIN\n'
    cat "$work/terminal" 2>/dev/null || :
    printf '\nINSTALL_PTY_TRANSCRIPT_END\n'
    rm -rf "$work"
    exit "$code"
}
trap finish EXIT
trap 'exit 1' HUP INT TERM
mkfifo "$work/input"
# ssh -tt allocates the real guest PTY even with the FIFO as local stdin.
# The watchdog cannot count as pass; SSH owns the session/PTY teardown.
timeout 75 "$@" "$terminal_command" <"$work/input" >"$work/terminal" 2>&1 &
pid=$!
exec 3>"$work/input"
wait_text() {
    n=0
    while ! tail -c "+${2:-1}" "$work/terminal" 2>/dev/null | grep -Fqi "$1"; do
        kill -0 "$pid" 2>/dev/null || { cat "$work/terminal"; return 1; }
        n=$((n + 1)); [ "$n" -lt 200 ] || { printf 'missing UI text: %s\n' "$1"; return 1; }
        sleep 0.1
    done
}
wait_file() {
    file=$1
    shift
    n=0
    # A separate real SSH connection observes a file written by the shell.
    # Terminal screen painting need not preserve contiguous output markers.
    while ! "$@" "test -f '$file'" >/dev/null 2>&1; do
        kill -0 "$pid" 2>/dev/null || return 1
        n=$((n + 1)); [ "$n" -lt 100 ] || return 1
        sleep 0.1
    done
}
wait_text 'choose a step'
wait_text 'Installation'
# The hub's real shell handoff must execute a command, not just echo its bytes.
printf s >&3
wait_text 'steps wait while you work in this shell'
after=$(wc -c <"$work/terminal")
printf "printf 'hub-shell-ok\\n' > '%s/hub'; printf 'HUB_%%s_EXECUTED\\n' SHELL; exit\n" "$guest_work" >&3
wait_file "$guest_work/hub" "$@"
# Wait for the shell to end and hub to draw again before switching to docs.
wait_text 'choose a step' "$((after + 1))"
printf '\035' >&3
wait_text 'Docs pane'
after=$(wc -c <"$work/terminal")
printf q >&3
wait_text 'Enter restarts' "$((after + 1))"
# Quit the terminal host, then execute in the launcher's fallback login shell.
after=$(wc -c <"$work/terminal")
printf '\021' >&3
wait_text 'root@' "$((after + 1))"
printf "printf 'fallback-shell-ok\\n' > '%s/fallback'; printf 'FALLBACK_%%s_EXECUTED\\n' SHELL; exit\n" "$guest_work" >&3
wait_file "$guest_work/fallback" "$@"
exec 3>&-
wait "$pid"
pid=
printf 'INSTALL_PTY_PASS hub-shell=1 docs-input=1 host-quit=1 fallback-shell=1\n'
