# Adversarial Test Fixtures

This directory contains Phase 3 integration-test fixtures for corrupted packages,
malicious payloads, adversarial dependency graphs, and large-package stress cases.

Layout:

- `corrupted/`: fixtures for checksum mismatch, truncation, and metadata corruption
- `malicious/`: fixtures for traversal, symlink, setuid, and hostile scriptlets
- `deps/`: interdependent CCS packages for resolver edge cases
- `large/`: generated stress fixtures that are too large to check in directly

Use `tests/fixtures/adversarial/build-all.sh` to build every fixture set, or run
`tests/fixtures/adversarial/build-large.sh` directly to regenerate the large stress
fixtures in `large/`.
