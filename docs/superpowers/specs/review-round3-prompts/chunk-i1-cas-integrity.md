You are performing an invariant verification review of a Rust codebase (Conary, a next-generation Linux package manager). This is round 3 -- rounds 1 and 2 found per-module and cross-boundary bugs. Round 3 exhaustively verifies that critical system invariants hold everywhere.

This is chunk-i1-cas-integrity.

## Contract
Every CAS object is stored by SHA-256 hash, integrity-verified on every retrieval, and never silently corrupted or lost while referenced by a surviving state.

## What To Verify

1. Every call to `store()` computes the SHA-256 hash of the content being written and verifies it matches the expected hash before finalizing the write. No store path accepts caller-provided hashes on trust without re-hashing.
2. Every call to `retrieve()` re-hashes the content read from disk and compares it to the expected hash. A mismatch returns an error, never silently returns corrupted data.
3. Every use of `retrieve_unchecked()` (or any variant that skips verification) is on a trusted path only -- never on data sourced from the network, user input, or any path where corruption could have occurred.
4. `object_path()` performs complete hex validation on the hash string. No partial validation (e.g., checking length but not charset) that would allow path traversal or invalid filenames.
5. All CAS writes use atomic write patterns (write to temp file, then rename). No direct writes to the final path that could leave partial objects on crash.
6. `state_cas_hashes` (or equivalent state snapshot mechanism) preserves the complete set of CAS hashes referenced by surviving states. No hash is dropped during snapshot creation that would cause GC to collect a live object.
7. No code path writes directly to the CAS directory bypassing the `store()` API. Grep for raw filesystem writes to the CAS root path.
8. CAS GC never removes an object that is still referenced by any surviving generation, transaction, or pending operation.
9. Hash algorithm is consistently SHA-256 everywhere -- no mixed use of other algorithms (e.g., XXH128 used for CAS addressing would break the integrity model).
10. Error handling on CAS I/O failures (disk full, permission denied, read error) propagates correctly and never leaves the CAS in an inconsistent state (e.g., a hash recorded in the DB but no corresponding object on disk).

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
- `conary-core/src/filesystem/` (CAS implementation)
- `conary-core/src/generation/` (generation building, GC)
- `conary-core/src/transaction/` (transaction commit)
- `conary-core/src/ccs/` (CCS package handling)
- `conary-core/src/delta/` (delta application)
- Any file that imports or calls CAS functions

## Summary
- Critical: N
- High: N
- Medium: N
- Low: N
