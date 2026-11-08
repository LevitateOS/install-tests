# The scenario scripts' service backend for OpenRC: the only place besides
# lib/init/systemd.sh that knows an init system.

svc_level() {
    for level in /etc/runlevels/*; do
        [ -e "$level/$1" ] && return 0
    done
    return 1
}
svc_enabled() { svc_level "$1"; }
svc_running() { rc-service "$1" status >/dev/null 2>&1; }
# The boot and default services that crashed or are not started, one per line.
svc_failed() {
    rc-status --crashed 2>/dev/null
    rc-status --format ini boot default 2>/dev/null | awk -F' = ' '$2 ~ /stopped|crashed|failed/ { print $1 }'
}
svc_boot_done() { [ "$(rc-status --runlevel 2>/dev/null)" = default ]; }
svc_pid1() { echo init; }
svc_autologin() { grep -qsE -- 'autologin|-n -l' /etc/inittab; }
svc_diagnose() {
    rc-service "$1" status
    rc-status --all
}
svc_add_failing() {
    mkdir -p "$1/etc/runlevels/default" &&
    printf '#!/sbin/openrc-run\nstart() {\n\treturn 1\n}\n' > "$1/etc/init.d/$2" &&
    chmod 755 "$1/etc/init.d/$2" &&
    ln -s "/etc/init.d/$2" "$1/etc/runlevels/default/$2"
}
svc_remove() {
    rm "$1/etc/init.d/$2" "$1/etc/runlevels/default/$2"
}
