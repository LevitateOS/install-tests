# install-tests

Scenario and installation verification harness for distro variants.

## Canonical entrypoints

- Scenario loop (recommended):
  - `cargo xtask scenarios boot live-boot levitate`
  - `cargo xtask scenarios test live-tools levitate`
  - `cargo run --bin scenarios -- --distro levitate --scenario build-preflight`
  - `cargo run --bin scenarios -- --distro levitate --status`
- Step catalog:
  - `cargo run --bin install-tests -- list --distro levitate`

`install-tests -- run` is intentionally disabled: the legacy serial wrapper harness has been removed.

## Distros

Supported ids: `levitate`, `acorn`.

## Boot injection

The scenario runner accepts boot injection through environment variables:

- `LEVITATE_BOOT_INJECTION_FILE=/abs/path/payload.env`
- `LEVITATE_BOOT_INJECTION_KV='KEY=VALUE,FOO=BAR'`

For interactive and `just` workflows, this is usually passed by `cargo xtask scenarios ...` or the root `just scenario*` wrappers.

## Notes

- Scenario preflight enforces contract + artifact checks before QEMU starts.
- `live-boot` includes SSH readiness/login verification after shell-ready.
- Use `just scenario*` and `just release-build*` wrappers in repo root for the default operator flow.

Scenarios run independently and consume existing artifacts. Build products explicitly with `just release-build <product> <distro>`. See the workspace `architecture.md` for ownership and output locations.

Scenario results are cached per input fingerprint. Use `--force` to rerun one scenario, `--status` to inspect current/stale results, and `--reset` to remove saved state. `LEVITATE_SCENARIO_STATE_DIR` overrides the default workspace `.scenarios` directory. Corrupt state is reported with a reset command.
