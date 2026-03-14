# install-tests

Scenario and installation verification harness for distro variants.

## Canonical entrypoints

- Scenario loop (recommended):
  - `cargo run --bin scenarios -- --distro levitate --scenario build-preflight`
  - `cargo run --bin scenarios -- --distro levitate --up-to-scenario runtime`
  - `cargo run --bin scenarios -- --distro levitate --status`
- Step catalog:
  - `cargo run --bin install-tests -- list --distro levitate`

`install-tests -- run` is intentionally disabled: the legacy serial wrapper harness has been removed.

## Distros

Supported ids: `levitate`, `acorn`, `iuppiter`, `ralph`.

## Boot injection

The scenario runner accepts boot injection through environment variables:

- `LEVITATE_BOOT_INJECTION_FILE=/abs/path/payload.env`
- `LEVITATE_BOOT_INJECTION_KV='KEY=VALUE,FOO=BAR'`

For interactive and `just` workflows, this is usually passed by `xtask stages ...`.

## Notes

- Scenario preflight enforces contract + artifact checks before QEMU starts.
- `live-boot` includes SSH readiness/login verification after shell-ready.
- Use `just` wrappers in repo root for the default operator flow.
