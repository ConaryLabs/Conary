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

Every CCS base package is current signed authority built with the disposable
key under `../ccs-test-authority/`. Corrupted fixtures are mutated only after
that signed build, so install failures exercise archive, projection, signature,
or payload integrity instead of the retired unsigned-package bypass. Rotate
the authority with `../ccs-test-authority/generate.sh`, then rebuild all CCS
fixture sets together.
