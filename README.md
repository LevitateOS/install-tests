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

## Explicit base/desktop artifacts

`cargo xtask test` and `cargo xtask run` accept `--flavour base|desktop`,
`--release-run /absolute/existing/run` and `--install-run /absolute/existing/run`.
Installation and build preflight select release for desktop, and the
existing base products for base. Installed boot/login/runtime checks select an
install of that exact flavour; the VM gets the corresponding memory size and
the guest's actual flavour is checked. Live-only scenarios retain their own
named product. A wrong explicit run fails rather than substituting the newest
product or another flavour. Old install records without a flavour establish base only.

Selected ISO bytes, run records and install disk/firmware
identity participate in result reuse. Status rechecks the same recorded paths,
including explicit historical artifacts. Preflight checks the selected release's
own canonical payload hashes and kernel/module binding; it does not substitute
the checkout's current kernel for a selected historical baseline. No input
selector builds artifacts or invokes another scenario as a prerequisite.

## The A/B scenarios

`install` checks, on the mounted target in the live system: an A/B install
verifies with `recab verify-installation`, has both boot entries and kernels,
root A as the boot default, recab, its `recab-started` service and the slot
marker in root A, and root B equal to root A (`recab compare --roots`). A
mutable install has no `usr/bin/recab`, no `recab-started` service and no
`usr/lib/recab`: only the A/B boot step of `recsetup` puts recab there.
`test-installed-boot` checks, on an A/B install, that the booted root is
read-only and the other root mounted read-write at `/run/recab/other`, and on a
mutable install that recab is absent.

`other-root` boots an existing A/B install through an overlay several times
(`src/scenarios/other_root.rs`): a file written into the unlocked other root
boots with `recab boot-other`, the booted root is locked and becomes the boot
default once `recab-started` ran; `recab copy` makes the roots equal (`recab
compare`); and an other root that cannot start (its init removed), picked once,
stops in the initramfs, after which the next boot starts the previous root.
Each boot keeps its own `boot-NN.serial.log` in the run directory. It needs an
A/B install run and does not run `install` for you:

```sh
cargo xtask test other-root levitate
cargo xtask test other-root acorn
```

The runtime scenario also executes an output-only recipe through the installed
Recipe/Ginux worker and runtime image, and verifies its native architecture.
No system package is activated for that check.

## Failing fast

A scenario stops at the first fatal console line or failed step and tears its
domain down. Timeouts are upper bounds for a step that is still making
progress: when a step times out, find out what is slow or broken instead of
raising the timeout.
