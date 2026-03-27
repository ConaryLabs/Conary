You are performing an invariant verification review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 exhaustively verifies that critical system invariants hold everywhere.

This is chunk-i3-signature-trust.

## Contract
Every cryptographic signature verification uses `verify_strict()`, checks against trusted keys, and cannot be bypassed by configuration, empty key lists, or error handling.

## What To Verify

1. No use of `verify()` (non-strict) in production code paths. Every signature check uses `verify_strict()` (or the equivalent strict verification mode for the cryptographic library in use). Search for all calls to signature verification functions.
2. When `TRUSTED_UPDATE_KEYS` (or any trusted key list) is empty, the system returns a hard error -- never silently skips verification or treats an empty key list as "trust everything."
3. CCS package verification checks all three layers: the CBOR signature over the package, the TOML metadata hash integrity, and the merkle root of the content tree. No verification path checks fewer than all required layers.
4. Model signing uses `canonical_json()` (or equivalent canonical serialization) consistently. No path where a model is signed or verified using non-canonical serialization that could allow signature mismatch or bypass.
5. Federation manifest signatures are verified BEFORE any data from the manifest is trusted or acted upon. No pattern where manifest contents are parsed and used, with verification deferred or optional.
6. TUF metadata enforcement is complete: key rotation is supported, threshold signatures are required (not just checked against 1), expiry timestamps are validated, and no TUF check is skipped in non-test code.
7. No `#[cfg(test)]` or `#[cfg(debug_assertions)]` blocks that weaken signature verification in ways that could accidentally leak into release builds. Test-only bypasses must be clearly gated and impossible to trigger in production.
8. Error handling around verification never converts a verification failure into a warning, log message, or default-allow. A failed signature check must be a hard stop.
9. Key material (public keys, trust anchors) is embedded or loaded from a trusted source. No path where an attacker-controlled input could substitute the verification key.
10. Signature algorithms are consistent and strong -- no fallback to weaker algorithms, no algorithm confusion attacks possible.
11. Self-update verification uses the same strict trust chain as package verification. The self-update binary cannot bypass signature checks that packages are subject to.
12. No TOCTOU (time-of-check-time-of-use) gaps where signature verification passes but the verified data is re-read or re-parsed before use, potentially reading different (unverified) content.

## Output Format

For each violation, report:

### [SEVERITY] Invariant violation title

**Location:** file:line
**Violation:** What breaks the contract and how
**Impact:** What goes wrong when this invariant is violated
**Suggested fix:** Concrete change to restore the invariant

Severity levels:
- CRITICAL: Invariant violated on a reachable code path with significant impact
- HIGH: Invariant violated but requires specific conditions to trigger
- MEDIUM: Invariant weakly held (defense-in-depth gap)
- LOW: Invariant technically holds but is fragile/undocumented

## Scope
Search the ENTIRE codebase for every location where this invariant is relevant. Do not limit to specific files. Pay particular attention to:
- `conary-core/src/trust/` (TUF supply chain trust)
- `conary-core/src/ccs/` (CCS signing and verification)
- `conary-core/src/model/` (model signing)
- `conary-core/src/self_update.rs` (self-update verification)
- `conary-core/src/provenance/` (provenance verification)
- `conary-core/src/federation/` (federation manifest trust)
- `conary-server/src/federation/` (server-side federation)
- Any file that imports ed25519, ring, rcgen, or cryptographic verification functions

## Summary
- Critical: N
- High: N
- Medium: N
- Low: N
