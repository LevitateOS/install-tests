# install-tests

The scenario harness for LevitateOS and AcornOS. It boots a variant's built ISO
in a libvirt domain (through `recqemu`), drives the live and installed systems
over the serial console and SSH, and records each scenario's result. It is a
library: `cargo xtask test`, `status`, `reset` and `run` call it with typed
options (`scenarios::RunOptions`, `interactive::Mode`) and render what it
returns; `cargo xtask --help` is the command reference.

## Where things live

- The scenarios, their order and what each one's input is:
  `distro_contract::registry`. What a variant adds to them is its
  `distro/variants/<id>/distro.toml`.
- Running a scenario, its status and reset: `src/scenarios/`. Each run is a
  `distro_contract::runs` record in its own run directory; the newest finished
  one is the scenario's status.
- The one VM launcher, for scenarios and `cargo xtask run`: `src/qemu/session.rs`;
  `cargo xtask run` itself: `src/interactive.rs`.
- Checks of the ISO before a VM starts: `src/preflight.rs`.
- Settings handed to a live boot: `src/boot_injection.rs` (with the guest side in
  `recinit::live`).
- The serial console: `src/qemu/serial/`.
- The scripts a scenario runs in the guest: `test-scripts/`.

Scenarios consume existing artifacts; products are built explicitly with
`cargo xtask build`.

## Failing fast

A scenario stops at the first fatal console line or failed step and tears its
domain down. Timeouts are upper bounds for a step that is still making
progress: when a step times out, find out what is slow or broken instead of
raising the timeout.
