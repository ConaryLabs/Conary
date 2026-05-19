# Release Security Waivers - 2026-05-06

This file records temporary RustSec exceptions for the limited public preview
readiness gate. Waivers here are not blanket approval for a wider release.

## Active Waivers

### RUSTSEC-2023-0071 - rsa 0.9.10

- **Advisory:** Marvin Attack: potential key recovery through timing sidechannels
- **Severity:** Medium, CVSS 5.9
- **Current fix status:** No fixed `rsa` release is available.
- **Dependency paths:**
  - `rsa 0.9.10 -> openidconnect 4.0.1 -> sigstore 0.13.0`
  - `rsa 0.9.10 -> sequoia-openpgp 2.2.0 -> conary-core`
  - `rsa 0.9.10 -> sigstore 0.13.0`
- **Conary reachability:** Conary uses these stacks for trust metadata,
  OpenPGP verification, Sigstore verification, and OIDC/Sigstore support.
  Conary does not expose RSA private-key decryption or signing operations to
  package install, conversion, Remi serving, conaryd, or test harness flows.
- **Limited-preview rationale:** The advisory is a private-key timing side
  channel and the reachable Conary paths are verification-oriented. Removing
  Sigstore, OpenPGP, or OIDC support would weaken the release more than a
  documented temporary waiver.
- **Expiry condition:** Remove this waiver as soon as `rsa`, `sigstore`,
  `openidconnect`, or `sequoia-openpgp` publishes a compatible fixed path, or
  before any release scope broader than a limited preview.
- **Release sign-off:** Required before publishing the limited preview.
- **2026-05-19 update:** Removed the `sigstore-trust-root` feature and the
  `tough` dependency from `Cargo.lock`; this waiver remains because `rsa`
  is still pulled by `sigstore`/`openidconnect` and `sequoia-openpgp`.
- **2026-05-19 security-advisory pipeline revisit:** Goal 3 did not add new
  Rust dependencies or expand RSA private-key operations. The release gate
  remains `bash scripts/release-cargo-audit.sh`, with this waiver as the only
  ignored RustSec vulnerability.

## Non-Blocking Warnings

`cargo audit` also reports `RUSTSEC-2024-0370` for unmaintained
`proc-macro-error 1.0.4` through `json-syntax -> sigstore`. This is an
unmaintained warning, not a vulnerability gate failure, and is not ignored by
`scripts/release-cargo-audit.sh`.
