# Scenario scripts

The guest-side half of the scenarios. The harness (`src/scenarios/`) copies a
scenario's script, `lib/common.sh` and the init backend into the guest's
`/run/scenario-tests/`, runs the script with the facts it needs as
environment, and passes the scenario only when the script exits 0 and prints
`SCENARIO_PASS_MARKER`.

```
test-scripts/
├── lib/
│   ├── common.sh          # checks that record and carry on, one report
│   └── init/
│       ├── systemd.sh     # svc_enabled, svc_running, svc_failed, svc_boot_done,
│       └── openrc.sh      # svc_pid1, svc_autologin, svc_diagnose, svc_add_failing, svc_remove
├── test-live-ssh.sh           # live system up, its services running, sshd, a default route
├── test-install-base.sh          # the live environment around the tools, the live overlay
├── install.sh             # what "the install passed" means, on the mounted target (A/B: both roots, both entries, recab; mutable: no recab)
├── install-pty.sh         # host-side driver of the live install screen over an SSH-allocated PTY (not run in the guest)
├── test-installed-boot.sh      # installed system healthy (no failed service), not live
├── automated-login.sh     # a root login runs commands
├── runtime.sh             # privilege policy and real Recipe/Ginux native worker execution
├── desktop-boot.sh        # release medium: installer up, live network, default software set and its package cache
└── login.sh               # desktop install: reclogin on console 7 (greeter as reclogin), no SDDM, text login on console 1
```

## Rules

- One script per scenario, the same on every variant: a script never detects
  the distro. What differs comes in as environment (`SCENARIO_INIT`,
  `SCENARIO_SERVICES`, `SCENARIO_PRIVILEGE`, ...), and only `lib/init/*.sh`
  knows an init system.
- Portable sh: what bash and busybox ash both run (no `echo -e`, no
  `pipefail`), and no `set -e`. Every check goes through `test_command` or
  `test_file_exists`, which record the result and carry on, so a failing
  script reports every failed check.
- A check tests a property; it never changes the guest to make itself pass.
- `other-root` has no script: `src/scenarios/other_root.rs` drives it over the
  serial console and SSH.
- The tools themselves are run by the harness from the contract's lists, not
  by these scripts.

To run one by hand in a guest, copy the tree there and set the environment:
`SCENARIO_PASS_MARKER=OK SCENARIO_INIT=systemd SCENARIO_SERVICES=sshd sh test-live-ssh.sh`.
