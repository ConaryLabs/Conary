# M1a Recipe Static Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the M1a recipe-only static repository loop: `conary cook`, `conary publish` to a static repo, `conary repo add` for that repo, sync, and install from it.

**Architecture:** Reuse the existing Kitchen, CCS, TUF, repository sync, resolver, and install pipelines. Add a focused static-repo layer under `crates/conary-core/src/repository/static_repo/`, keep CLI modules as thin routers, and keep trust-sensitive behavior in the existing `trust/` and `ccs/` boundaries. Static repos use TUF for metadata/index/package-key trust, then CCS Ed25519 package signatures for package trust.

**Tech Stack:** Rust 2024, clap, tokio/reqwest plus filesystem fetches, rusqlite, serde/toml/serde_json, existing TUF metadata helpers, existing Kitchen and CCS builder/signing code.

---

## Scope

In scope:

- M1a command surfaces: recipe-driven `conary cook`, project-form `conary publish <target>`, repeatable-fingerprint `conary repo add <name> <url|path> --fingerprint <root-key-id>`, `conary repo reset-trust <name>`, `conary repo sync`, and install from the synced static repo.
- Static repo v1 format from `docs/specs/static-repo-format-v1.md`.
- File/local-path support for static repo metadata, index, packages, Kitchen source downloads, and recipe `[source] path = "."` local workspaces.
- Package signing trust from TUF-verified `keys/package-keys.json`.
- Preview-honest provenance: `origin_class = native-built`, `hardening_level = host|sandboxed`, never `hermetic`, and no build attestation in M1a.

Out of scope:

- M1b inference, `conary new`, `conary try`, tarball/git target routing.
- M2 artifact-form publish, Remi push, hermetic/offline builds, attestations.
- TUF delegations, consistent-snapshot versioned targets/snapshot filenames, chunk semantics.

## Current Repo Facts

- Top-level `publish` does not exist; existing publish surfaces are model/profile/generation-specific (`apps/conary/src/cli/mod.rs`, `apps/conary/src/cli/model.rs`, `apps/conary/src/cli/generation.rs`).
- `conary cook` exists, but currently requires a recipe path and exposes `--no-isolation`/`--hermetic`; M1a needs the parent-spec shape: host by default, `--isolated` for Kitchen isolation, no hermetic claim.
- `repo add` exists with GPG flags only (`apps/conary/src/cli/repo.rs`, `apps/conary/src/commands/repo.rs`).
- `repo sync` already runs TUF when `repo.tuf_enabled` is true, then continues into native/JSON sync (`crates/conary-core/src/repository/sync.rs`).
- TUF fetches are HTTP-only and default to `<repo>/tuf`; static repos need `<repo>/metadata` and filesystem fallback (`crates/conary-core/src/trust/client.rs`).
- Existing `verify_snapshot_consistency` accepts missing `root.json` / `targets.json` snapshot entries; static repos must hard-fail.
- Existing timestamp monotonicity rejects equal versions; static repos need equal-version no-change sync when the stored metadata hash matches.
- `SigningKeyPair::save_to_files` writes then chmods; M1a must create private files 0600 at open time and create key dirs 0700 before writes.
- No DB table persists CCS package signing keys from static repos; M1a needs one so install can verify packages against TUF-verified `keys/package-keys.json`.
- `crates/conary-core/src/repository/sync.rs` is 1752 lines. Preserve it as the sync orchestrator and add focused child modules instead of growing it substantially.

## File Map

Create:

- `crates/conary-core/src/repository/static_repo/mod.rs` - module hub and public core API.
- `crates/conary-core/src/repository/static_repo/format.rs` - `conary-repo.toml`, `index.json`, `package-keys.json` structs and validation.
- `crates/conary-core/src/repository/static_repo/location.rs` - HTTP/HTTPS/file/bare-path base handling and bounded byte fetches.
- `crates/conary-core/src/repository/static_repo/paths.rs` - repo-relative path normalization and traversal rejection.
- `crates/conary-core/src/repository/static_repo/publish.rs` - file-based static publisher, watermark, reverse upload order, refresh cascades.
- `crates/conary-core/src/repository/static_repo/sync.rs` - static index/package-key sync mapping into repository DB rows.
- `crates/conary-core/src/db/models/repository_package_key.rs` - persisted CCS package key trust for static repos.
- `apps/conary/src/commands/publish.rs` - top-level publish command implementation.
- `apps/conary/src/commands/repo_static.rs` - static `repo add`, fingerprint/TOFU/reset-trust helpers.
- `apps/conary/tests/static_repo_m1a.rs` - CLI integration coverage for local static repo flow.

Modify:

- `crates/conary-core/src/repository/mod.rs` - export static repo module/types.
- `crates/conary-core/src/repository/sync.rs`, `sync/types.rs`, `sync/native.rs` - route `default_strategy = "static"` into static sync and persist package keys.
- `crates/conary-core/src/repository/client.rs` - allow local/file static fetches without weakening native distro SSRF checks.
- `crates/conary-core/src/repository/download.rs` - download local/file package URLs and preserve checksum cleanup.
- `crates/conary-core/src/repository/resolution.rs` - carry static source kind on `RepositorySourceMetadata` for packages selected by the core resolver; install-layer provenance and signature enforcement remain in `apps/conary/src/commands/install/`.
- `crates/conary-core/src/trust/client.rs` - filesystem metadata fetch, static metadata base support, timestamp no-change, snapshot presence hard-fail.
- `crates/conary-core/src/trust/verify.rs` - static snapshot meta presence helper.
- `crates/conary-core/src/trust/ceremony.rs` - publish-key rotation helper that updates targets/snapshot/timestamp together.
- `crates/conary-core/src/ccs/signing.rs` - 0700 key dir and 0600 private-file-at-open behavior.
- `crates/conary-core/src/ccs/manifest.rs` and `recipe/kitchen/provenance_capture.rs` - origin/hardening provenance fields.
- `crates/conary-core/src/recipe/format.rs`, `recipe/kitchen/archive.rs`, and `recipe/kitchen/cook.rs` - remote/archive source compatibility plus local workspace source support.
- `crates/conary-core/src/recipe/kitchen/config.rs` and `apps/conary/src/commands/cook.rs` - M1a cook mode semantics.
- `crates/conary-core/src/db/schema.rs`, `db/migrations/v41_current.rs`, `db/models/mod.rs` - schema v72 package-key persistence.
- `apps/conary/src/cli/mod.rs`, `cli/repo.rs`, `dispatch/root.rs`, `dispatch/repo.rs`, `commands/mod.rs` - CLI and dispatch wiring.
- `apps/conary/src/commands/install/acquire.rs`, `install/conversion.rs`, `install/options.rs`, `install/resolve.rs` - static CCS signature verification before install.
- `docs/modules/feature-ownership.md`, `docs/llms/subsystem-map.md`, `docs/ARCHITECTURE.md` - new look-here-first routing for static repo/publish work.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv`, `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - plan registration.

---

### Task 1: Add Static Repo Format Types And Path Validation

**Files:**
- Create: `crates/conary-core/src/repository/static_repo/mod.rs`
- Create: `crates/conary-core/src/repository/static_repo/format.rs`
- Create: `crates/conary-core/src/repository/static_repo/paths.rs`
- Modify: `crates/conary-core/src/repository/mod.rs`

- [ ] **Step 1: Write format and path validation tests**

Add tests in `format.rs` and `paths.rs` covering:

```rust
#[test]
fn repo_identity_rejects_bad_name() {
    let input = r#"
schema = 1
[repo]
name = "Bad_Name"
description = "bad"
[trust]
root_key_ids = ["9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"]
"#;
    assert!(RepoIdentity::parse(input).is_err());
}

#[test]
fn target_path_rejects_percent_encoded_traversal() {
    assert!(validate_repo_relative_path("packages/%2e%2e/evil.ccs").is_err());
    assert!(validate_repo_relative_path("packages/foo%2fbar.ccs").is_err());
    assert!(validate_repo_relative_path("/packages/rooted.ccs").is_err());
    assert!(validate_repo_relative_path("https://example.test/pkg.ccs").is_err());
    assert!(validate_repo_relative_path("packages/foo..bar/pkg.ccs").is_ok());
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core repository::static_repo::format
cargo test -p conary-core repository::static_repo::paths
```

Expected: compile failure because the new module/types do not exist yet.

- [ ] **Step 3: Implement the static format API**

Use this API shape:

```rust
impl RepoIdentity {
    pub fn parse(input: &str) -> Result<Self> {
        let parsed: Self = toml::from_str(input)?;
        parsed.validate()?;
        Ok(parsed)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoIdentity {
    pub schema: u64,
    pub repo: RepoIdentityRepo,
    pub trust: RepoIdentityTrust,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoIdentityRepo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoIdentityTrust {
    pub root_key_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StaticIndex {
    pub schema: u64,
    pub name: String,
    pub index_version: u64,
    pub generated: chrono::DateTime<chrono::Utc>,
    pub packages: Vec<StaticPackageEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StaticPackageEntry {
    pub name: String,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageKeysFile {
    pub schema: u64,
    pub keys: Vec<PackageKeyEntry>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKeyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PackageKeyEntry {
    pub algorithm: String,
    pub public_key: String,
    #[serde(default)]
    pub key_id: Option<String>,
    pub status: PackageKeyStatus,
    #[serde(default)]
    pub comment: Option<String>,
}
```

Validation must reject:

- unknown schema values
- empty package-key sets for non-empty indexes
- non-hex/incorrect-length root key IDs
- non-base64 or non-32-byte package public keys
- package filename mismatch against `<name>-<version>-<release>-<arch>.ccs`
- path traversal per `docs/specs/static-repo-format-v1.md` section 3

- [ ] **Step 4: Export the module**

Add:

```rust
pub mod static_repo;
```

to `crates/conary-core/src/repository/mod.rs`, and re-export only stable internal conveniences:

```rust
pub use static_repo::{
    PackageKeyEntry, PackageKeyStatus, PackageKeysFile, RepoIdentity, StaticIndex,
    StaticPackageEntry,
};
```

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary-core repository::static_repo::format
cargo test -p conary-core repository::static_repo::paths
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/repository/mod.rs crates/conary-core/src/repository/static_repo
git commit -m "feat(static-repo): add v1 format validation"
```

### Task 2: Add HTTP/File/Local Static Fetch Support

**Files:**
- Create: `crates/conary-core/src/repository/static_repo/location.rs`
- Modify: `crates/conary-core/src/repository/client.rs`
- Modify: `crates/conary-core/src/repository/download.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/archive.rs`

- [ ] **Step 1: Write transport tests**

Cover these cases:

```rust
#[tokio::test]
async fn static_location_fetches_from_bare_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("conary-repo.toml"), b"schema = 1\n").unwrap();
    let location = RepoLocation::parse(dir.path().to_str().unwrap()).unwrap();
    let bytes = location.fetch_bytes("conary-repo.toml", 1024).await.unwrap();
    assert_eq!(bytes, b"schema = 1\n");
}

#[tokio::test]
async fn static_location_fetches_from_file_url() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("metadata")).unwrap();
    std::fs::write(dir.path().join("metadata/root.json"), b"{}").unwrap();
    let url = format!("file://{}", dir.path().display());
    let location = RepoLocation::parse(&url).unwrap();
    let bytes = location.fetch_bytes("metadata/root.json", 1024).await.unwrap();
    assert_eq!(bytes, b"{}");
}
```

Add Kitchen archive tests:

```rust
#[test]
fn download_file_accepts_local_path_sources() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.tar");
    let dest = dir.path().join("dest.tar");
    std::fs::write(&source, b"archive").unwrap();
    download_file(source.to_str().unwrap(), &dest).unwrap();
    assert_eq!(std::fs::read(dest).unwrap(), b"archive");
}
```

Add package downloader tests proving a static `RepositoryPackage.download_url` that is `file://...` or a bare local path bypasses `RepositoryClient::download_file`, copies the file, verifies the expected SHA-256, and rejects a size mismatch against `RepositoryPackage.size`.

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p conary-core repository::static_repo::location
cargo test -p conary-core repository::download
cargo test -p conary-core recipe::kitchen::archive
```

Expected: compile failures or HTTP-only rejection failures.

- [ ] **Step 3: Implement `RepoLocation`**

Implement:

```rust
pub enum RepoLocation {
    Http { base: String },
    File { root: PathBuf },
}

impl RepoLocation {
    pub fn parse(input: &str) -> Result<Self>;
    pub fn join_display(&self, relative: &str) -> Result<String>;
    pub async fn fetch_bytes(&self, relative: &str, limit: u64) -> Result<Vec<u8>>;
    pub async fn try_fetch_bytes(&self, relative: &str, limit: u64) -> Result<Option<Vec<u8>>>;
}
```

Rules:

- Strip trailing `/` for URL bases before appending paths.
- Treat `file://...` and bare paths as filesystem roots.
- Use `paths::validate_repo_relative_path` before filesystem joins.
- Enforce the byte limit for both HTTP and filesystem reads.
- For HTTP 404 in `try_fetch_bytes`, return `Ok(None)`.
- For filesystem not-found in `try_fetch_bytes`, return `Ok(None)`.

- [ ] **Step 4: Update existing download helpers without weakening native repo URL checks**

Keep `RepositoryClient::validate_url_scheme` HTTP-only for native distro metadata unless the call is explicitly static/local. Add a separate helper for package downloads:

```rust
pub async fn download_static_or_http_file(url_or_path: &str, dest_path: &Path) -> Result<()>;
```

Use it only from static-aware paths. In `repository/download.rs`, detect `file://` or bare local paths before constructing/calling `RepositoryClient`; route those URLs through `download_static_or_http_file` or a direct filesystem copy. Do not make native repo metadata parsers accept arbitrary local paths by accident.

After any static package download, validate both SHA-256 and file size before returning:

```rust
verify_checksum(&dest_path, &repo_pkg.checksum)?;
let expected_size = u64::try_from(repo_pkg.size)
    .map_err(|_| Error::DownloadError("negative package size in repository metadata".to_string()))?;
verify_file_size(&dest_path, expected_size)?;
```

Cleanup behavior must match checksum failures: remove the downloaded/copied file on hash or size mismatch.

Update `recipe/kitchen/archive.rs::download_file` to accept:

- `http://`
- `https://`
- `file://`
- bare local paths

Local paths copy bytes with `std::fs::copy`; HTTP keeps the existing curl retry behavior.

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary-core repository::static_repo::location
cargo test -p conary-core repository::download
cargo test -p conary-core recipe::kitchen::archive
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/repository/static_repo/location.rs crates/conary-core/src/repository/client.rs crates/conary-core/src/repository/download.rs crates/conary-core/src/recipe/kitchen/archive.rs
git commit -m "feat(static-repo): support local file fetches"
```

### Task 3: Harden TUF Client For Static Repos

**Files:**
- Modify: `crates/conary-core/src/trust/client.rs`
- Modify: `crates/conary-core/src/trust/verify.rs`
- Test: `crates/conary-core/src/trust/client.rs`
- Test: `crates/conary-core/src/trust/verify.rs`

- [ ] **Step 1: Write tests for the two Fable/Gemini gaps**

Add tests for:

```rust
#[test]
fn static_snapshot_consistency_requires_root_and_targets_entries() {
    let snapshot = SnapshotMetadata {
        type_field: "snapshot".to_string(),
        spec_version: TUF_SPEC_VERSION.to_string(),
        version: 1,
        expires: chrono::Utc::now() + chrono::Duration::days(1),
        meta: BTreeMap::new(),
    };
    let err = verify_static_snapshot_consistency(&snapshot, 1, 1).unwrap_err();
    assert!(err.to_string().contains("root.json"));
}
```

and an async `TufClient` file-repo test that:

1. bootstraps root v1
2. runs `update`
3. runs `update` again against identical timestamp bytes
4. expects the second update to succeed, not rollback

Add a second static update test where the offered timestamp version is greater than stored, but the fetched `snapshot.json` omits `meta["root.json"]` or `meta["targets.json"]`; the static update must fail before persistence. This guards the normal greater-version path, not only the equal timestamp no-change branch.

Add an expired-metadata error test asserting static repo expiry errors name the operator remedy: `conary publish --refresh`.

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary-core trust::verify
cargo test -p conary-core trust::client
```

Expected: missing helper and equal timestamp rollback failure.

- [ ] **Step 3: Add static snapshot presence helper**

Add:

```rust
pub fn verify_static_snapshot_consistency(
    snapshot: &SnapshotMetadata,
    expected_root_version: u64,
    expected_targets_version: u64,
) -> TrustResult<()> {
    let Some(root_meta) = snapshot.meta.get("root.json") else {
        return Err(TrustError::ConsistencyError(
            "Snapshot missing mandatory root.json reference".to_string(),
        ));
    };
    let Some(targets_meta) = snapshot.meta.get("targets.json") else {
        return Err(TrustError::ConsistencyError(
            "Snapshot missing mandatory targets.json reference".to_string(),
        ));
    };
    if root_meta.version != expected_root_version {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins root.json v{} but expected v{}",
            root_meta.version, expected_root_version
        )));
    }
    if targets_meta.version != expected_targets_version {
        return Err(TrustError::ConsistencyError(format!(
            "Snapshot pins targets.json v{} but expected v{}",
            targets_meta.version, expected_targets_version
        )));
    }
    Ok(())
}
```

Keep the existing generic helper if other TUF callers need permissive behavior.

- [ ] **Step 4: Add equal timestamp no-change support**

Make the stricter snapshot check an explicit TUF update mode carried by the client, for example:

```rust
pub enum TufUpdateMode {
    Generic,
    StaticRepo,
}

pub struct TufClient {
    repo_id: i64,
    tuf_base_url: String,
    update_mode: TufUpdateMode,
}
```

The general path keeps calling `verify_snapshot_consistency`; static repository update calls use `TufUpdateMode::StaticRepo` and call `verify_static_snapshot_consistency` exactly once before returning a verified snapshot. Do not unconditionally replace the generic helper unless the caller audit proves every TUF user requires static repo invariants.

In `TufUpdateMode::StaticRepo`, expiry errors for timestamp/snapshot/targets/root must include the operator remedy `conary publish --refresh` in the surfaced error text.

Extend `TufUpdateState` with:

```rust
stored_timestamp_hash: Option<String>,
```

Load `stored_timestamp_hash` from `tuf_metadata.metadata_hash` for the `timestamp` role. Extract the persistence hash calculation into a helper and use it from both equal-version comparison and `persist_metadata`:

```rust
fn metadata_hash_for_persistence<T: serde::Serialize + TufMetadataFields>(
    signed: &Signed<T>,
) -> TrustResult<String> {
    let json = serde_json::to_string(signed)?;
    Ok(hash::sha256(json.as_bytes()))
}
```

This intentionally matches the current `persist_metadata` serialization (`serde_json::to_string` of the full `Signed<T>` wrapper), not the canonical JSON of `signed.signed` used for signatures.

After verifying timestamp signatures and expiry:

```rust
if let Some(stored_v) = stored_timestamp_version {
    match signed_timestamp.signed.version.cmp(&stored_v) {
        std::cmp::Ordering::Greater => {}
        std::cmp::Ordering::Equal => {
            let offered_hash = metadata_hash_for_persistence(&signed_timestamp)?;
            if Some(offered_hash) != stored_timestamp_hash {
                return Err(TrustError::ConsistencyError(
                    "Timestamp version matches stored version but metadata bytes/hash differ"
                        .to_string(),
                ));
            }
            let signed_snapshot = stored_snapshot.ok_or_else(|| {
                TrustError::ConsistencyError("No stored snapshot found".to_string())
            })?;
            let signed_targets = stored_targets.ok_or_else(|| {
                TrustError::ConsistencyError("No stored targets found".to_string())
            })?;
            match self.update_mode {
                TufUpdateMode::StaticRepo => verify_static_snapshot_consistency(
                    &signed_snapshot.signed,
                    current_root.signed.version,
                    signed_targets.signed.version,
                )?,
                TufUpdateMode::Generic => verify_snapshot_consistency(
                    &signed_snapshot.signed,
                    current_root.signed.version,
                    Some(signed_targets.signed.version),
                )?,
            }
            return Ok(TufUpdateSnapshot {
                current_root,
                rotated_roots,
                signed_timestamp,
                signed_snapshot,
                signed_targets,
                snapshot_changed: false,
                targets_changed: false,
            });
        }
        std::cmp::Ordering::Less => {
            verify_version_increase(Role::Timestamp, signed_timestamp.signed.version, stored_v)?;
        }
    }
}
```

After snapshot and targets have been selected for the normal update path, replace the existing final consistency call with the same mode-aware helper. Static repos must run the strict check for every successful update, including greater-version timestamp/snapshot updates:

```rust
match self.update_mode {
    TufUpdateMode::StaticRepo => verify_static_snapshot_consistency(
        &signed_snapshot.signed,
        current_root.signed.version,
        signed_targets.signed.version,
    )?,
    TufUpdateMode::Generic => verify_snapshot_consistency(
        &signed_snapshot.signed,
        current_root.signed.version,
        Some(signed_targets.signed.version),
    )?,
}
```

Static sync and static `repo add` must construct/use the static mode; existing non-static TUF callers stay generic.

Use the same serialization/hash procedure as `persist_metadata` so hash comparisons match persisted values.

- [ ] **Step 5: Route TUF metadata fetches through `RepoLocation`**

Change `TufClient` internals to parse `tuf_base_url` into `RepoLocation`.

Static repos must be constructed with `tuf_root_url = Some("<repo-base>/metadata")`.
Keep `TufClient::new(repo_id, repo_url, None)` defaulting to `<repo>/tuf` for existing trust commands.

- [ ] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p conary-core trust::verify
cargo test -p conary-core trust::client
cargo test -p conary-core repository::static_repo::location
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/trust/client.rs crates/conary-core/src/trust/verify.rs
git commit -m "security(tuf): handle static repo no-change syncs"
```

### Task 4: Persist Static Package Signing Keys

**Files:**
- Create: `crates/conary-core/src/db/models/repository_package_key.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`
- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`

- [ ] **Step 1: Write migration/model tests**

Add tests proving:

- schema version increments to 72
- the table exists after fresh init
- active and retired keys persist
- `replace_for_repository` deletes absent keys and writes the new verified set

Use:

```rust
let (_tmp, conn) = crate::db::testing::create_test_db();
RepositoryPackageKey::replace_for_repository(
    &conn,
    1,
    &[
        RepositoryPackageKey {
            repository_id: 1,
            public_key: "base64-key".to_string(),
            key_id: Some("publish".to_string()),
            status: RepositoryPackageKeyStatus::Active,
            synced_at: None,
        },
    ],
)?;
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary-core db::schema
cargo test -p conary-core db::models::repository_package_key
```

Expected: schema/model compile failures.

- [ ] **Step 3: Add schema v72**

Add migration:

```sql
CREATE TABLE repository_package_keys (
    repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    public_key TEXT NOT NULL,
    key_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'retired')),
    synced_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (repository_id, public_key)
);
CREATE INDEX idx_repository_package_keys_repo
    ON repository_package_keys(repository_id);
```

Update `SCHEMA_VERSION` from 71 to 72 and schema tests that assert the current version.

- [ ] **Step 4: Add model API**

Use:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryPackageKeyStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPackageKey {
    pub repository_id: i64,
    pub public_key: String,
    pub key_id: Option<String>,
    pub status: RepositoryPackageKeyStatus,
    pub synced_at: Option<String>,
}

impl RepositoryPackageKey {
    pub fn replace_for_repository(conn: &Connection, repository_id: i64, keys: &[Self]) -> Result<()>;
    pub fn trusted_keys_for_repository(conn: &Connection, repository_id: i64) -> Result<Vec<String>>;
}
```

`trusted_keys_for_repository` returns both active and retired keys, sorted for deterministic tests.

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary-core db::schema
cargo test -p conary-core db::models::repository_package_key
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/db/schema.rs crates/conary-core/src/db/migrations/v41_current.rs crates/conary-core/src/db/models/mod.rs crates/conary-core/src/db/models/repository_package_key.rs
git commit -m "feat(static-repo): persist package signing keys"
```

### Task 5: Implement Verified Static Repo Sync

**Files:**
- Create: `crates/conary-core/src/repository/static_repo/sync.rs`
- Modify: `crates/conary-core/src/repository/sync.rs`
- Modify: `crates/conary-core/src/repository/sync/types.rs`
- Modify: `crates/conary-core/src/repository/sync/native.rs`
- Modify: `crates/conary-core/src/repository/mod.rs`

- [ ] **Step 1: Write sync tests**

Create tests that build a temp static repo with:

- `index.json`
- `keys/package-keys.json`
- valid TUF targets entries for both files and one `.ccs`

Assert:

- index is fetched only after TUF update
- static repo with `tuf_enabled = false` hard-fails sync before any native/JSON fetch
- static repo with no trusted root hard-fails sync with the `repo add --replace` re-pin command in the error
- `index_version != targets.version` fails
- missing targets entry for a package fails
- package target hash or length mismatches the index package entry fails
- package path traversal fails
- package keys with unknown `status` fail
- active and retired keys persist to `repository_package_keys`
- package rows land in `repository_packages`

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary-core repository::sync::static_repo
cargo test -p conary-core repository::static_repo::sync
```

Expected: missing static sync route.

- [ ] **Step 3: Route static repositories by strategy before generic sync**

Use `repo.default_strategy.as_deref() == Some("static")` as the routing marker. Static `repo add` will set it.

In both `sync_repository_from_db_path` and `sync_repository`, branch on `repo.default_strategy.as_deref() == Some("static")` before the existing `if repo.tuf_enabled` block and before any call to `fetch_repository_sync_snapshot`. Static repositories must never fall through to native/JSON sync.

Static sync preconditions:

- if `tuf_enabled == false`, hard error: `Static repository trust is not established; run conary repo add <name> <url-or-path> --fingerprint <root-key-id> --replace`
- if no trusted root exists, hard error with the same re-pin command shape
- after successful TUF verification, run the static sync path and return; do not call native/JSON fallback

Do not require or encourage operators to pass `--default-strategy static` manually. The existing `repo add --default-strategy` parser is for native package routing strategies; static repositories become static only after probing `conary-repo.toml` and establishing root trust.

- [ ] **Step 4: Implement verify-before-parse static sync**

The static sync flow:

1. Run TUF update in `TufUpdateMode::StaticRepo` and obtain `VerifiedTufState`.
2. Fetch `index.json` using `RepoLocation`.
3. Verify length/hash against `verified.targets["index.json"]` before parsing.
4. Parse `StaticIndex`.
5. Enforce `index_version == verified.targets_version`.
6. Fetch and verify `keys/package-keys.json` the same way.
7. Parse `PackageKeysFile` and persist all active + retired keys.
8. For every package entry, require a matching TUF target descriptor and enforce `entry.sha256 == target.hashes["sha256"]` plus `entry.size == target.length`.
9. Map packages into `RepositoryPackage` rows with `download_url = repo-base + validated path`.
10. Persist dependency strings in `dependencies` JSON and at least a self-provide for the package name/version.

The conversion function should have this shape:

```rust
pub(super) async fn fetch_static_sync_snapshot(
    repo: &Repository,
    verified: &VerifiedTufState,
) -> Result<RepositorySyncSnapshot>;
```

If `RepositorySyncSnapshot` is too narrow, extend it with:

```rust
StaticRows {
    packages: Vec<SyncedPackageRow>,
    package_keys: Vec<RepositoryPackageKey>,
}
```

and update persistence to write both package rows and package keys in one DB transaction.

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary-core repository::sync
cargo test -p conary-core repository::static_repo
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/repository/sync.rs crates/conary-core/src/repository/sync crates/conary-core/src/repository/static_repo crates/conary-core/src/repository/mod.rs
git commit -m "feat(static-repo): sync verified static indexes"
```

### Task 6: Implement Static `repo add` And `repo reset-trust`

**Files:**
- Create: `apps/conary/src/commands/repo_static.rs`
- Modify: `apps/conary/src/cli/repo.rs`
- Modify: `apps/conary/src/dispatch/repo.rs`
- Modify: `apps/conary/src/commands/repo.rs`
- Modify: `apps/conary/src/commands/mod.rs`

- [ ] **Step 1: Write CLI parse tests**

In `apps/conary/src/cli/mod.rs` tests, add:

```rust
#[test]
fn repo_add_rejects_fingerprint_with_gpg_flags_at_parse_time() {
    assert!(Cli::try_parse_from([
        "conary", "repo", "add", "acme", "file:///tmp/repo",
        "--fingerprint", "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        "--no-gpg-check",
    ]).is_err());
}

#[test]
fn repo_reset_trust_parses() {
    let cli = Cli::try_parse_from(["conary", "repo", "reset-trust", "acme"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Repo(RepoCommands::ResetTrust { .. }))
    ));
}

#[test]
fn repo_add_replace_parses_for_static_repin() {
    let cli = Cli::try_parse_from([
        "conary", "repo", "add", "acme", "file:///tmp/repo",
        "--fingerprint", "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        "--replace",
    ]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Repo(RepoCommands::Add { .. }))));
}
```

- [ ] **Step 2: Write command tests**

In `repo_static.rs`, test:

- fingerprint mismatch fails before insert
- single-key fingerprint exact-set match inserts repo with `tuf_enabled = true`
- multi-key root exact-set fingerprint match inserts repo with `tuf_enabled = true`
- fingerprint subset fails when root role has extra key IDs
- fingerprint superset fails when supplied set contains an unserved key ID
- duplicate fingerprints after lowercasing/normalization fail as ambiguous input
- inserted repo has `tuf_root_url = <base>/metadata`
- inserted repo has `default_strategy = Some("static")`
- static repo add rejects GPG flags after probing when no explicit fingerprint was passed but repo is static
- manual `repo add --default-strategy static` remains rejected for non-static/native repositories unless the implementation adds an explicit runtime guard
- non-interactive TOFU fails when no fingerprint is supplied
- `CONARY_NON_INTERACTIVE=1` is treated as non-interactive
- interactive TOFU prompt includes the stale-root replay caveat from `docs/specs/static-repo-format-v1.md` section 6.1
- `reset-trust` removes static trust material and synced package visibility for the repository
- duplicate-name static `repo add` without `--replace` fails before changing existing trust
- duplicate-name static `repo add --replace` updates the existing repository row and bootstraps the new root
- after `reset-trust`, `repo add <same-name> <same-or-new-url> --fingerprint <fp> --replace` re-establishes trust and re-enables sync

- [ ] **Step 3: Run failing tests**

Run:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary --lib commands::repo_static
```

Expected: missing fields/subcommand/helpers.

- [ ] **Step 4: Add CLI fields**

Extend `RepoCommands::Add`:

```rust
#[arg(long = "fingerprint", conflicts_with_all = ["gpg_key", "no_gpg_check", "gpg_strict"])]
fingerprints: Vec<String>,

#[arg(short = 'y', long)]
yes: bool,

#[arg(long)]
replace: bool,
```

Propagate `replace` through `RepoAddOptions` into `cmd_repo_add`; do not leave it as a CLI-only field.

Add:

```rust
#[command(name = "reset-trust")]
ResetTrust {
    name: String,
    #[command(flatten)]
    db: DbArgs,
}
```

- [ ] **Step 5: Implement static detection and trust establishment**

`cmd_repo_add` should probe `<base>/conary-repo.toml` first. If present:

1. Parse `RepoIdentity`.
2. Fetch `<base>/metadata/root.json`.
3. Parse and verify self-signed root with existing TUF helpers.
4. Compare `conary-repo.toml` root key IDs to the root role key IDs.
5. If `--fingerprint` is supplied, normalize/dedupe/lowercase all provided 64-hex values and require the supplied set to exactly equal the root-role key ID set. Subsets, supersets, duplicates after normalization, and mismatches hard-fail before persistence.
6. Without `--fingerprint`, require interactive stdin and explicit acceptance. If `CONARY_NON_INTERACTIVE=1` or stdin is not a TTY, fail.
   The prompt must include the old-root-replay warning from `docs/specs/static-repo-format-v1.md` section 6.1 so operators do not mistake TOFU for authenticated first contact.
7. Insert or replace repository state:
   - `url = normalized base`; for bare local paths, store an absolutized path so later syncs do not depend on the caller's working directory
   - `gpg_check = false`
   - `gpg_strict = false`
   - `gpg_key_url = None`
   - `default_strategy = Some("static")`
   - `tuf_enabled = true`
   - `tuf_root_url = Some("<base>/metadata")`
   - `enabled = true` unless the user explicitly passed `--disabled`
   - if a repository with the same name exists and `--replace` is not set, fail with the duplicate-name error before changing trust rows
   - if a repository with the same name exists and `--replace` is set, update that row in a transaction after deleting old TUF roots/keys/metadata/targets, package keys, and synced package rows for that repo
8. Bootstrap TUF with the fetched root bytes and configure subsequent updates to use `TufUpdateMode::StaticRepo`.

`cmd_repo_reset_trust` should remove TUF roots/keys/metadata/targets, package keys, and synced package rows for the repo in one transaction. It should set `tuf_enabled = false`, clear `tuf_root_version`, disable the repository row, and print the explicit recovery command shape: `conary repo add <name> <url-or-path> --fingerprint <new-root-key-id> --replace`.

- [ ] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary --lib commands::repo
cargo test -p conary --lib commands::repo_static
cargo fmt --check
```

Commit:

```bash
git add apps/conary/src/cli/repo.rs apps/conary/src/dispatch/repo.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/repo.rs apps/conary/src/commands/repo_static.rs apps/conary/src/cli/mod.rs
git commit -m "feat(repo): add static trust establishment"
```

### Task 7: Verify Static CCS Package Signatures During Install

**Files:**
- Modify: `apps/conary/src/commands/install/options.rs`
- Modify: `apps/conary/src/commands/install/resolve.rs`
- Modify: `apps/conary/src/commands/install/acquire.rs`
- Modify: `apps/conary/src/commands/install/conversion.rs`
- Modify: `crates/conary-core/src/repository/resolution.rs`

**Ownership boundary:** Preserve `resolution.rs` as the package source resolver. Task 7 may add static-source provenance tagging to the existing repository package selection output, but static package signature enforcement belongs in the install acquisition/conversion boundary and must not reshape legacy/native resolution behavior.

- [ ] **Step 1: Write install verification tests**

Add tests proving:

- static repo `.ccs` install fails when unsigned
- static repo `.ccs` install fails when signed by an unlisted key
- static repo `.ccs` install succeeds when signed by a TUF-verified active key
- static repo `.ccs` install fails when signed only by a retired key

Use `SigningKeyPair`, `write_signed_ccs_package`, and `RepositoryPackageKey::replace_for_repository` to avoid network in unit tests.

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary --lib install::conversion
cargo test -p conary --lib install::acquire
```

Expected: static packages install without repo package-key policy.

- [ ] **Step 3: Carry static repository provenance into install**

Extend both core resolution provenance and install provenance:

```rust
pub enum RepositorySourceKind {
    Native,
    Remi,
    Static,
}

pub struct RepositorySourceMetadata {
    pub repository_id: i64,
    pub source_distro: Option<String>,
    pub version_scheme: Option<String>,
    pub source_kind: RepositorySourceKind,
}

pub(crate) struct RepositoryInstallProvenance {
    pub repository_id: i64,
    pub source_distro: Option<String>,
    pub version_scheme: Option<String>,
    pub source_kind: RepositorySourceKind,
}
```

When resolution selects a package from a repo whose `default_strategy == Some("static")`, set `RepositorySourceMetadata.source_kind = Static` in `repository_source_metadata()`. Update `install_provenance_from_resolved()` and `repository_install_provenance_from_package()` to copy the source kind into `RepositoryInstallProvenance`.

- [ ] **Step 4: Enforce CCS trust for static sources**

Before `CcsPackage::parse(ccs_path)` in the `.ccs` install path:

```rust
if provenance.source_kind == RepositorySourceKind::Static {
    let keys = RepositoryPackageKey::trusted_keys_for_repository(&conn, provenance.repository_id)?;
    let policy = TrustPolicy::strict(keys);
    let result = conary_core::ccs::verify::verify_package(Path::new(ccs_path), &policy)
        .context("Static repository package signature verification failed")?;
    if !result.valid {
        anyhow::bail!("Static repository package signature verification failed");
    }
}
```

Do not allow `--allow-unsigned` to bypass static repo installs; that flag remains local `conary ccs install` behavior.

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary --lib install::conversion
cargo test -p conary --lib install::acquire
cargo test -p conary-core repository::resolution
cargo test -p conary-core ccs::verify
cargo fmt --check
```

Commit:

```bash
git add apps/conary/src/commands/install crates/conary-core/src/repository/resolution.rs
git commit -m "security(static-repo): verify CCS package keys on install"
```

### Task 8: Add Recipe Local-Source Variant

**Files:**
- Modify: `crates/conary-core/src/recipe/format.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/archive.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/cook.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/config.rs`
- Modify: `apps/conary/src/commands/cook.rs`

- [ ] **Step 1: Write local-source recipe tests**

Add parser tests proving:

- current remote archive recipes still parse unchanged
- `[source] path = "."` parses as a local workspace source
- `[source] path = "./src"` resolves relative to the recipe file directory
- `[source]` with both `archive` and `path` is rejected
- `[source] path = "../outside"` is rejected unless the implementation deliberately documents and tests an outside-workspace allowance

Add Kitchen/cook tests proving:

- local path source builds from the local workspace instead of downloading an archive
- isolated local-source builds mount or copy the workspace into the build root
- remote archive source still uses archive download/cache behavior

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary-core recipe::format
cargo test -p conary-core recipe::kitchen
cargo test -p conary --lib commands::cook
```

Expected: local-source parse/build support missing.

- [ ] **Step 3: Implement source enum with backward compatibility**

Keep the existing wire shape for archive recipes, but parse source as an enum:

```rust
pub enum SourceSection {
    Remote(RemoteSourceSection),
    Local(LocalSourceSection),
}

pub struct RemoteSourceSection {
    pub archive: String,
    pub checksum: String,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub additional: Vec<AdditionalSource>,
}

pub struct LocalSourceSection {
    pub path: PathBuf,
}
```

If serde's untagged enum produces poor diagnostics, keep a custom `Deserialize` impl that rejects ambiguous source sections with a clear message.

- [ ] **Step 4: Route Kitchen source preparation**

For local sources:

1. resolve relative paths against the recipe file's directory
2. reject missing paths before entering build execution
3. for host builds, use the resolved workspace path as the source root
4. for isolated builds, bind-mount or copy the workspace into the build root according to existing Kitchen isolation primitives
5. do not claim hermetic filtering in M1a; tracked-file-only hermetic input selection remains M2

For remote sources, preserve the existing archive download/checksum/cache behavior, including the Task 2 `file://` and bare-path archive support.

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary-core recipe::format
cargo test -p conary-core recipe::kitchen
cargo test -p conary --lib commands::cook
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/recipe/format.rs crates/conary-core/src/recipe/kitchen apps/conary/src/commands/cook.rs
git commit -m "feat(recipe): add local source workspaces"
```

### Task 9: Align `conary cook` With M1a Semantics

**Files:**
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/commands/cook.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/config.rs`
- Modify: `crates/conary-core/src/recipe/kitchen/provenance_capture.rs`
- Modify: `crates/conary-core/src/ccs/manifest.rs`

- [ ] **Step 1: Write CLI and provenance tests**

Add tests:

```rust
#[test]
fn cook_accepts_optional_target_and_recipe_flag() {
    assert!(Cli::try_parse_from(["conary", "cook"]).is_ok());
    assert!(Cli::try_parse_from(["conary", "cook", "--recipe", "recipe.toml"]).is_ok());
    assert!(Cli::try_parse_from(["conary", "cook", "recipe.toml", "--isolated"]).is_ok());
}
```

Add provenance tests:

```rust
#[test]
fn manifest_provenance_serializes_m1a_origin_and_hardening() {
    let provenance = ManifestProvenance {
        origin_class: Some("native-built".to_string()),
        hardening_level: Some("sandboxed".to_string()),
        ..Default::default()
    };
    let toml = toml::to_string(&provenance).unwrap();
    assert!(toml.contains("origin_class"));
    assert!(toml.contains("hardening_level"));
}
```

Add runtime compatibility tests proving:

- `conary cook --hermetic <recipe-path>` parses the hidden flag but fails before build execution with an error containing `M2`, and does not write a package.
- `conary cook --no-isolation <recipe-path>` parses as a hidden compatibility alias/no-op for the M1a host default.

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary-core ccs::manifest
cargo test -p conary-core recipe::kitchen::provenance_capture
```

Expected: CLI/provenance failures.

- [ ] **Step 3: Update `cook` CLI shape**

Change root `Cook` to:

```rust
Cook {
    target: Option<String>,
    #[arg(long)]
    recipe: Option<String>,
    #[arg(short, long, default_value = "./dist")]
    output: String,
    #[arg(long, default_value = "/var/cache/conary/sources")]
    source_cache: String,
    #[arg(short, long)]
    jobs: Option<u32>,
    #[arg(long)]
    keep_builddir: bool,
    #[arg(long)]
    validate_only: bool,
    #[arg(long)]
    fetch_only: bool,
    #[arg(long)]
    isolated: bool,
    #[arg(long, hide = true)]
    no_isolation: bool,
    #[arg(long, hide = true)]
    hermetic: bool,
}
```

Resolution:

- `--recipe` wins when provided.
- positional target may be a recipe path or directory containing `recipe.toml`.
- no target means current directory, requiring `recipe.toml`.
- bare source inference remains an error in M1a with a message naming M1b.

- [ ] **Step 4: Add provenance fields**

Add to `ManifestProvenance` with serde defaults:

```rust
#[serde(default)]
pub origin_class: Option<String>,
#[serde(default)]
pub hardening_level: Option<String>,
```

`cmd_cook` sets:

- host default: `origin_class = "native-built"`, `hardening_level = "host"`
- `--isolated`: `origin_class = "native-built"`, `hardening_level = "sandboxed"`

Keep `--hermetic` hidden and make it fail before build execution if explicitly used. This is an intentional M1a behavior removal from the current visible/functional flag, because M1a must not claim hermetic builds:

```rust
if hermetic {
    anyhow::bail!("Hermetic cook/publish is an M2 feature; M1a supports host or --isolated builds only");
}
```

Keep `--no-isolation` hidden for compatibility and treat it as a no-op because host builds are now the default. Do not let it override `--isolated`; if both are passed, fail with a clear conflict.

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary-core ccs::manifest
cargo test -p conary-core recipe::kitchen
cargo fmt --check
```

Commit:

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/dispatch/root.rs apps/conary/src/commands/cook.rs crates/conary-core/src/recipe/kitchen crates/conary-core/src/ccs/manifest.rs
git commit -m "feat(cook): align recipe builds with M1a"
```

### Task 10: Harden Key File Writes And Add Static Key Directory Helpers

**Files:**
- Modify: `crates/conary-core/src/ccs/signing.rs`
- Create or extend: `crates/conary-core/src/repository/static_repo/publish.rs`

- [ ] **Step 1: Write permission tests**

On Unix, assert:

```rust
#[test]
#[cfg(unix)]
fn save_to_files_creates_private_key_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let private_path = dir.path().join("key.private");
    let public_path = dir.path().join("key.public");
    SigningKeyPair::generate()
        .save_to_files(&private_path, &public_path)
        .unwrap();
    let mode = std::fs::metadata(&private_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
```

Add a static key-dir helper test asserting `~/.config/conary/keys/<repo-name>` equivalent paths are created 0700.

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary-core ccs::signing
cargo test -p conary-core repository::static_repo::publish
```

Expected: key-dir helper missing; existing chmod-after-write may still pass mode but not creation behavior.

- [ ] **Step 3: Replace private key files via 0600 temp file and atomic rename**

Change `save_to_files` to write the private key through a temporary file in the same directory, opened with 0600 on Unix, then atomically rename it over the destination. This preserves secure creation while allowing publish-key rotation to replace an existing `publish.private` without partial writes.

```rust
let tmp_private_path = private_path.with_extension(format!(
    "private.tmp.{}",
    std::process::id()
));
let mut options = std::fs::OpenOptions::new();
options.write(true).create_new(true);
#[cfg(unix)]
{
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}
let mut file = options.open(&tmp_private_path)?;
use std::io::Write;
file.write_all(private_toml.as_bytes())?;
file.sync_all()?;
drop(file);
std::fs::rename(&tmp_private_path, private_path)?;
```

Ensure parent dirs are created before the write. For publish key dirs, create the directory mode 0700 before key writes.

- [ ] **Step 4: Run tests and commit**

Run:

```bash
cargo test -p conary-core ccs::signing
cargo test -p conary-core repository::static_repo::publish
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/ccs/signing.rs crates/conary-core/src/repository/static_repo/publish.rs
git commit -m "security(keys): create private keys with restrictive modes"
```

### Task 11: Implement Static Publisher Core

**Files:**
- Modify: `crates/conary-core/src/repository/static_repo/publish.rs`
- Modify: `crates/conary-core/src/trust/ceremony.rs`
- Modify: `crates/conary-core/src/trust/generate.rs` only if helper signatures need target extras

- [ ] **Step 1: Write publisher tests**

Cover:

- initial publish creates `conary-repo.toml`, `metadata/1.root.json`, `metadata/root.json`, `targets.json`, `snapshot.json`, `timestamp.json`, `index.json`, `keys/package-keys.json`, and `packages/<name>/...ccs`
- first publish output includes the root fingerprint, publish key ID, and the root-key-is-identity/store-offline warning from `docs/specs/static-repo-format-v1.md` section 7.1
- package overwrite with different bytes fails
- `index_version == targets.version`
- `keys/package-keys.json` includes active publish key
- `--refresh` with no near-expiry root/targets/snapshot content changes still bumps timestamp
- `--refresh` selects roles by the static spec's 25% lifetime window and expands the minimal closed cascade
- targets refresh cascades to snapshot and timestamp and updates index only in `index_version`/`generated`
- root refresh cascades to snapshot and timestamp
- state-file watermark rejects destination version regression
- `--force-reinit` allows destination present but unverifiable, with a new identity warning
- publish-key rotation updates targets/snapshot/timestamp root roles in one root version and marks old package key retired
- publish command builds with isolation enabled and `allow_network = true`, and still stamps `hardening_level = "sandboxed"` rather than `hermetic`

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary-core repository::static_repo::publish
cargo test -p conary-core trust::ceremony
```

Expected: publisher missing.

- [ ] **Step 3: Implement publish input/output structs**

Use:

```rust
pub struct StaticPublishOptions {
    pub repo_name: String,
    pub repo_description: Option<String>,
    pub destination: RepoLocation,
    pub key_dir: PathBuf,
    pub state_file: PathBuf,
    pub package_paths: Vec<PathBuf>,
    pub refresh: bool,
    pub force_reinit: bool,
    pub accept_destination_state: bool,
    pub rotate_publish_key: bool,
    pub rotate_root_key: bool,
}

pub struct StaticPublishOutcome {
    pub root_version: u64,
    pub targets_version: u64,
    pub snapshot_version: u64,
    pub timestamp_version: u64,
    pub root_key_ids: Vec<String>,
    pub publish_key_id: String,
    pub package_count: usize,
    pub preview_warning: String,
}
```

Only local filesystem destinations are required in the first core implementation. Keep rsync/S3 target parsing out of M1a until there is a tested uploader abstraction.

- [ ] **Step 4: Implement initial and incremental publish**

Follow `docs/specs/static-repo-format-v1.md` exactly:

- two-key ceremony: root key signs root; publish key signs targets/snapshot/timestamp and CCS packages
- root generated by `create_initial_root(root_key, publish_key, publish_key, publish_key, 365)`
- package artifacts immutable once published
- `index.json` and `keys/package-keys.json` are TUF targets
- `--refresh` is content-preserving: compute the role set from expiry windows, then apply the closed cascade from `docs/specs/static-repo-format-v1.md` section 5.5
- `generate_timestamp` receives hours, not days
- upload/write order is packages+keys+roots+identity, index+targets, snapshot, timestamp
- re-fetch timestamp before final write for local destination race detection
- conditional writes reject mismatched existing mutable files unless they match the expected previous version or `--force-reinit` applies
- `accept_destination_state` is the loud watermark override from `docs/specs/static-repo-format-v1.md` section 5.1.4; require an explicit CLI flag and warning before allowing it

- [ ] **Step 5: Add batch publish-key rotation helper**

Add a helper in `trust/ceremony.rs`:

```rust
pub fn rotate_publish_key(
    current_root: &Signed<RootMetadata>,
    old_publish_key: &SigningKeyPair,
    new_publish_key: &SigningKeyPair,
    root_key: &SigningKeyPair,
    expires_days: i64,
) -> TrustResult<Signed<RootMetadata>>;
```

It must replace the old publish key for targets, snapshot, and timestamp together in one root version.

- [ ] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p conary-core repository::static_repo::publish
cargo test -p conary-core trust::ceremony
cargo fmt --check
```

Commit:

```bash
git add crates/conary-core/src/repository/static_repo/publish.rs crates/conary-core/src/trust/ceremony.rs crates/conary-core/src/trust/generate.rs
git commit -m "feat(static-repo): publish file-based repositories"
```

### Task 12: Add Top-Level `conary publish`

**Files:**
- Create: `apps/conary/src/commands/publish.rs`
- Modify: `apps/conary/src/cli/mod.rs`
- Modify: `apps/conary/src/dispatch/root.rs`
- Modify: `apps/conary/src/commands/mod.rs`

- [ ] **Step 1: Write CLI tests**

Add tests:

```rust
#[test]
fn publish_project_form_parses() {
    let cli = Cli::try_parse_from(["conary", "publish", "./repo", "--recipe", "recipe.toml"]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Publish { .. })));
}

#[test]
fn publish_artifact_form_is_rejected_in_m1a() {
    let cli = Cli::try_parse_from(["conary", "publish", "dist/pkg.ccs", "./repo"]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Publish { .. })));
}
```

Runtime test should assert two-position artifact form fails with this M2 message: `artifact-form publish requires M2 attestation support; run project-form publish from a recipe project`.

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary --lib commands::publish
```

Expected: command missing.

- [ ] **Step 3: Add CLI shape**

Add:

```rust
Publish {
    what: String,
    target: Option<String>,
    #[arg(long)]
    recipe: Option<String>,
    #[arg(long)]
    key_dir: Option<String>,
    #[arg(long)]
    state_file: Option<String>,
    #[arg(long)]
    refresh: bool,
    #[arg(long)]
    force_reinit: bool,
    #[arg(long)]
    accept_destination_state: bool,
    #[arg(long)]
    rotate_publish_key: bool,
    #[arg(long)]
    rotate_root_key: bool,
    #[arg(short = 'y', long)]
    yes: bool,
}
```

M1a supports project form only. Runtime disambiguation:

- one positional (`what`, `target = None`) means project-form publish to destination `what`
- two positionals (`what`, `target = Some(_)`) means artifact-form publish and must fail with: `artifact-form publish requires M2 attestation support; run project-form publish from a recipe project`

- [ ] **Step 4: Implement command flow**

`cmd_publish`:

1. Resolve destination from the one-position project form and recipe path with the same helper as `cmd_cook`.
2. Build in Kitchen isolation regardless of normal cook default, with `KitchenConfig.allow_network = true` for M1a publish builds. This is sandboxed, not hermetic/offline.
3. Stamp provenance as `origin_class = "native-built"`, `hardening_level = "sandboxed"`.
4. Print that M1a static repos are preview repos, not reproducible release evidence.
5. Sign the package with publish key.
6. Call `StaticPublisher`.
7. On first publish, print root fingerprint, publish key ID, and the root-key-is-identity/store-offline warning from `docs/specs/static-repo-format-v1.md` section 7.1.

- [ ] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p conary --lib cli::tests
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish
cargo fmt --check
```

Commit:

```bash
git add apps/conary/src/cli/mod.rs apps/conary/src/dispatch/root.rs apps/conary/src/commands/mod.rs apps/conary/src/commands/publish.rs
git commit -m "feat(publish): add static repo project publish"
```

### Task 13: End-To-End Static Repo CLI Tests

**Files:**
- Create: `apps/conary/tests/static_repo_m1a.rs`

- [ ] **Step 1: Write local filesystem E2E**

Test flow:

```rust
#[test]
fn m1a_publish_add_sync_install_from_local_static_repo() {
    let work = tempfile::tempdir().unwrap();
    let repo_dir = work.path().join("repo");
    let project_dir = work.path().join("project");
    let root = work.path().join("root");
    let db_path = work.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    // Create a tiny recipe project named test-hello with [source] path = ".".
    // Put the source file directly in the project workspace.
    // Run: conary publish <repo_dir> --recipe <recipe> --key-dir <keys> --state-file <state> --yes
    // Parse printed root fingerprint.
    // Run: conary repo add local-static <repo_dir> --fingerprint <fp> --db-path <db>
    // Run: conary repo sync local-static --db-path <db> --force
    // Run: conary install test-hello --repo local-static --db-path <db> --root <root> --sandbox never --yes
    // Assert installed file exists under <root>.
}
```

Do not reuse `apps/conary/tests/fixtures/recipes/simple-hello/recipe.toml`: it depends on the Remi test HTTP fixture and its package name is `test-hello`, which makes it a poor M1a static-repo E2E seed. Generate the recipe and local workspace source inside the test tempdir instead.

- [ ] **Step 2: Add failure E2Es**

Add tests for:

- no-change `repo sync --force` twice succeeds
- tampered `index.json` fails sync
- unsigned static package fails install
- root fingerprint mismatch fails add
- non-interactive add without fingerprint fails

- [ ] **Step 3: Run E2E tests**

Run:

```bash
cargo test -p conary --test static_repo_m1a -- --nocapture
```

Expected: pass after previous tasks are complete.

- [ ] **Step 4: Commit**

```bash
git add apps/conary/tests/static_repo_m1a.rs
git commit -m "test(static-repo): cover M1a publish install loop"
```

### Task 14: Update Docs And Assistant Routing

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/specs/static-repo-format-v1.md` only if implementation discovers a spec erratum

- [ ] **Step 1: Check coherency ownership**

Run:

```bash
rg -n "static-repo|publish|repo add|cook|trust/client|commands/publish|repository/static_repo" docs/superpowers/feature-coherency-ledger.tsv
```

If rows point at touched docs/source claims, run the corresponding coherency checks named in the ledger.

- [ ] **Step 2: Update routing docs**

Add a feature ownership card for "Packaging And Static Repository Publishing" that points to:

- `docs/specs/static-repo-format-v1.md`
- `docs/superpowers/specs/2026-06-10-packaging-toolchain-design.md`
- `apps/conary/src/commands/publish.rs`
- `apps/conary/src/commands/cook.rs`
- `apps/conary/src/commands/repo_static.rs`
- `crates/conary-core/src/repository/static_repo/`
- `crates/conary-core/src/trust/`
- `crates/conary-core/src/ccs/signing.rs`

Focused proof:

```bash
cargo test -p conary-core repository::static_repo
cargo test -p conary-core trust::client
cargo test -p conary-core trust::verify
cargo test -p conary --test static_repo_m1a
```

Interaction gate:

```bash
cargo test -p conary-core
cargo test -p conary
cargo run -p conary-test -- list
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 3: Run docs checks and commit**

Run:

```bash
bash scripts/check-doc-truth.sh
cargo fmt --check
```

Commit:

```bash
git add docs/ARCHITECTURE.md docs/llms/subsystem-map.md docs/modules/feature-ownership.md docs/specs/static-repo-format-v1.md
git commit -m "docs: route static repo implementation ownership"
```

### Task 15: Final Verification Gate

**Files:**
- Verify only.

- [ ] **Step 1: Run targeted suites**

```bash
cargo test -p conary-core repository::static_repo
cargo test -p conary-core trust::client
cargo test -p conary-core trust::verify
cargo test -p conary-core ccs::signing
cargo test -p conary-core ccs::verify
cargo test -p conary --lib cli::tests
cargo test -p conary --lib commands::repo
cargo test -p conary --lib commands::publish
cargo test -p conary --test static_repo_m1a -- --nocapture
```

- [ ] **Step 2: Run package-level suites**

```bash
cargo test -p conary-core
cargo test -p conary
```

- [ ] **Step 3: Run workspace quality gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p conary-test -- list
```

- [ ] **Step 4: Run docs and audit gates**

```bash
diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv <(bash scripts/docs-audit-inventory.sh)
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-doc-truth.sh
```

- [ ] **Step 5: Final commit if needed**

If final verification changes docs/test fixtures:

```bash
git add <changed-files>
git commit -m "test(static-repo): finish M1a verification"
```

---

## Implementation Notes

- Do not add schema migrations except the package-key persistence migration in Task 4.
- Do not make native distro repository metadata parsers accept file paths. File/local support is for static repos, Kitchen source fetches, and recipe local-source workspaces only.
- Do not accept GPG flags for static repos. Parse-time conflicts cover explicit `--fingerprint` plus GPG flags; execution-time rejection covers probed static repos with GPG flags.
- Do not parse `index.json` or `keys/package-keys.json` until TUF target hash/length verification succeeds.
- Do not let `--allow-unsigned` bypass static repo package signatures.
- Do not claim `hermetic` or emit build attestations in M1a output/provenance.
- Keep `repository/sync.rs` as an orchestrator; put new static-specific code in child modules.
- Keep `apps/conary/src/cli/mod.rs` edits narrow despite its current size; do not decompose it as part of M1a unless a focused parse-test-only child file already exists.

## Self-Review Checklist

- [ ] Every M1a milestone item has a task: local-source recipe support, cook, publish, repo add, sync, install.
- [ ] Static spec section 1 file/local support is covered by Tasks 2, 3, 5, 6, 7, 8, 11, 12, and 13.
- [ ] Static spec section 2 repo identity and fingerprint trust is covered by Task 6.
- [ ] Static spec section 3 index verification/path normalization is covered by Tasks 1 and 5.
- [ ] Static spec section 4 TUF metadata/package keys is covered by Tasks 3, 4, 5, 7, and 11.
- [ ] Static spec section 5 publish algorithm/refresh/watermark/key ceremony is covered by Tasks 10, 11, and 12.
- [ ] Static spec section 6 client behavior/reset-trust/failure semantics is covered by Tasks 3, 5, 6, 7, and 13.
- [ ] Static spec section 7 key lifecycle is covered by Tasks 10 and 11.
- [ ] Parent spec local-source/provenance honesty is covered by Tasks 8, 9, and 12.
- [ ] No task depends on a later task's code without naming the dependency.
- [ ] Every task has a verification command and a commit boundary.

## Handoff

Plan execution should start with Task 1 and proceed in order. The highest-risk review checkpoints are after Task 3 (TUF semantics), Task 7 (static package signature enforcement), Task 11 (publisher ordering/watermark), and Task 13 (end-to-end loop).
