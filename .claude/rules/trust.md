---
paths:
  - "conary-core/src/trust/**"
---

# Trust Module

TUF (The Update Framework) implementation for supply chain security. Protects
against rollback, freeze, arbitrary package, and mix-and-match attacks using
Ed25519 signatures and versioned metadata with expiration.

## Key Types
- `RootMetadata` -- top-level TUF metadata with keys and role definitions
- `Signed<T>` -- wrapper adding signatures to any metadata type
- `TufKey` -- public key with `KeyVal` (hex-encoded Ed25519 public key)
- `TufSignature` -- signature with `keyid` and `sig` (hex-encoded)
- `RoleDefinition` -- keyids + threshold for a role
- `Role` -- enum: `Root`, `Targets`, `Snapshot`, `Timestamp`
- `TrustError` -- `VerificationFailed`, `MetadataExpired`, `RollbackAttack`, `ThresholdNotMet`
- `SigningKeyPair` -- from `ccs::signing`, used for key generation

## Constants
- `TUF_SPEC_VERSION` -- TUF specification version string

## Invariants
- `verify_signatures()` requires role-specific keys, NOT the full `root.keys` map
- Use `extract_role_keys()` to filter keys before calling `verify_signatures()`
- Signature deduplication: duplicate keyids in a signed document are skipped
- Only `ed25519` key type is supported -- other types silently skipped
- Version monotonicity enforced: new version must be strictly greater than stored
- Expiration checked against `Utc::now()`

## Gotchas
- `ceremony.rs` generates keys and initial root metadata -- offline operation
- `verify.rs` does signature verification, rollback/freeze/consistency checks
- `client.rs` handles TUF client update flow (fetch, verify, store)
- `keys.rs` has `canonical_json()` for deterministic serialization before signing
- `generate.rs` is feature-gated behind `--features server` (server-side metadata generation)
- Key IDs are SHA-256 of canonical JSON of the public key

## Files
- `mod.rs` -- `TrustError` enum, module re-exports
- `ceremony.rs` -- `generate_role_key()`, `create_initial_root()`
- `verify.rs` -- `verify_signatures()`, `verify_ed25519_signature()`, expiration/rollback checks
- `client.rs` -- TUF client update protocol
- `keys.rs` -- `canonical_json()`, `compute_key_id()`, `sign_tuf_metadata()`
- `metadata.rs` -- all TUF metadata structs (`RootMetadata`, `Signed`, `TufKey`, etc.)
- `generate.rs` -- server-side metadata generation (feature-gated)
