---
last_updated: 2026-07-15
status: active
summary: Limited-preview RustSec waiver policy and resolved W1 crossbeam blocker
---

# Release Security Waivers

This file records temporary RustSec exceptions for the limited public preview
readiness gate. Waivers here are not blanket approval for a wider release.

## Current Unwaived Blockers

None. The fresh 2026-07-15 W1 release audit passed after the crossbeam
remediation below. The active RSA waiver and non-blocking warnings remain.

## Active Waivers

### RUSTSEC-2023-0071 - rsa 0.9.10

- **Advisory:** Marvin Attack: potential key recovery through timing sidechannels
- **Severity:** Medium, CVSS 5.9
- **Current fix status:** No fixed `rsa` release is available.
- **Dependency paths:**
  - `rsa 0.9.10 -> openidconnect 4.0.1 -> sigstore 0.14.0`
  - `rsa 0.9.10 -> sequoia-openpgp 2.3.0 -> conary-core`
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
- **2026-07-03 audit refresh:** At that checkpoint the RSA paths above still
  had no compatible fixed path and this remained the only ignored RustSec
  vulnerability. The later W1 audit resolved the separate unwaived crossbeam
  blocker; this RSA waiver remains active.

## Resolved Advisory Follow-Ups

### RUSTSEC-2026-0204 - crossbeam-epoch

The W1 candidate updates `crossbeam-epoch` from vulnerable version 0.9.18 to
fixed version 0.9.20 in `Cargo.lock`. The fresh 2026-07-15 release audit no
longer reports the invalid-pointer-dereference advisory. The dependency remains
transitive through `crossbeam-deque` and Rayon; no waiver or audit ignore was
added.

### RUSTSEC-2026-0194 and RUSTSEC-2026-0195 - quick-xml

Fresh RustSec data on 2026-07-03 reported two high-severity `quick-xml`
advisories against `quick-xml 0.38.4` and `0.40.1`.

- `conary-core` now depends on `quick-xml 0.41`.
- `rust-s3 0.37.2` and `aws-creds 0.39.1` are patched locally under
  `third_party/` to raise their `quick-xml` dependency from `0.38` to `0.41`.
- Remove the local patches once upstream publishes compatible crates that
  depend on `quick-xml >= 0.41`.

## Non-Blocking Warnings

`cargo audit` also reports `RUSTSEC-2026-0173` for unmaintained
`proc-macro-error2 2.0.1` and reports yanked `spin 0.9.8`. These are warnings,
not vulnerability gate failures, and are not ignored by
`scripts/release-cargo-audit.sh`.
