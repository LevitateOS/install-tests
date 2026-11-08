# CLAUDE.md - install-tests

The scenario harness for both distros; `README.md` says where each part lives.

## What belongs here

- Scenarios that boot a built ISO in a libvirt domain and check the live or
  installed system, on LevitateOS and AcornOS alike.
- The install scenario installs the way a user does, with the tools the ISO
  ships. If it passes, users can install; if it fails, fix the install, not the
  scenario.

## What does not

| Don't put here | Put it in |
|----------------|-----------|
| Built-artifact validation | `testing/fsdbg/` |
| Per-distro facts (identity, services, tools) | the variant contract, `distro/variants/<id>/distro.toml` |
| Domain definition and sizing | `tools/recqemu` |

Run it through `cargo xtask` (`cargo xtask --help`); `cargo check -p install-tests`
compiles it.
