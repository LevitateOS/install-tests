# install-tests

Scenario and installation verification harness for distro variants.

## Running

This crate is a library; run it through the workspace front door:

- `cargo xtask test <scenario|all> <distro>`: run scenarios (cached per input; `--force` reruns)
- `cargo xtask status <distro>`: which scenarios pass, fail or are stale (`--json` for tools)
- `cargo xtask reset <distro>`: forget saved results
- `cargo xtask run <scenario> <distro>`: boot a scenario interactively

## Distros

Supported ids: `levitate`, `acorn`.

## Boot injection

The scenario runner accepts boot injection through environment variables:

- `LEVITATE_BOOT_INJECTION_FILE=/abs/path/payload.env`
- `LEVITATE_BOOT_INJECTION_KV='KEY=VALUE,FOO=BAR'`

`cargo xtask test` and `cargo xtask run` set these from `--inject`, `--inject-file` and the SSH key.

## Notes

- Scenario preflight enforces contract + artifact checks before QEMU starts.
- `live-boot` includes SSH readiness/login verification after shell-ready.

Scenarios run independently and consume existing artifacts. Build products explicitly with `cargo xtask build <product> <distro>`. See the workspace `architecture.md` for ownership and output locations.

Scenario results are cached per input fingerprint. Use `cargo xtask test --force` to rerun one scenario, `cargo xtask status` to inspect current/stale results, and `cargo xtask reset` to remove saved state. `LEVITATE_SCENARIO_STATE_DIR` overrides the default workspace `.scenarios` directory. Corrupt state is reported with a reset command.
