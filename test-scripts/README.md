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
├── live-boot.sh           # live system up, its services running, sshd, a default route
├── live-tools.sh          # the live environment around the tools, the live overlay
├── install.sh             # what "the install passed" means, on the mounted target
├── installed-boot.sh      # installed system healthy (no failed service), not live
├── automated-login.sh     # a root login runs commands
├── runtime.sh             # privilege policy and real Recipe/Ginux native worker execution
└── desktop-boot.sh        # live desktop up: display manager, graphical session, no failed service
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
- The tools themselves are run by the harness from the contract's lists, not
  by these scripts.

To run one by hand in a guest, copy the tree there and set the environment:
`SCENARIO_PASS_MARKER=OK SCENARIO_INIT=systemd SCENARIO_SERVICES=sshd sh live-boot.sh`.
