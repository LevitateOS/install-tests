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
Installation and build preflight select live-desktop for desktop, and the
existing base products for base. Installed boot/login/runtime checks select an
install of that exact flavour; the VM gets the corresponding memory size and
the guest's actual flavour is checked. Live-only scenarios retain their own
named product. A wrong explicit run fails rather than substituting the newest
product or another flavour. Old install records without a flavour establish base only.

Selected ISO bytes, run records, public descriptor and install disk/firmware
identity participate in result reuse. Status rechecks the same recorded paths,
including explicit historical artifacts. Preflight checks the selected release's
own canonical payload hashes and kernel/module binding; it does not substitute
the checkout's current kernel for a selected historical baseline. No input
selector builds artifacts or invokes another scenario as a prerequisite.

## Public-origin A/B scenario (x86_64)

This mode is implemented but its VM lifecycle has **not been run**. Production
provisioning and enablement remain pending. It uses the existing named
`ab-update` lifecycle with real candidate discovery and acquisition by the
installed Recipe/Ginux producer. It cannot mount a RECAB_P13 feed ISO, launch a
loopback helper, accept a private key or substitute a local payload tree.

Use a complete existing stable baseline and a genuinely newer immutable
publication of the same distro/init/libc/flavour. Generate a small public JSON
input from their existing signed metadata and inventories:

```sh
target/debug/ab-update-fixture public --repo-root /absolute/repo --distro levitate \
  --baseline-manifest /absolute/baseline/release.json \
  --base-policy /absolute/baseline/rootfs-source/usr/lib/recab/update-source.json \
  --publication /absolute/new-publication --inventory /absolute/new-release/system-inventory.json \
  --updated-path usr/share/example-release-data --conflict-path etc/example.conf \
  --output /absolute/public-update.json
```

The generator verifies signatures/delegation, manifest and inventory hashes,
exact publication bytes, x86_64 execution profile and sequence advancement. Both
observation paths must actually change upstream and be ordinary files in the
old and new inventories. It writes metadata only; no image build, payload copy,
signing, network request or VM boot occurs. The input binds baseline/release IDs,
expected identity, candidate policy, exact publication SHA-256 and source commit.
The canonical Pages/Release origins must match the approved production design.

When an installation run is explicitly authorized, it consumes its selected
existing producer run and personalizes only candidate audience/index settings
before recsetup seals both identical roots. It retains the baseline trust anchor,
factory proof and delivery grants; old development media cannot be promoted by
this personalization. Public and desktop A/B test disks are sparse 96 GiB, leaving shared
space for desktop transport, verifier copies and retry scratch. This does not
change installed production disk defaults. For example:

```sh
cargo xtask test install levitate --flavour desktop --mode ab \
  --release-run /absolute/existing-live-desktop-run --public-update /absolute/public-update.json
```

The independent lifecycle run consumes an already successful compatible paired
installation through an overlay; it does not call `install` or build anything:

```sh
cargo xtask test ab-update levitate --flavour desktop --mode ab \
  --install-run /absolute/existing-public-install-run --public-update /absolute/public-update.json
```

Use the same selectors for AcornOS and for base. A public install requires an
explicit release run; public `ab-update` requires an explicit install run. Its
baseline/policy binding permits reuse for a later compatible target descriptor.

The guest checks actual signed discovery against the exact expected release and
reconstructs its publication binding from the authenticated pin. It interrupts
an actual acquisition process group after a durable part checkpoint and before
all parts complete, then requires verified saved-part/object reuse. An origin
failure or a transfer too fast to observe interruption fails clearly; it never
substitutes the local truncation helper or invents a cancellation receipt.
Download waiting has a separate bounded public-transfer budget. All subsequent
X/conflict/reproduction, failed/healthy trial, reverse Y, profile/data preservation
and write-reduction observations use the existing lifecycle engine. Resolving
source-policy conflicts retains the incoming authority/delivery policy and its
baseline sequence, then applies only the explicit candidate audience/index
personalization. The incoming policy is read from the verified release through
a read-only EROFS mount; the running and prepared systems are not remounted
writable. Every reboot checks the selected base/desktop flavour.

Only a completely successful actual public-origin run writes
`public_update_result` in `run-manifest.json`, bound to the descriptor, release,
source commit, publication, flavour, public index and both lifecycle directions.
Partial observations and every boot transcript remain in the run directory.
`cargo xtask release record-validation /absolute/passing-run` exports the
matching small validation/run records for review without signing, pushing or
enabling production. Stable promotion checks these structured observations; a
local-fixture run or an operator boolean alone cannot satisfy that guard.

## A/B continuation and signed update fixture

`ab-update` exercises the existing installed disk through overlays. It checks
first-use A and B, X plus a signed downloaded release, an unresolved conflict,
failed and healthy trials in each direction, Y, repeated incremental
continuation, discard and rollback. It never installs prerequisites or builds
release artifacts. Supply an existing `RECAB_P13` fixture ISO explicitly:

```sh
cargo xtask test ab-update levitate --update-fixture /absolute/path/update-fixture.iso
cargo xtask test ab-update acorn --update-fixture /absolute/path/acorn-update-fixture.iso
```

The existing successful A/B install must have the matching public source and
trust policy sealed into both slots. `install --update-fixture` personalizes
that policy before `recsetup` seals the paired systems. It also enables SSH
through `recsetup` and seeds hardlinks, numeric ownership, sparse and raw-name
local files. To produce such an install when an install run is requested:

```sh
cargo xtask test install levitate --update-fixture /absolute/path/update-fixture.iso
```

These are independent scenario invocations, not an ordered test ladder.
`ab-update` refuses an unconfigured old install; it does not run `install` for
you. Existing configured installs can be reused with another newer fixture if
their public source/trust policy matches. Inputs, fixture SHA-256 and source
fingerprints participate in result reuse. Changes during a run invalidate it.

The fixture packer consumes a signed release manifest and its already produced
payloads, the original installed policy and a static guest HTTP helper. Build
the small helpers explicitly; this does not build a kernel or system image:

```sh
cargo build -p install-tests --bin ab-update-fixture
rustc --edition 2024 --target x86_64-unknown-linux-musl -C opt-level=2 \
  -C strip=symbols testing/install-tests/src/bin/ab-update-origin.rs -o /tmp/ab-update-origin
target/debug/ab-update-fixture pack \
  --manifest /absolute/path/new-release/release.json \
  --base-policy /absolute/path/original/update-source.json \
  --public-key "$RECAB_TEST_PUBLIC_KEY" \
  --index-signing-key /absolute/path/private-test-key.pk8 \
  --origin /tmp/ab-update-origin --output /absolute/path/update-fixture.iso \
  --updated-path usr/share/example-release-data --conflict-path etc/example.conf
```

`xorriso` must be available. Use a disposable test signing key matching the
signed release. The packer verifies that release, checks its higher sequence and
matching channel, and signs an index using the existing release contract. Both
selected paths must be ordinary release-owned files: `updated-path` must really
change from the installed base, and `conflict-path` must change upstream so the
local X edit produces a conflict. Do not select a probe-owned path. The packer
does not build or rewrite release content. It refuses an existing output and a
dynamic or wrong-architecture helper. The ISO contains only public policy,
signed artifacts, acquisition recipe and helper; private signing keys stay out.

The loopback-only helper deliberately truncates the first rootfs download.
The scenario requires the refusal, retries, and verifies that acquisition kept
the preparation and running root intact. Merge decisions keep the explicit
local configuration and reproduce the two explicitly authored probe selections
through Recipe against the new release. Unexpected configuration/package
conflicts fail with a useful error; they are not resolved automatically.

`ab-update.json` records actual boots, generation identities, recab reports,
producer/invocation I/O and isolated root-partition sector deltas. Each boot has
its own `boot-NN.serial.log` in the run directory, including failed unattended
trials; a later boot never overwrites it. Independent Recipe profile contents
and shared data are checked after trials and rollback. Metadata-only Recipe
revisions change owner/mode, ACLs, capabilities and user xattrs, then remove the
attributes; unchanged file content is verified across the release and boots.
Prepared packages must remain absent from the active Recipe selection, and
check/download must preserve boot selection as well as the preparation.
The 32 MiB probe changes one byte for Y; bounded root writes and zero unchanged payload
writes are separate from Recipe staging/export traffic. Invocation I/O includes
producer I/O: do not add them. These are application and virtual block-device
observations, not physical SSD SMART or lifetime measurements.

The runtime scenario also executes an output-only recipe through the installed
Recipe/Ginux worker and runtime image, and verifies its native architecture.
No system package is activated for that check.

The new VM lifecycle is implemented but has not been run against rebuilt
artifacts. Native Linux x86_64 is the supported target. Additional CPU
architectures are deferred indefinitely and are not required for P13.

## Failing fast

A scenario stops at the first fatal console line or failed step and tears its
domain down. Timeouts are upper bounds for a step that is still making
progress: when a step times out, find out what is slow or broken instead of
raising the timeout.
