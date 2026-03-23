# Self-Update Ed25519 Signature Verification

**Goal:** Add cryptographic signature verification to the self-update pipeline so clients can verify that downloaded binaries were produced by the official release process, not a compromised server or MITM.

**Status:** Design approved, pending implementation.

---

## Architecture

Signatures are generated in GitHub Actions during the release job, stored as sidecar `.sig` files on Remi, served in the `/latest` JSON response, and verified client-side against compile-time pinned Ed25519 public keys.

**What is signed:** The hex-encoded SHA-256 hash of the CCS file (the same hash the client already computes during download). Signing the hash avoids reading the full file twice and keeps the signed payload small (64 bytes).

**Signature format:** Raw Ed25519 signature (64 bytes), base64-encoded for transport.

**Key encoding convention:** Hex-encoded throughout (both private seed and public key), consistent with `model/signing.rs`. The base64 encoding is used only for the signature itself in transit.

---

## Components

### 1. Signing Helper Binary

**File:** `conary-core/examples/sign_hash.rs`

A small standalone binary that:
1. Reads `RELEASE_SIGNING_KEY` env var (hex-encoded 32-byte Ed25519 seed)
2. Computes SHA-256 of a given file
3. Signs the hex hash string with Ed25519
4. Writes base64 signature to stdout (no trailing newline)

Validates key format on startup — exits non-zero with a clear message if `RELEASE_SIGNING_KEY` is unset, empty, or not valid 64-char hex.

**Why a Rust binary:** The project already depends on `ed25519_dalek` and `sha2`. A Rust example binary avoids introducing `openssl` as a dependency and guarantees the signing logic matches the verification logic exactly.

### 2. Signing Shell Script

**File:** `scripts/sign-release.sh`

Wrapper that:
1. Builds the signing helper: `cargo build --example sign_hash -p conary-core`
2. Runs it on the CCS file: `./target/debug/sign_hash path/to/conary-{VERSION}.ccs > path/to/conary-{VERSION}.ccs.sig`
3. Validates the `.sig` file was created and is non-empty

The output `.sig` file contains only the base64 signature string, no trailing newline, no envelope.

### 3. GitHub Actions Integration

**File:** `.github/workflows/release.yml`

In the release job, after the CCS is built but before upload to Remi:
1. Build the signing helper (reuses the workspace already compiled in the build-ccs job)
2. Run `scripts/sign-release.sh` with `RELEASE_SIGNING_KEY` from secrets
3. Upload both `conary-{VERSION}.ccs` and `conary-{VERSION}.ccs.sig` to Remi at `/conary/self-update/`

**New secret:** `RELEASE_SIGNING_KEY` — hex-encoded 32-byte Ed25519 seed. Same trust level as `REMI_SSH_KEY` (which already has root access to Remi).

### 4. Server Changes

**File:** `conary-server/src/server/handlers/self_update.rs`

Add `signature` field to `LatestResponse`:
```rust
pub struct LatestResponse {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,  // base64 Ed25519 signature over sha256 hex
}
```

**Caching:** The `VersionsCacheEntry` (or equivalent) stores the signature string alongside the SHA-256 and size. During `scan_versions`, if `conary-{VERSION}.ccs.sig` exists, read its contents (stripping whitespace) and cache. If absent, store `None`. This avoids re-reading the `.sig` file on every `/latest` request.

### 5. Client Changes

**File:** `conary-core/src/self_update.rs`

**New field on `LatestVersionInfo`:**
```rust
#[serde(default)]  // backward compat with older Remi servers
pub signature: Option<String>,
```

**Propagate through `VersionCheckResult`:**
```rust
pub enum VersionCheckResult {
    UpdateAvailable {
        current: String,
        latest: String,
        download_url: String,
        sha256: String,
        size: u64,
        signature: Option<String>,  // NEW
    },
    UpToDate { version: String },
}
```

**Trusted keys array:**
```rust
/// Trusted Ed25519 public keys for self-update signature verification.
/// Hex-encoded 32-byte public keys. Add new keys BEFORE removing old
/// ones — a release with key N+1 must ship before signing with N+1.
const TRUSTED_UPDATE_KEYS: &[&str] = &[
    "...", // Primary key (2026-XX-XX)
];
```

**Verification function:**
```rust
/// Verify an Ed25519 signature over a SHA-256 hash string.
/// Returns Ok(()) if any trusted key validates the signature.
/// Returns Err(UpdateSignatureError) if no key matches or input is malformed.
fn verify_update_signature(sha256_hex: &str, signature_base64: &str) -> Result<(), UpdateSignatureError>
```

**Error type:**
```rust
#[derive(Debug, thiserror::Error)]
pub enum UpdateSignatureError {
    #[error("invalid signature: no trusted key verified the signature")]
    Untrusted,
    #[error("malformed signature data: {0}")]
    Malformed(String),
}
```

**Verification timing:** Verify BEFORE download, not after. The signature covers the SHA-256 hash, which is already known from the `/latest` response. If the signature is invalid, there is no reason to download the file. This also keeps the verification logic out of the streaming download path.

**Verification policy:**

| Condition | Behavior |
|-----------|----------|
| Signature present + valid | Proceed to download |
| Signature present + invalid | Hard error, refuse update |
| Signature absent (`null`) | Warn, allow (backward compat with pre-signing releases) |
| `cfg!(test)` | Skip verification (test-only bypass) |

Note: Debug builds verify normally. The `cfg!(debug_assertions)` skip from the earlier draft was removed — developer machines should also reject tampered updates. Only `cfg!(test)` skips verification to avoid needing real keys in unit tests.

**`--force` path:** The `--force` re-fetch path in `cmd_self_update()` that bypasses `check_for_update()` must also parse and verify the signature from the raw `/latest` response. Extract the verification into a standalone function that both code paths call.

### 6. Key Management

**Initial setup (one-time):**
1. Generate Ed25519 keypair (can use the signing helper with a `--generate` flag)
2. Store private key (hex seed) as GitHub secret `RELEASE_SIGNING_KEY`
3. Compile public key (hex) into `TRUSTED_UPDATE_KEYS` in source

**Key rotation:**
1. Generate new keypair
2. Add new public key to `TRUSTED_UPDATE_KEYS` array, release
3. Switch `RELEASE_SIGNING_KEY` secret to new private key
4. Future release removes old public key from array

This two-phase approach ensures clients always have the new key before signatures switch to using it.

---

## What Doesn't Change

- Download flow (still streams with inline SHA-256 verification)
- Apply flow (still atomic rename)
- `verify_binary` (still checks `--version` output)
- Version comparison logic
- Custom update channels (signature is optional, channels without signing still work)
- `/versions` and `/download` endpoints (unchanged)

---

## Data Flow

```
Release:
  GitHub Actions builds CCS
  → sign_hash example: SHA-256(ccs) → Ed25519.sign(hash, RELEASE_SIGNING_KEY) → .sig file
  → Upload ccs + sig to Remi /conary/self-update/

Client check:
  GET /v1/ccs/conary/latest
  → { version, download_url, sha256, size, signature }

Client verify (BEFORE download):
  If signature present: verify_update_signature(sha256, signature)
  → If valid: proceed to download
  → If invalid: abort with error
  If signature absent: warn, proceed to download

Client download + apply (unchanged):
  Stream CCS, verify SHA-256 inline
  Extract binary, atomic rename
```

---

## Testing

- **Unit test:** `verify_update_signature` with a test keypair — valid sig passes, tampered hash fails, wrong key fails, malformed base64 returns `Malformed` error
- **Unit test:** Absent signature warns but allows update to proceed
- **Unit test:** `VersionCheckResult::UpdateAvailable` carries signature through pipeline
- **Unit test:** `UpdateSignatureError` variants have correct messages
- **Integration:** `sign_hash` example produces a signature that `verify_update_signature` accepts
- **Server test:** `LatestResponse` includes signature when `.sig` file exists, omits field when absent (via `skip_serializing_if`)
- **Server test:** `.sig` file contents are cached in `VersionsCacheEntry`

---

## Files Modified

| File | Change |
|------|--------|
| `conary-core/src/self_update.rs` | Add signature field, `UpdateSignatureError`, verification function, policy |
| `conary-core/examples/sign_hash.rs` | New — signing helper binary |
| `conary-server/src/server/handlers/self_update.rs` | Add signature to response, read/cache .sig files |
| `scripts/sign-release.sh` | New — signing script wrapper |
| `.github/workflows/release.yml` | Add signing step, new secret |
| `src/commands/self_update.rs` | Verify before download, handle `--force` path |

---

## Deferred

- **Require signatures always** — Currently absent signatures are allowed for backward compatibility. Tighten once all releases on Remi are signed.
- **Signature on `/versions` endpoint** — Currently only `/latest` includes the signature. Add per-version signatures if downgrade protection is needed.
- **TUF integration** — The TUF infrastructure could eventually manage self-update metadata. This design is simpler and sufficient for now.
- **`--skip-verify` CLI flag** — Could add for developer convenience, but not needed for v1.
