# The scenario scripts' service backend for systemd: the only place besides
# lib/init/openrc.sh that knows an init system.

svc_enabled() { systemctl is-enabled --quiet "$1.service"; }
svc_running() { systemctl is-active --quiet "$1.service"; }
# The failed units, one per line (none when the boot is healthy).
svc_failed() { systemctl list-units --failed --plain --no-legend --no-pager | awk '{print $1}'; }
# Whether start-up has finished (running or degraded).
svc_boot_done() {
    case "$(systemctl is-system-running 2>/dev/null)" in
        running|degraded) return 0 ;;
        *) return 1 ;;
    esac
}
# PID 1's command name.
svc_pid1() { echo systemd; }
# Whether a getty logs anyone in without a password.
svc_autologin() { grep -rqsE -- '--autologin|-a root' /etc/systemd/system/getty@.service.d /etc/systemd/system/serial-getty@.service.d; }
svc_diagnose() {
    systemctl status "$1.service" --no-pager -l
    journalctl -b -u "$1.service" --no-pager -n 80
}
# Add (svc_add_failing ROOT NAME) or remove (svc_remove ROOT NAME) a service
# under ROOT that always fails, enabled in the normal boot.
svc_add_failing() {
    mkdir -p "$1/etc/systemd/system/multi-user.target.wants" &&
    printf '[Service]\nType=oneshot\nExecStart=/bin/false\n[Install]\nWantedBy=multi-user.target\n' > "$1/etc/systemd/system/$2.service" &&
    ln -s "/etc/systemd/system/$2.service" "$1/etc/systemd/system/multi-user.target.wants/$2.service"
}
svc_remove() {
    rm "$1/etc/systemd/system/$2.service" "$1/etc/systemd/system/multi-user.target.wants/$2.service"
}
