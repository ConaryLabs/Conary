# bootstrap/stage0 Reference

This directory is a checked-in reference for an older `crosstool-ng`-driven
cross-toolchain experiment. It is **not** the supported bootstrap entrypoint
for the current release.

The active bootstrap surface lives under `conary bootstrap` and is documented
in `docs/modules/bootstrap.md`. The current user-facing commands are:

```bash
target/debug/conary bootstrap --help
target/debug/conary bootstrap cross-tools
target/debug/conary bootstrap temp-tools
target/debug/conary bootstrap system
target/debug/conary bootstrap config
target/debug/conary bootstrap image --format qcow2
target/debug/conary bootstrap tier2
target/debug/conary bootstrap run <manifest> --seed <seed-dir>
target/debug/conary bootstrap verify-convergence --run-a <dir> --run-b <dir>
target/debug/conary bootstrap diff-seeds <seed-a> <seed-b>
```

The modern CLI intentionally rejects legacy stage names such as
`conary bootstrap stage0`.

## What Is Still Useful Here

- `crosstool.config` preserves a historical static toolchain baseline for
  `x86_64-conary-linux-gnu`
- the config still records the expected vendor and toolchain shape for that
  experiment:
  - `CT_STATIC_TOOLCHAIN=y`
  - `CT_TARGET_VENDOR="conary"`
  - Linux headers `6.18`
  - binutils `2.46`
  - glibc `2.43`
  - GCC `15`

Use this directory only if you are intentionally revisiting that older
bootstrap approach. Do not treat `ct-ng build` here as the release-backed
bootstrap workflow.
