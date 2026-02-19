# install-tests

Stage and installation verification harness for distro variants.

## Canonical entrypoints

- Stage loop (recommended):
  - `cargo run --bin stages -- --distro levitate --stage 1`
  - `cargo run --bin stages -- --distro levitate --up-to 6`
  - `cargo run --bin stages -- --distro levitate --status`
- Step catalog:
  - `cargo run --bin install-tests -- list --distro levitate`

`install-tests -- run` is intentionally disabled: the legacy serial wrapper harness has been removed.

## Distros

Supported ids: `levitate`, `acorn`, `iuppiter`, `ralph`.

## Boot injection

The stage runner accepts boot injection through environment variables:

- `LEVITATE_BOOT_INJECTION_FILE=/abs/path/payload.env`
- `LEVITATE_BOOT_INJECTION_KV='KEY=VALUE,FOO=BAR'`

For interactive and `just` workflows, this is usually passed by `xtask stages ...`.

## Notes

- Stage preflight enforces contract + artifact checks before QEMU starts.
- Stage 01 includes SSH readiness/login verification after shell-ready.
- Use `just` wrappers in repo root for the default operator flow.
