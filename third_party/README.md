# Third-Party Patches

This directory contains temporary crate patches used through the workspace
`[patch.crates-io]` table.

- `rust-s3-0.37.2-patched`
- `aws-creds-0.39.1-patched`

Both are copied from their crates.io releases and keep the upstream MIT license
metadata in `Cargo.toml`. The local change is limited to raising their
`quick-xml` dependency from `0.38` to `0.41` so the release audit gate does not
ship `RUSTSEC-2026-0194` or `RUSTSEC-2026-0195`.

Remove these patches once upstream publishes compatible crates that depend on
`quick-xml >= 0.41`.
