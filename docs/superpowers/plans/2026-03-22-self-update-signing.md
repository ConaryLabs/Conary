# Self-Update Ed25519 Signing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Ed25519 signature verification to the self-update pipeline — signing in CI, serving via Remi, verifying on client.

**Architecture:** Signing happens in GitHub Actions using a Rust helper binary. The signature is stored as a `.sig` sidecar file on Remi, read during version scanning, and served in the `/latest` JSON response. The client verifies before downloading, using compile-time pinned public keys.

**Tech Stack:** ed25519_dalek, base64, sha2, hex (all existing deps)

**Spec:** `docs/superpowers/specs/2026-03-22-self-update-signing-design.md`

---

## File Map

| File | Role |
|------|------|
| `conary-core/src/self_update.rs` | Client: add signature field, error type, verification function, policy |
| `conary-core/examples/sign_hash.rs` | New: signing helper binary for CI |
| `conary-server/src/server/handlers/self_update.rs` | Server: add signature to response, read/cache .sig files |
| `src/commands/self_update.rs` | CLI: verify before download, handle --force path |
| `scripts/sign-release.sh` | New: shell wrapper around the signing helper |
| `.github/workflows/release.yml` | CI: add signing step |

---

### Task 1: Client — add UpdateSignatureError and verification function

**Files:**
- Modify: `conary-core/src/self_update.rs:1-30` (imports, types)
- Test: `conary-core/src/self_update.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)]` module at the bottom of `self_update.rs` (create one if it doesn't exist):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_keypair() -> (String, String) {
        // Generate a test keypair, return (hex_public_key, hex_private_seed)
        use ed25519_dalek::SigningKey;
        let seed: [u8; 32] = [42u8; 32]; // deterministic for tests
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = signing_key.verifying_key();
        (hex::encode(public_key.as_bytes()), hex::encode(seed))
    }

    fn sign_hash(sha256_hex: &str, seed_hex: &str) -> String {
        use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
        use ed25519_dalek::{Signer, SigningKey};
        let seed_bytes: [u8; 32] = hex::decode(seed_hex).unwrap().try_into().unwrap();
        let key = SigningKey::from_bytes(&seed_bytes);
        let sig = key.sign(sha256_hex.as_bytes());
        BASE64.encode(sig.to_bytes())
    }

    #[test]
    fn test_verify_valid_signature() {
        let (pubkey, seed) = test_keypair();
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let sig = sign_hash(hash, &seed);
        let result = verify_update_signature_with_keys(hash, &sig, &[&pubkey]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_tampered_hash_fails() {
        let (pubkey, seed) = test_keypair();
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let sig = sign_hash(hash, &seed);
        let tampered = "0000001234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let result = verify_update_signature_with_keys(tampered, &sig, &[&pubkey]);
        assert!(matches!(result, Err(UpdateSignatureError::Untrusted)));
    }

    #[test]
    fn test_verify_wrong_key_fails() {
        let (_pubkey, seed) = test_keypair();
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let sig = sign_hash(hash, &seed);
        let wrong_key = "0000000000000000000000000000000000000000000000000000000000000000";
        // This will fail because the wrong key bytes won't form a valid point
        // or won't verify the signature
        let result = verify_update_signature_with_keys(hash, &sig, &[wrong_key]);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_malformed_base64_fails() {
        let (pubkey, _seed) = test_keypair();
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let result = verify_update_signature_with_keys(hash, "not-valid-base64!!!", &[&pubkey]);
        assert!(matches!(result, Err(UpdateSignatureError::Malformed(_))));
    }

    #[test]
    fn test_verify_empty_key_list_fails() {
        let (_pubkey, seed) = test_keypair();
        let hash = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let sig = sign_hash(hash, &seed);
        let result = verify_update_signature_with_keys(hash, &sig, &[]);
        assert!(matches!(result, Err(UpdateSignatureError::Untrusted)));
    }
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test -p conary-core self_update::tests -- --nocapture 2>&1 || true`
Expected: FAIL — `verify_update_signature_with_keys` and `UpdateSignatureError` don't exist.

- [ ] **Step 3: Implement UpdateSignatureError and verify function**

Add to `conary-core/src/self_update.rs`, after the imports:

```rust
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// Trusted Ed25519 public keys for self-update signature verification.
/// Hex-encoded 32-byte public keys. Add new keys BEFORE removing old
/// ones — a release with key N+1 must ship before signing with N+1.
const TRUSTED_UPDATE_KEYS: &[&str] = &[
    // TODO: Generate real key and add here before first signed release
];

/// Error from self-update signature verification.
#[derive(Debug, thiserror::Error)]
pub enum UpdateSignatureError {
    /// Signature is valid Ed25519 but no trusted key verified it.
    #[error("invalid signature: no trusted key verified the signature")]
    Untrusted,
    /// Signature data is malformed (bad base64, wrong length, etc.).
    #[error("malformed signature data: {0}")]
    Malformed(String),
}

/// Verify an Ed25519 signature over a SHA-256 hash string.
///
/// Tries each key in `trusted_keys`. Returns `Ok(())` if any validates.
/// This is the inner implementation; the public wrapper uses `TRUSTED_UPDATE_KEYS`.
fn verify_update_signature_with_keys(
    sha256_hex: &str,
    signature_base64: &str,
    trusted_keys: &[&str],
) -> std::result::Result<(), UpdateSignatureError> {
    let sig_bytes = BASE64
        .decode(signature_base64)
        .map_err(|e| UpdateSignatureError::Malformed(format!("base64 decode: {e}")))?;

    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| UpdateSignatureError::Malformed(format!("signature format: {e}")))?;

    for key_hex in trusted_keys {
        let key_bytes = match hex::decode(key_hex) {
            Ok(b) => b,
            Err(_) => continue, // skip malformed keys
        };
        let key_array: [u8; 32] = match key_bytes.try_into() {
            Ok(a) => a,
            Err(_) => continue,
        };
        let verifying_key = match VerifyingKey::from_bytes(&key_array) {
            Ok(k) => k,
            Err(_) => continue,
        };
        if verifying_key.verify(sha256_hex.as_bytes(), &signature).is_ok() {
            return Ok(());
        }
    }

    Err(UpdateSignatureError::Untrusted)
}

/// Verify a self-update signature against the compiled-in trusted keys.
///
/// Skipped entirely in test builds (`cfg!(test)`).
pub fn verify_update_signature(
    sha256_hex: &str,
    signature_base64: &str,
) -> std::result::Result<(), UpdateSignatureError> {
    if cfg!(test) {
        return Ok(());
    }
    verify_update_signature_with_keys(sha256_hex, signature_base64, TRUSTED_UPDATE_KEYS)
}
```

- [ ] **Step 4: Run tests, verify they pass**

Run: `cargo test -p conary-core self_update::tests -- --nocapture`
Expected: All 5 tests PASS. Note: tests call `verify_update_signature_with_keys` directly (not the `cfg!(test)`-guarded wrapper).

- [ ] **Step 5: Commit**

```
feat: add Ed25519 signature verification for self-update

Add UpdateSignatureError type and verify_update_signature_with_keys()
that tries each trusted key in order. Public wrapper skips in test
builds. TRUSTED_UPDATE_KEYS is empty until the first signed release.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

### Task 2: Client — add signature field to LatestVersionInfo and VersionCheckResult

**Files:**
- Modify: `conary-core/src/self_update.rs:24-45` (struct and enum definitions)

- [ ] **Step 1: Add signature to LatestVersionInfo**

In `LatestVersionInfo` (line 24-30), add:
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct LatestVersionInfo {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub signature: Option<String>,
}
```

- [ ] **Step 2: Add signature to VersionCheckResult::UpdateAvailable**

In `VersionCheckResult` (line 33-45), add `signature` field. **Preserve existing derives** (`#[derive(Debug, Clone, PartialEq)]`):
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum VersionCheckResult {
    UpdateAvailable {
        current: String,
        latest: String,
        download_url: String,
        sha256: String,
        size: u64,
        signature: Option<String>,
    },
    UpToDate { version: String },
}
```

- [ ] **Step 3: Update check_for_update to propagate signature**

In `check_for_update` (line 176), add `signature: info.signature`:
```rust
Ok(VersionCheckResult::UpdateAvailable {
    current: current_version.to_string(),
    latest: info.version,
    download_url: info.download_url,
    sha256: info.sha256,
    size: info.size,
    signature: info.signature,
})
```

- [ ] **Step 4: Build and fix all compilation errors**

Run: `cargo build`

This will break in two places:
1. `src/commands/self_update.rs` — pattern matches on `VersionCheckResult::UpdateAvailable` need `signature` added (or `..`). There are ~3 destructuring sites.
2. `conary-core/src/self_update.rs` — the existing test `test_version_check_result_variants` (around line 505) constructs `UpdateAvailable` without `signature`. Add `signature: None` to the test construction.

Fix all of them.

- [ ] **Step 5: Commit**

```
feat: add signature field to LatestVersionInfo and VersionCheckResult

Propagates the optional Ed25519 signature from the /latest response
through the version check pipeline. Uses #[serde(default)] for
backward compatibility with older Remi servers.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

### Task 3: CLI — verify signature before download

**Files:**
- Modify: `src/commands/self_update.rs:13-114`

- [ ] **Step 1: Extract a shared verification helper function**

Per the spec, both code paths must call the same function. Add this helper to `src/commands/self_update.rs`:

```rust
/// Verify the self-update signature, applying the absent/present/invalid policy.
/// Present + valid = Ok, present + invalid = error, absent = warn + Ok.
fn check_update_signature(sha256: &str, signature: &Option<String>) -> Result<()> {
    match signature {
        Some(sig) => {
            conary_core::self_update::verify_update_signature(sha256, sig)
                .map_err(|e| anyhow::anyhow!("Update signature verification failed: {e}"))?;
            println!("Signature verified");
        }
        None => {
            eprintln!("Warning: update has no signature (pre-signing release)");
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Call the helper in the normal update path**

After the match on `result` (around line 57), before determining download URL:

```rust
if let VersionCheckResult::UpdateAvailable { ref sha256, ref signature, .. } = result {
    check_update_signature(sha256, signature)?;
}
```

- [ ] **Step 3: Call the helper in the --force re-fetch path**

In the `VersionCheckResult::UpToDate` arm (line 66-73), after re-fetching `LatestVersionInfo`:

```rust
VersionCheckResult::UpToDate { .. } => {
    let info: LatestVersionInfo = reqwest::get(format!("{channel_url}/latest"))
        .await?
        .json()
        .await?;
    check_update_signature(&info.sha256, &info.signature)?;
    (info.download_url, info.sha256, info.version)
}
```

- [ ] **Step 4: Build and verify**

Run: `cargo build`

- [ ] **Step 4: Commit**

```
feat: verify self-update signature before downloading

Both the normal update path and the --force re-fetch path now
verify the Ed25519 signature against trusted keys before
downloading. Present+invalid = hard error. Absent = warn+allow.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

### Task 4: Server — add signature to LatestResponse, read .sig files

**Files:**
- Modify: `conary-server/src/server/handlers/self_update.rs:31-36, 42-48, 80, 85-151, 200-244`

- [ ] **Step 1: Add signature to LatestResponse**

```rust
#[derive(Serialize)]
pub struct LatestResponse {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}
```

- [ ] **Step 2: Extend VersionsCacheEntry and LatestHash to include signature**

Change the `LatestHash` type alias and cache struct:
```rust
/// Precomputed hash, size, and optional signature for the latest CCS package.
type LatestHash = Option<(String, u64, Option<String>)>;

struct VersionsCacheEntry {
    fetched_at: Instant,
    versions: Vec<String>,
    latest_hash: LatestHash,
}
```

- [ ] **Step 3: Read .sig file in scan_versions_and_hash**

In `scan_versions_and_hash`, after computing SHA-256 (around line 130-148), read the `.sig` sidecar:

```rust
let latest_hash = if let Some(latest) = versions.last() {
    let ccs_path = dir.join(format!("conary-{latest}.ccs"));
    let sig_path = dir.join(format!("conary-{latest}.ccs.sig"));
    match std::fs::read(&ccs_path) {
        Ok(data) => {
            let sha256 = conary_core::hash::sha256(&data);
            let size = data.len() as u64;
            // Read signature if available
            let signature = std::fs::read_to_string(&sig_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some((sha256, size, signature))
        }
        Err(e) => {
            tracing::warn!("Failed to hash latest CCS: {}: {}", ccs_path.display(), e);
            None
        }
    }
} else {
    None
};
```

- [ ] **Step 4: Update get_latest handler to include signature**

In `get_latest` (around line 230), update the destructuring and response:
```rust
let (sha256, size, signature) = match latest_hash {
    Some(cached) => cached,
    None => {
        // Fallback path...
        let data = match tokio::fs::read(&ccs_path).await { ... };
        let sig_path = dir.join(format!("conary-{latest}.ccs.sig"));
        let signature = tokio::fs::read_to_string(&sig_path)
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        (conary_core::hash::sha256(&data), data.len() as u64, signature)
    }
};

let response = LatestResponse {
    version: latest.clone(),
    download_url: format!("/v1/ccs/conary/{latest}/download"),
    sha256,
    size,
    signature,
};
```

- [ ] **Step 5: Build and verify**

Run: `cargo build --features server`

- [ ] **Step 6: Commit**

```
feat: serve self-update signature in /latest response

Reads .sig sidecar files during version scanning and caches
alongside the SHA-256 hash. LatestResponse now includes an
optional signature field (omitted when null via skip_serializing_if).

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

### Task 5: Signing helper binary

**Files:**
- Create: `conary-core/examples/sign_hash.rs`

- [ ] **Step 1: Write the signing helper**

```rust
// conary-core/examples/sign_hash.rs

//! Ed25519 signing helper for self-update CCS packages.
//!
//! Usage: sign_hash <path-to-ccs-file>
//!
//! Reads RELEASE_SIGNING_KEY env var (hex-encoded 32-byte Ed25519 seed),
//! computes SHA-256 of the file, signs the hex hash, prints base64 signature
//! to stdout (no trailing newline).
//!
//! Exit codes: 0 = success, 1 = error

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::{env, fs, process};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: sign_hash <path-to-ccs-file>");
        process::exit(1);
    }
    let ccs_path = &args[1];

    // Read and validate signing key
    let key_hex = match env::var("RELEASE_SIGNING_KEY") {
        Ok(k) if k.len() == 64 && k.chars().all(|c| c.is_ascii_hexdigit()) => k,
        Ok(k) if k.is_empty() => {
            eprintln!("Error: RELEASE_SIGNING_KEY is empty");
            process::exit(1);
        }
        Ok(_) => {
            eprintln!("Error: RELEASE_SIGNING_KEY must be 64 hex characters (32-byte Ed25519 seed)");
            process::exit(1);
        }
        Err(_) => {
            eprintln!("Error: RELEASE_SIGNING_KEY environment variable not set");
            process::exit(1);
        }
    };

    let seed_bytes: [u8; 32] = hex::decode(&key_hex)
        .expect("already validated hex")
        .try_into()
        .expect("already validated length");
    let signing_key = SigningKey::from_bytes(&seed_bytes);

    // Compute SHA-256 of file
    let mut file = match fs::File::open(ccs_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: cannot open {ccs_path}: {e}");
            process::exit(1);
        }
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).unwrap_or_else(|e| {
            eprintln!("Error: read failed: {e}");
            process::exit(1);
        });
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let sha256_hex = hex::encode(hasher.finalize());

    // Sign the hex hash string
    let signature = signing_key.sign(sha256_hex.as_bytes());
    let sig_base64 = BASE64.encode(signature.to_bytes());

    // Print to stdout with no trailing newline
    print!("{sig_base64}");
}
```

Also include a `--show-public-key` flag (needed in Task 8 for key generation):
```rust
// At the top of main(), before the file path argument check:
if args.get(1).map(|s| s.as_str()) == Some("--show-public-key") {
    let key_hex = env::var("RELEASE_SIGNING_KEY").unwrap_or_else(|_| {
        eprintln!("Error: RELEASE_SIGNING_KEY not set");
        process::exit(1);
    });
    // Validate and parse key (same validation as the signing path)
    let seed_bytes: [u8; 32] = hex::decode(&key_hex)
        .and_then(|b| b.try_into().map_err(|_| hex::FromHexError::InvalidStringLength))
        .unwrap_or_else(|_| {
            eprintln!("Error: RELEASE_SIGNING_KEY must be 64 hex characters");
            process::exit(1);
        });
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    let public_key = signing_key.verifying_key();
    print!("{}", hex::encode(public_key.as_bytes()));
    return;
}
```

- [ ] **Step 2: Build and test the example**

```bash
cargo build --example sign_hash -p conary-core
# Quick smoke test with a dummy key and file:
echo "test content" > /tmp/test.ccs
RELEASE_SIGNING_KEY=$(python3 -c "import secrets; print(secrets.token_hex(32))") \
    ./target/debug/examples/sign_hash /tmp/test.ccs
# Should print a base64 string to stdout
```

- [ ] **Step 3: Commit**

```
feat: add sign_hash example binary for CI release signing

Reads RELEASE_SIGNING_KEY env var, computes SHA-256 of a CCS file,
signs the hex hash with Ed25519, outputs base64 signature to stdout.
Validates key format on startup.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

### Task 6: Signing shell script

**Files:**
- Create: `scripts/sign-release.sh`

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# scripts/sign-release.sh
#
# Sign a CCS self-update package for release.
# Requires RELEASE_SIGNING_KEY env var (hex-encoded 32-byte Ed25519 seed).
#
# Usage: sign-release.sh <path-to-ccs-file>
# Output: creates <path-to-ccs-file>.sig

set -euo pipefail

if [ $# -ne 1 ]; then
    echo "Usage: $0 <path-to-ccs-file>" >&2
    exit 1
fi

CCS_FILE="$1"
SIG_FILE="${CCS_FILE}.sig"

if [ ! -f "$CCS_FILE" ]; then
    echo "Error: CCS file not found: $CCS_FILE" >&2
    exit 1
fi

if [ -z "${RELEASE_SIGNING_KEY:-}" ]; then
    echo "Error: RELEASE_SIGNING_KEY not set" >&2
    exit 1
fi

# Build the signing helper if needed
SIGN_BIN="./target/release/examples/sign_hash"
if [ ! -f "$SIGN_BIN" ]; then
    echo "Building sign_hash helper..."
    cargo build --example sign_hash -p conary-core --release --quiet
fi

# Sign
echo "Signing $CCS_FILE..."
"$SIGN_BIN" "$CCS_FILE" > "$SIG_FILE"

# Validate output
if [ ! -s "$SIG_FILE" ]; then
    echo "Error: signing produced empty output" >&2
    rm -f "$SIG_FILE"
    exit 1
fi

echo "Signature written to $SIG_FILE"
```

- [ ] **Step 2: Make executable**

```bash
chmod +x scripts/sign-release.sh
```

- [ ] **Step 3: Commit**

```
feat: add sign-release.sh script for CI release signing

Wrapper around sign_hash example binary. Builds if needed, signs
the CCS file, validates non-empty output.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

### Task 7: GitHub Actions — add signing step to release workflow

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Read the current release.yml to find the right insertion point**

The signing step goes in the `release` job, after artifacts are downloaded and before the SSH upload to Remi. Find the section that uploads `conary-*.ccs` to `/conary/self-update/`.

- [ ] **Step 2: Ensure Rust toolchain is available in the release job**

The release job (`ubuntu-latest`) does not currently install Rust. Either:
- (a) Build `sign_hash` in the `build-ccs` job (which already has Rust) and pass it as an artifact, OR
- (b) Add `actions-rust-lang/setup-rust-toolchain@v1` to the release job

Option (a) is preferred — more efficient and consistent with existing structure. Add `sign_hash` to the build-ccs job's artifact upload.

- [ ] **Step 3: Add signing step**

After the CCS artifact is available but before uploading to Remi, add:

```yaml
    - name: Sign CCS package
      env:
        RELEASE_SIGNING_KEY: ${{ secrets.RELEASE_SIGNING_KEY }}
      run: |
        CCS_FILE=$(ls conary-*.ccs | head -1)
        if [ -n "$CCS_FILE" ] && [ -n "${RELEASE_SIGNING_KEY:-}" ]; then
          cargo build --example sign_hash -p conary-core --release --quiet
          ./target/release/examples/sign_hash "$CCS_FILE" > "${CCS_FILE}.sig"
          echo "Signed: $CCS_FILE -> ${CCS_FILE}.sig"
        else
          echo "Skipping signing (no CCS or no key)"
        fi
```

- [ ] **Step 3: Update the SSH upload section to include .sig files**

Find the `scp` or `rsync` command that uploads to `/conary/self-update/` and ensure `.sig` files are included in the pattern.

- [ ] **Step 4: Commit**

```
feat: add Ed25519 signing step to release workflow

Signs the CCS self-update package during release. Requires
RELEASE_SIGNING_KEY secret. Gracefully skips if key is not
configured (first deployment will need the secret added).

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

### Task 8: Generate real keypair and document

**Files:**
- Modify: `conary-core/src/self_update.rs` (update `TRUSTED_UPDATE_KEYS`)

- [ ] **Step 1: Generate the keypair**

Generate a random 32-byte seed and derive the public key using the `--show-public-key` flag from Task 5:

```bash
# Generate a random seed
SEED=$(python3 -c "import secrets; print(secrets.token_hex(32))")
echo "Private seed (store as RELEASE_SIGNING_KEY secret): $SEED"

# Derive the public key
cargo build --example sign_hash -p conary-core
RELEASE_SIGNING_KEY="$SEED" ./target/debug/examples/sign_hash --show-public-key
# Outputs the hex public key to stdout
```

Save the seed securely — it goes into GitHub secrets. The public key goes into source.

- [ ] **Step 2: Add the public key to TRUSTED_UPDATE_KEYS**

Replace the empty array with the real key:
```rust
const TRUSTED_UPDATE_KEYS: &[&str] = &[
    "actual_hex_public_key_here", // Primary key (2026-03-22)
];
```

- [ ] **Step 3: Add RELEASE_SIGNING_KEY to GitHub secrets**

This is a manual step: go to GitHub repo Settings > Secrets > Actions and add `RELEASE_SIGNING_KEY` with the hex seed value.

- [ ] **Step 4: Commit**

```
feat: add initial self-update signing public key

Compile the Ed25519 public key for the first signing key into
TRUSTED_UPDATE_KEYS. The corresponding private seed is stored
as RELEASE_SIGNING_KEY in GitHub Actions secrets.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
```

---

## Summary

| Task | What | Files |
|------|------|-------|
| 1 | Error type + verification function + tests | `conary-core/src/self_update.rs` |
| 2 | Signature field on types + propagation | `conary-core/src/self_update.rs` |
| 3 | CLI verification before download | `src/commands/self_update.rs` |
| 4 | Server: read .sig, serve in /latest | `conary-server/src/server/handlers/self_update.rs` |
| 5 | Signing helper binary | `conary-core/examples/sign_hash.rs` |
| 6 | Signing shell script | `scripts/sign-release.sh` |
| 7 | GitHub Actions integration | `.github/workflows/release.yml` |
| 8 | Generate real keypair, add to source + GitHub | `conary-core/src/self_update.rs` + manual |

Tasks 1-4 can be implemented and tested without the signing infrastructure.
Tasks 5-7 set up the CI pipeline.
Task 8 is a manual step to generate and deploy the real key.
