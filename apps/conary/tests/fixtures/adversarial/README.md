# Adversarial Test Fixtures

This directory contains Phase 3 integration-test fixtures for corrupted packages,
malicious payloads, adversarial dependency graphs, and large-package stress cases.

Layout:

- `corrupted/`: fixtures for checksum mismatch, truncation, and metadata corruption
  Also includes per-distro native-package corruption outputs under
  `corrupted/native/output/`.
- `malicious/`: fixtures for traversal, symlink, setuid, and hostile scriptlets
- `deps/`: interdependent CCS packages for resolver edge cases
- `large/`: tracked large CCS stress fixtures plus the regeneration scripts used
  to rebuild them
- `build-boot-image.sh`: optional QEMU boot fixture builder used by the
  adversarial boot-validation path

Use `apps/conary/tests/fixtures/adversarial/build-all.sh` to build every
fixture set, or run
`apps/conary/tests/fixtures/adversarial/build-large.sh` directly to regenerate
the large stress fixtures in `large/`.
