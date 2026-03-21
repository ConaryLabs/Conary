# Bootstrap v2: Deferred Items

> Backlog items deferred from Phase 6 verification. Not a formal phase — pick up as needed.

## 1. Trust Level Policy Enforcement

Currently trust levels are informational (displayed by `verify-derivation chain` and SBOM). Add policy enforcement so the system can refuse packages below a configured threshold.

- [ ] Add `min_trust_level` setting (system model or config)
- [ ] Enforce during `Pipeline::execute()` — reject substituter hits below threshold
- [ ] Enforce during `conary install` from derivation cache
- [ ] `--trust-level` CLI override for one-off exceptions
- [ ] Error messages guide user toward `verify-derivation rebuild` to raise trust

## 2. Seed Revocation Checking

Allow seeds to be revoked (e.g., if a seed image is compromised). Requires a revocation list hosted on Remi.

- [ ] Design revocation list format (signed JSON on Remi, e.g. `/v1/seeds/revoked`)
- [ ] Remi endpoint to publish/query revoked seed IDs
- [ ] `verify-derivation chain` checks seed against revocation list before reporting COMPLETE
- [ ] `Pipeline::execute()` warns (or refuses) if seed is revoked
- [ ] Admin API endpoint for revoking seeds

## 3. SPDX SBOM Generation

CycloneDX is already implemented. SPDX is the other major SBOM standard, used by some compliance pipelines.

- [ ] Evaluate whether any users actually need SPDX (CycloneDX is more widely adopted)
- [ ] If needed: add `--format spdx` flag to `conary sbom`
- [ ] Reuse derivation index data, emit SPDX 2.3 JSON
- [ ] Map trust levels and provenance to SPDX annotations
