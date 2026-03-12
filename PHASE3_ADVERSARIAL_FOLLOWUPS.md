# Phase 3 Adversarial Follow-Ups

Running follow-up list for gaps, approximations, and hardening work uncovered
while implementing the 2026-03-11 Phase 3 adversarial testing plan.

## CCS Schema

- Add first-class package conflict declarations to `ccs.toml`.
- Add explicit OR-dependency expressions such as `foo | bar` to `ccs.toml`.

## Fixtures And Coverage

- Native-package corruption coverage still uses truncated per-distro packages
  for Group G's T81, which exercises native-format rejection but not a true
  repo-style checksum mismatch path.
- Some malicious fixture families were added incrementally during rollout and
  should be reviewed for completeness against the Phase 3 design, especially
  proc-environ access, outside-root writes, signature-expiry cases, capability
  policy violations, decompression bombs, and intentionally failing scriptlets.
- Adversarial manifests still hard-code in-container fixture paths like
  `/opt/remi-tests/fixtures/...` because the Rust test engine does not yet
  expose a dedicated adversarial fixture root variable.

## Capability And Policy

- Install-time capability declarations currently fail closed. That is safer
  than silently accepting them, but Conary still needs a real capability
  allow/deny and application model for CCS installs.

## Mock Server

- The current mock server only supports static routes with optional headers,
  delay, and truncation.
- It still needs stateful retries or response sequences.
- It still needs mirror-pool or first-success failover modeling.
- It still needs TLS handshake, certificate-chain, and hostname-validation
  coverage.
- It still needs support for large generated bodies created during setup.

## Lifecycle And Bootstrap

- Group L lifecycle coverage still behaves more like robustness probing than
  strict end-to-end success verification for generation switching and rollback
  inside unprivileged containers.
- Self-update tests still use synthesized local payloads rather than a faithful
  signed artifact pipeline.
- Bootstrap artifact checks still verify that stage0 starts and emits plausible
  output, not that the full artifact set is complete across all CI distros.

## Kernel, Boot, And QEMU

- Group N container tests validate deployed files and generation/BLS layout,
  not actual boot correctness.
- Real boot correctness still depends on the QEMU half.
- The first `qemu_boot` implementation is intentionally thin: it shells out to
  host tools, assumes SSH in the guest, and does not yet provide richer VM
  orchestration such as snapshots, serial-pattern matching, or guest-specific
  auth flows.
- Boot-image generation still needs a smoother end-to-end builder flow for
  producing the qcow2 artifacts used by Group N.

## Publishing And Hosting

- Adversarial fixtures initially built locally but could not be published to a
  dedicated `/test-fixtures/adversarial/` path until server-side work was
  added. That publish path should still be rechecked end-to-end after deploys.
- Task 24 originally only completed locally; the hosted fixture and artifact
  paths should remain part of post-deploy smoke coverage.

## Podman And Local Smoke

- Local Phase 3 smoke originally exposed multiple Podman compatibility issues
  in `conary-test`. Those have been improved, but local Podman-backed smoke
  coverage is still worth keeping as a first-class supported path.

## Group M And Converted Packages

- Real converted-package installs required explicit base-system adoption during
  fresh DB runs before provider data like `glibc` and `libc.so.6` could satisfy
  dependency checks reliably. That area is much better now, but it should stay
  on the watch list for regressions.

## Metadata Refresh

- Remi now refreshes stale repositories on an interval and retries one targeted
  refresh when conversion hits an upstream `404`.
- It would still be valuable to add a persisted metadata fingerprint or
  checksum system so Remi can cheaply detect `repomd.xml` or equivalent
  upstream metadata changes without relying only on age-based refresh.
