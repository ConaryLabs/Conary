# M4c Remi Native CCS Publication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status:** Locked implementation plan for M4c.

**Goal:** Implement Remi as a first-class native CCS v2 publication service so a release-eligible native package can be uploaded, verified, indexed, fetched, and installed without synthetic conversion metadata.

**Architecture:** Add a dedicated native publication storage and lookup layer under Remi, while keeping the existing admin route as the first intake surface. Reuse the M2 static publish gate, add a narrow verified-artifact helper so Remi can inspect v2 authority without legacy reparsing, project native rows into repository metadata for public discovery, and extend client/resolver identity with `package_release`. Keep conversion behavior intact and make native rows explicit in metadata, sparse index, search, download, TUF, chunk reachability, and garbage collection.

**Tech Stack:** Rust 2024, Axum, Tokio, rusqlite/SQLite migrations, serde/serde_json, Tantivy, existing CCS v2 schema/writer/verifier, existing static publish gate, existing Remi TUF metadata helpers, Cargo test.

---

## Design Inputs

Read these before executing:

- `AGENTS.md`
- `docs/superpowers/specs/2026-06-18-m4c-remi-native-ccs-publication-design.md`
- `docs/superpowers/specs/2026-06-17-m4-ccs-native-ecosystem-design.md`
- `docs/superpowers/specs/2026-06-17-m4a-ccs-v2-native-package-contract-design.md`
- `docs/superpowers/specs/2026-06-18-m4b-native-authoring-build-lint-test-design.md`
- `docs/superpowers/plans/2026-06-05-ccs-native-ecosystem-roadmap.md`
- `docs/modules/remi.md`
- `docs/modules/feature-ownership.md`
- `docs/modules/test-fixtures.md`
- `docs/llms/subsystem-map.md`
- `apps/remi/src/server/release_publish.rs`
- `apps/remi/src/server/handlers/packages.rs`
- `apps/remi/src/server/handlers/index.rs`
- `apps/remi/src/server/handlers/sparse.rs`
- `apps/remi/src/server/handlers/chunks.rs`
- `apps/remi/src/server/search.rs`
- `apps/remi/src/server/chunk_gc.rs`
- `crates/conary-core/src/ccs/package.rs`
- `crates/conary-core/src/ccs/v2/schema.rs`
- `crates/conary-core/src/ccs/builder/package_writer.rs`
- `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- `crates/conary-core/src/repository/remi.rs`
- `crates/conary-core/src/repository/sync/remi.rs`
- `crates/conary-core/src/resolver/identity.rs`
- `apps/conary/src/commands/remi_publish.rs`

## Scope Locks

M4c includes:

- A `native_package_publications` table and model.
- `repository_packages.package_release`.
- Non-null normalized architecture keys for native publication rows, using `noarch` when v2 authority omits architecture.
- Full-key native identity: `distro + name + version + package_release + architecture`.
- A Remi-owned `native_publish/` module boundary.
- Existing `POST /v1/admin/releases/{distro}` intake routed through native publication internals.
- M2 static publish-gate reuse for all accepted native uploads.
- Server-side v2 inspection through verified v2 authority, not legacy `CcsPackage::parse`.
- No synthetic `converted_packages` row for native publications.
- Atomic replacement that supersedes previous native rows and removes/deactivates superseded TUF target rows.
- Native lookup before conversion fallback for package metadata and download.
- Native rows in metadata, sparse index, search, chunk reachability, and garbage collection.
- Client and resolver release support for native rows.
- A local Remi publish, fetch, download, and install proof.
- Docs and docs-audit updates after implementation passes.

M4c excludes:

- Developer key registration, rotation, or revocation.
- Cloud R2 as a required local proof.
- New public distro targets beyond Fedora 44, Ubuntu 26.04, and Arch.
- M4d target-profile facts.
- A Remi-only install path.
- Rewriting federation, OCI, conversion benchmark, or conversion persistence behavior.
- Treating conversion scriptlet publication gates as native publication truth.

## File Map

Create:

- `crates/conary-core/src/db/models/native_publication.rs` - native publication row model, status enum, normalized architecture helper, active lookup helpers.
- `apps/remi/src/server/native_publish/mod.rs` - native publication module hub and public exports.
- `apps/remi/src/server/native_publish/types.rs` - request/response DTOs, error codes, and verified native artifact DTO.
- `apps/remi/src/server/native_publish/verify.rs` - Remi-native verification wrapper over the shared publish gate.
- `apps/remi/src/server/native_publish/storage.rs` - safe native filenames, target paths, package/CAS copy, cleanup helpers.
- `apps/remi/src/server/native_publish/persistence.rs` - DB transaction for native publication, repository projection, TUF target updates, and supersede.
- `apps/remi/src/server/native_publish/public_lookup.rs` - metadata/download/index/search lookup helpers for active native rows.
- `apps/remi/src/server/native_publish/test_support.rs` - release-eligible v2 fixture generation and Remi test database helpers.
- `apps/conary/tests/packaging_m4c.rs` - client proof for native Remi publish, fetch, download, and install.

Modify:

- `crates/conary-core/src/db/schema.rs` - bump `SCHEMA_VERSION` to 74 and route migration 74.
- `crates/conary-core/src/db/migrations/v41_current.rs` - add `migrate_v74`.
- `crates/conary-core/src/db/models/mod.rs` - export `NativePackagePublication`.
- `crates/conary-core/src/db/models/repository.rs` - add `package_release` to `RepositoryPackage`, `COLUMNS`, inserts, row parsing, and tests.
- `crates/conary-core/src/repository/download.rs` and `crates/conary-core/src/repository/resolution.rs` - update direct `RepositoryPackage` literals to include `package_release`.
- `crates/conary-core/src/resolver/identity.rs` - carry `package_release` on resolver candidates.
- `crates/conary-core/src/repository/remi.rs` - add optional `release` query support to package/download URLs.
- `crates/conary-core/src/repository/sync/remi.rs` - preserve native release metadata when syncing Remi rows.
- `crates/conary-core/src/repository/sync/types.rs` - deserialize Remi metadata `release`.
- `crates/conary-core/src/resolver/provider/mod.rs` - update direct `PackageIdentity` literals to include `package_release`.
- `crates/conary-core/src/repository/static_repo/publish_gate.rs` - add a verified static artifact helper that returns the parsed package plus lint report.
- `apps/conary/src/commands/remi_publish.rs` - update preflight so v2 packages are structurally accepted before server verification.
- `apps/remi/src/server/mod.rs` - export `native_publish`.
- `apps/remi/src/server/release_publish.rs` - delegate release upload verification, inspection, promotion, and commit to `native_publish`.
- `apps/remi/src/server/prewarm.rs`, `apps/remi/src/server/federated_index.rs`, and `apps/remi/src/server/conversion/lookup.rs` - update direct `RepositoryPackage` literals to include `package_release`.
- `apps/remi/src/server/handlers/packages.rs` - consult native rows before conversion fallback; support `release` query.
- `apps/remi/src/server/handlers/index.rs` - merge native rows into metadata with `native`/`source_kind` and `converted=false`.
- `apps/remi/src/server/handlers/sparse.rs` - include native release rows.
- `apps/remi/src/server/search.rs` - preserve native `version + release + arch` document identity and response projection.
- `apps/remi/src/server/chunk_gc.rs` - treat active native chunks as referenced.
- `apps/remi/src/server/handlers/chunks.rs` and `apps/remi/src/server/handlers/oci.rs` - include native reachability only where public chunk serving depends on publication state.
- `apps/remi/src/server/handlers/openapi.rs` - document native release upload responses and error codes.
- `docs/modules/remi.md`, `docs/modules/test-fixtures.md`, `docs/modules/feature-ownership.md`, `docs/llms/subsystem-map.md` - update after implementation passes.
- `docs/superpowers/feature-coherency-ledger.tsv` - update rows for changed public claims/routes when applicable.
- `docs/superpowers/documentation-accuracy-audit-inventory.tsv` and `docs/superpowers/documentation-accuracy-audit-ledger.tsv` - update when the plan is locked and after implementation docs change.

Maintainability boundaries:

- Before editing `apps/remi/src/server/release_publish.rs`, state that the preserved boundary is route/staging compatibility while native publication behavior moves to `native_publish/`.
- Before editing `crates/conary-core/src/db/migrations/v41_current.rs`, state that the preserved boundary is migration sequencing only; migration 74 must not delete existing repository rows to force uniqueness.
- Before editing `crates/conary-core/src/repository/static_repo/publish_gate.rs`, state that the preserved boundary is shared static publish policy; M4c adds a return-value helper, not a Remi-specific gate.
- Before editing `apps/conary/src/commands/remi_publish.rs`, state that the command remains upload orchestration; trust decisions stay server-side.
- Keep `native_publish/persistence.rs` responsible for native DB/TUF consistency. Public handlers should call lookup helpers, not duplicate SQL.
- Keep shared release repository/TUF helper ownership explicit: either move `ensure_release_repository`, key loading, and TUF refresh helpers into a small `release_publish/support.rs` module, or make them `pub(crate)` in `release_publish.rs`; native-specific supersede, native row insertion, native TUF target deletion, and promoted-object cleanup stay in `native_publish/persistence.rs`.

## Checkpoints

- Checkpoint 1 after Task 2: schema/model migration tests pass.
- Checkpoint 2 after Task 4: native publish verification, storage, persistence, and no-converted-row tests pass.
- Checkpoint 3 after Task 5: package metadata/download native lookup tests pass.
- Checkpoint 4 after Task 6: metadata, sparse index, search, chunk reachability, and GC tests pass.
- Checkpoint 5 after Task 7: client release query, resolver identity, and CLI preflight tests pass.
- Checkpoint 6 after Task 9: local Remi client proof and final regression gates pass.

## Review Lock Mapping

| Design concern | Plan owner |
| --- | --- |
| Dedicated native publication state | Task 1 and Task 4 |
| No synthetic `converted_packages` rows | Task 4 |
| `package_release` and full native identity | Task 1, Task 5, Task 7 |
| Non-null normalized native architecture | Task 1 and Task 4 |
| Verified-v2 server inspection | Task 3 |
| M2 publish-gate reuse | Task 3 |
| Local-dev/host release refusal | Task 3 and Task 4 |
| Atomic supersede and last public state preservation | Task 4 |
| Superseded TUF target removal | Task 4 |
| Promoted-object cleanup on failed commit and superseded package cleanup | Task 4 |
| Native metadata/download before conversion fallback | Task 5 |
| Native metadata, sparse index, and search projection | Task 6 |
| Search does not collapse releases | Task 6 |
| Active native chunk reachability and GC protection | Task 6 |
| Client `release` parameter, Remi sync release persistence, and resolver identity | Task 7 |
| Client preflight does not reject v2 before upload | Task 7 |
| End-to-end publish/fetch/install proof | Task 8 |
| Docs-audit and coherency updates | Task 9 |

---

### Task 1: Add Native Publication Schema And Repository Release Identity

**Files:**
- Modify: `crates/conary-core/src/db/schema.rs`
- Modify: `crates/conary-core/src/db/migrations/v41_current.rs`
- Modify: `crates/conary-core/src/db/models/mod.rs`
- Create: `crates/conary-core/src/db/models/native_publication.rs`
- Modify: `crates/conary-core/src/db/models/repository.rs`
- Modify: `crates/conary-core/src/repository/download.rs`
- Modify: `crates/conary-core/src/repository/resolution.rs`
- Modify: `apps/remi/src/server/prewarm.rs`
- Modify: `apps/remi/src/server/federated_index.rs`
- Modify: `apps/remi/src/server/conversion/lookup.rs`
- Modify: `apps/remi/src/server/handlers/sparse.rs`

- [ ] **Step 1: Write failing migration tests**

Add tests to `crates/conary-core/src/db/migrations/v41_current.rs`:

```rust
#[test]
fn test_migrate_v74_adds_native_publications_and_package_release() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    let version: i32 = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, crate::db::schema::SCHEMA_VERSION);

    conn.execute("SELECT package_release FROM repository_packages LIMIT 0", [])
        .unwrap();
    conn.execute("SELECT package_release, architecture, status FROM native_package_publications LIMIT 0", [])
        .unwrap();

    let index_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_repo_packages_unique'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(index_sql.contains("package_release"));
}

#[test]
fn test_migrate_v74_native_noarch_identity_is_unique() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    conn.execute(
        "INSERT INTO repositories (name, url) VALUES ('test-distro', 'remi-release://test-distro')",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, package_release, architecture, checksum, size, download_url)
         VALUES (?1, 'hello', '1.0.0', '1', 'noarch', 'sha256:a', 10, '/v1/chunks/a')",
        [repo_id],
    )
    .unwrap();
    let duplicate = conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, package_release, architecture, checksum, size, download_url)
         VALUES (?1, 'hello', '1.0.0', '1', 'noarch', 'sha256:b', 10, '/v1/chunks/b')",
        [repo_id],
    );
    assert!(duplicate.is_err());
}

#[test]
fn test_migrate_v74_preserves_existing_null_architecture_rows() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE repositories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            url TEXT NOT NULL
        );
        CREATE TABLE repository_packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            architecture TEXT,
            checksum TEXT NOT NULL,
            size INTEGER NOT NULL,
            download_url TEXT NOT NULL
        );
        CREATE UNIQUE INDEX idx_repo_packages_unique
            ON repository_packages(repository_id, name, version, architecture);
        INSERT INTO repositories (name, url) VALUES ('legacy', 'https://legacy.test');
        ",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, architecture, checksum, size, download_url)
         VALUES (1, 'legacy-null-arch', '1.0.0', NULL, 'sha256:a', 10, '/a')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, architecture, checksum, size, download_url)
         VALUES (1, 'legacy-null-arch', '1.0.0', NULL, 'sha256:b', 11, '/b')",
        [],
    )
    .unwrap();

    migrate_v74(&conn).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM repository_packages
             WHERE name = 'legacy-null-arch' AND package_release = ''",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}
```

- [ ] **Step 2: Run migration tests and verify failure**

Run:

```bash
cargo test -p conary-core migrate_v74
```

Expected: FAIL because migration 74 and `package_release` do not exist.

- [ ] **Step 3: Add migration 74**

Update `crates/conary-core/src/db/schema.rs`:

```rust
pub const SCHEMA_VERSION: i32 = 74;
```

Update existing schema-version tests in this file as well, including the current
`assert_eq!(SCHEMA_VERSION, 73)` assertion.

Add case 74 in `apply_migration`:

```rust
74 => migrations::migrate_v74(conn),
```

Add `migrate_v74` in `crates/conary-core/src/db/migrations/v41_current.rs`:

```rust
/// Version 74: Native CCS publication state and release-aware repository identity
pub fn migrate_v74(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 74");

    conn.execute_batch(
        "
        ALTER TABLE repository_packages
            ADD COLUMN package_release TEXT NOT NULL DEFAULT '';

        DROP INDEX IF EXISTS idx_repo_packages_unique;
        CREATE UNIQUE INDEX idx_repo_packages_unique
            ON repository_packages(repository_id, name, version, package_release, architecture);

        CREATE TABLE native_package_publications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            repository_package_id INTEGER NOT NULL REFERENCES repository_packages(id) ON DELETE CASCADE,
            distro TEXT NOT NULL,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            package_release TEXT NOT NULL,
            architecture TEXT NOT NULL,
            package_kind TEXT NOT NULL,
            authority_format_version INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('public', 'superseded', 'rolled_back')),
            content_hash TEXT NOT NULL,
            chunk_hashes_json TEXT NOT NULL,
            total_size INTEGER NOT NULL,
            package_path TEXT NOT NULL,
            target_path TEXT NOT NULL,
            authority_hash TEXT,
            package_signature_key_id TEXT,
            package_signature_public_key_sha256 TEXT,
            build_attestation_hash TEXT,
            build_attestation_signer_key_id TEXT,
            origin_class TEXT,
            hardening_level TEXT,
            provenance_json TEXT,
            trust_status TEXT NOT NULL,
            verification_report_json TEXT,
            published_at TEXT,
            superseded_at TEXT,
            rolled_back_at TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );

        CREATE UNIQUE INDEX idx_native_publications_active_identity
            ON native_package_publications(distro, name, version, package_release, architecture)
            WHERE status = 'public';

        CREATE INDEX idx_native_publications_repo_package
            ON native_package_publications(repository_package_id);
        CREATE INDEX idx_native_publications_chunk_hash
            ON native_package_publications(content_hash);
        ",
    )?;

    info!("Schema version 74 applied successfully (native CCS publication)");
    Ok(())
}
```

Do not add a duplicate-row deletion step to migration 74. The existing non-null repository identity is already protected by `idx_repo_packages_unique`, and legacy rows with `NULL` architecture must be preserved rather than collapsed. Native publication rows normalize absent architecture to `noarch` in `native_package_publications`; do not rewrite existing repository rows as part of this migration.

- [ ] **Step 4: Add native publication model**

Create `crates/conary-core/src/db/models/native_publication.rs`:

```rust
// conary-core/src/db/models/native_publication.rs

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub const NATIVE_NOARCH: &str = "noarch";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePublicationStatus {
    Public,
    Superseded,
    RolledBack,
}

impl NativePublicationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Superseded => "superseded",
            Self::RolledBack => "rolled_back",
        }
    }

    pub fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "public" => Ok(Self::Public),
            "superseded" => Ok(Self::Superseded),
            "rolled_back" => Ok(Self::RolledBack),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                format!("invalid native publication status {other}").into(),
            )),
        }
    }
}

pub fn normalize_native_architecture(architecture: Option<&str>) -> String {
    architecture
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(NATIVE_NOARCH)
        .to_string()
}

#[derive(Debug, Clone)]
pub struct NativePackagePublication {
    pub id: Option<i64>,
    pub repository_id: i64,
    pub repository_package_id: i64,
    pub distro: String,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: String,
    pub package_kind: String,
    pub authority_format_version: i64,
    pub status: NativePublicationStatus,
    pub content_hash: String,
    pub chunk_hashes_json: String,
    pub total_size: i64,
    pub package_path: String,
    pub target_path: String,
    pub trust_status: String,
}

impl NativePackagePublication {
    pub fn find_active(
        conn: &Connection,
        distro: &str,
        name: &str,
        version: Option<&str>,
        package_release: Option<&str>,
        architecture: Option<&str>,
    ) -> Result<Vec<Self>> {
        let mut sql = String::from(
            "SELECT id, repository_id, repository_package_id, distro, name, version,
                    package_release, architecture, package_kind, authority_format_version,
                    status, content_hash, chunk_hashes_json, total_size, package_path,
                    target_path, trust_status
             FROM native_package_publications
             WHERE status = 'public' AND distro = ?1 AND name = ?2",
        );
        let mut values: Vec<String> = vec![distro.to_string(), name.to_string()];
        if let Some(version) = version {
            values.push(version.to_string());
            sql.push_str(&format!(" AND version = ?{}", values.len()));
        }
        if let Some(package_release) = package_release {
            values.push(package_release.to_string());
            sql.push_str(&format!(" AND package_release = ?{}", values.len()));
        }
        if let Some(architecture) = architecture {
            values.push(normalize_native_architecture(Some(architecture)));
            sql.push_str(&format!(" AND architecture = ?{}", values.len()));
        }
        sql.push_str(" ORDER BY name, version, package_release, architecture");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), Self::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn active_by_content_hash(
        conn: &Connection,
        content_hash: &str,
    ) -> Result<Option<Self>> {
        let sql = "SELECT id, repository_id, repository_package_id, distro, name, version,
                          package_release, architecture, package_kind, authority_format_version,
                          status, content_hash, chunk_hashes_json, total_size, package_path,
                          target_path, trust_status
                   FROM native_package_publications
                   WHERE status = 'public' AND content_hash = ?1";
        conn.query_row(sql, [content_hash], Self::from_row)
            .optional()
            .map_err(Into::into)
    }

    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let raw_status: String = row.get(10)?;
        Ok(Self {
            id: row.get(0)?,
            repository_id: row.get(1)?,
            repository_package_id: row.get(2)?,
            distro: row.get(3)?,
            name: row.get(4)?,
            version: row.get(5)?,
            package_release: row.get(6)?,
            architecture: row.get(7)?,
            package_kind: row.get(8)?,
            authority_format_version: row.get(9)?,
            status: NativePublicationStatus::from_db(&raw_status)?,
            content_hash: row.get(11)?,
            chunk_hashes_json: row.get(12)?,
            total_size: row.get(13)?,
            package_path: row.get(14)?,
            target_path: row.get(15)?,
            trust_status: row.get(16)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_architecture_normalizes_absent_values() {
        assert_eq!(normalize_native_architecture(None), "noarch");
        assert_eq!(normalize_native_architecture(Some("")), "noarch");
        assert_eq!(normalize_native_architecture(Some(" x86_64 ")), "x86_64");
    }
}
```

Add a row-conversion regression for unknown native publication statuses; the
model must fail closed rather than defaulting an unknown status to `Public`.

Export it from `crates/conary-core/src/db/models/mod.rs`:

```rust
pub mod native_publication;
pub use native_publication::{
    NativePackagePublication, NativePublicationStatus, normalize_native_architecture,
};
```

- [ ] **Step 5: Add `RepositoryPackage.package_release`**

In `crates/conary-core/src/db/models/repository.rs`, add:

```rust
pub package_release: String,
```

Update `COLUMNS`, `COLUMNS_PREFIXED`, `BATCH_INSERT_SQL`, `new`, `insert`, `from_row`, and every direct `RepositoryPackage` struct literal. Default existing callers to:

```rust
package_release: String::new(),
```

The direct literal sites to update are:

- `apps/remi/src/server/prewarm.rs`
- `apps/remi/src/server/federated_index.rs`
- `apps/remi/src/server/handlers/sparse.rs`
- `apps/remi/src/server/conversion/lookup.rs`
- `crates/conary-core/src/db/models/repository.rs`
- `crates/conary-core/src/repository/resolution.rs`
- `crates/conary-core/src/repository/download.rs`

Add a model test:

```rust
#[test]
fn repository_package_round_trips_package_release() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();
    let mut repo = Repository::new("release-test".to_string(), "https://example.test".to_string());
    let repo_id = repo.insert(&conn).unwrap();
    let mut package = RepositoryPackage::new(
        repo_id,
        "hello".to_string(),
        "1.0.0".to_string(),
        "sha256:hello".to_string(),
        42,
        "/v1/chunks/hello".to_string(),
    );
    package.package_release = "2".to_string();
    let id = package.insert(&conn).unwrap();
    let loaded = RepositoryPackage::find_by_id(&conn, id).unwrap().unwrap();
    assert_eq!(loaded.package_release, "2");
}
```

- [ ] **Step 6: Run focused schema/model tests**

Run:

```bash
cargo test -p conary-core migrate_v74
cargo test -p conary-core repository_package_round_trips_package_release
cargo test -p conary-core native_architecture_normalizes_absent_values
cargo test -p conary-core test_migrate_v74_preserves_existing_null_architecture_rows
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/conary-core/src/db/schema.rs \
  crates/conary-core/src/db/migrations/v41_current.rs \
  crates/conary-core/src/db/models/mod.rs \
  crates/conary-core/src/db/models/native_publication.rs \
  crates/conary-core/src/db/models/repository.rs \
  crates/conary-core/src/repository/download.rs \
  crates/conary-core/src/repository/resolution.rs \
  apps/remi/src/server/prewarm.rs \
  apps/remi/src/server/federated_index.rs \
  apps/remi/src/server/conversion/lookup.rs \
  apps/remi/src/server/handlers/sparse.rs
git commit -m "feat(remi): add native publication schema"
```

### Task 2: Add Native Publish Module Skeleton And Verified Artifact Types

**Files:**
- Create: `apps/remi/src/server/native_publish/mod.rs`
- Create: `apps/remi/src/server/native_publish/types.rs`
- Create: `apps/remi/src/server/native_publish/storage.rs`
- Create: `apps/remi/src/server/native_publish/test_support.rs`
- Modify: `apps/remi/src/server/mod.rs`

- [ ] **Step 1: Create module skeleton**

Create `apps/remi/src/server/native_publish/mod.rs`:

```rust
// apps/remi/src/server/native_publish/mod.rs
//! Native CCS publication pipeline for Remi release uploads.

pub mod storage;
pub mod test_support;
pub mod types;

pub use types::{
    NativePublishError, NativePublishErrorCode, NativePublishResult, VerifiedNativeArtifact,
};
```

Add to `apps/remi/src/server/mod.rs`:

```rust
pub mod native_publish;
```

- [ ] **Step 2: Add error and artifact DTOs**

Create `apps/remi/src/server/native_publish/types.rs`:

```rust
// apps/remi/src/server/native_publish/types.rs

use axum::{Json, http::StatusCode, response::{IntoResponse, Response}};
use conary_core::ccs::CcsPackage;
use conary_core::repository::static_repo::publish_gate::PublishLintReport;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NativePublishErrorCode {
    InvalidCcs,
    UnsupportedCcsFormat,
    PackageSignatureFailed,
    PublishGateFailed,
    UntrustedBuildAttestationSigner,
    OutputIdentityMismatch,
    LocalDevArtifactRefused,
    UnsupportedDistro,
    MetadataCommitFailed,
    IoError,
}

#[derive(Debug)]
pub struct NativePublishError {
    pub status: StatusCode,
    pub code: NativePublishErrorCode,
    pub message: String,
}

impl NativePublishError {
    pub fn unprocessable(code: NativePublishErrorCode, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code,
            message: message.into(),
        }
    }

    pub fn internal(code: NativePublishErrorCode, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code,
            message: message.into(),
        }
    }

    pub fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "code": self.code,
                "error": self.message,
            })),
        )
            .into_response()
    }
}

#[derive(Debug)]
pub struct VerifiedNativeArtifact {
    pub package: CcsPackage,
    pub lint: PublishLintReport,
    pub name: String,
    pub version: String,
    pub package_release: String,
    pub architecture: String,
    pub package_kind: String,
    pub authority_format_version: i64,
    pub content_hash: String,
    pub total_size: u64,
}

#[derive(Debug, Clone)]
pub struct NativePublishResult {
    pub distro: String,
    pub package: String,
    pub version: String,
    pub package_release: String,
    pub architecture: String,
    pub path: PathBuf,
    pub size: u64,
    pub content_hash: String,
}
```

- [ ] **Step 3: Add target path helper tests**

Create `apps/remi/src/server/native_publish/storage.rs`:

```rust
// apps/remi/src/server/native_publish/storage.rs

use std::path::PathBuf;

pub fn safe_native_ccs_filename(
    name: &str,
    version: &str,
    package_release: &str,
    architecture: &str,
    content_hash: &str,
) -> String {
    let hash_prefix = content_hash.get(..12).unwrap_or(content_hash);
    format!(
        "{}-{}-{}-{}-{hash_prefix}.ccs",
        target_safe_segment(name),
        target_safe_segment(version),
        target_safe_segment(package_release),
        target_safe_segment(architecture),
    )
}

pub fn native_target_path(
    distro: &str,
    name: &str,
    version: &str,
    package_release: &str,
    architecture: &str,
    content_hash: &str,
) -> String {
    format!(
        "packages/{}/{}",
        target_safe_segment(distro),
        safe_native_ccs_filename(name, version, package_release, architecture, content_hash)
    )
}

fn target_safe_segment(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            output.push(ch);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[derive(Debug, Clone)]
pub struct PromotedNativeArtifact {
    pub package_path: PathBuf,
    pub chunk_path: PathBuf,
    pub target_path: String,
}

impl PromotedNativeArtifact {
    pub fn cleanup_package_path_blocking(package_path: &std::path::Path) {
        let _ = std::fs::remove_file(package_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_target_path_includes_release_arch_and_hash() {
        let path = native_target_path(
            "test-distro",
            "hello/pkg",
            "1.0.0",
            "1",
            "noarch",
            "abcdef0123456789",
        );
        assert_eq!(path, "packages/test-distro/hello%2Fpkg-1.0.0-1-noarch-abcdef012345.ccs");
    }

    #[test]
    fn native_target_path_percent_encodes_release_without_collisions() {
        let slash = native_target_path("fedora", "hello", "1.0.0", "1/2", "x86_64", "abcdef012345");
        let dash = native_target_path("fedora", "hello", "1.0.0", "1-2", "x86_64", "abcdef012345");

        assert!(slash.contains("1%2F2"));
        assert_ne!(slash, dash);
    }
}
```

- [ ] **Step 4: Add test support module shell**

Create `apps/remi/src/server/native_publish/test_support.rs`:

```rust
// apps/remi/src/server/native_publish/test_support.rs
//! Test helpers for native Remi publication.

#[cfg(test)]
pub fn assert_json_code(body: &str, expected: &str) {
    let value: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(value["code"], expected);
}
```

- [ ] **Step 5: Run module tests**

Run:

```bash
cargo test -p remi native_publish::storage
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/remi/src/server/mod.rs apps/remi/src/server/native_publish
git commit -m "feat(remi): add native publish module boundary"
```

### Task 3: Return Verified Packages From The Shared Publish Gate

**Files:**
- Modify: `crates/conary-core/src/repository/static_repo/publish_gate.rs`
- Create: `apps/remi/src/server/native_publish/verify.rs`
- Modify: `apps/remi/src/server/native_publish/mod.rs`
- Test: `crates/conary-core/src/repository/static_repo/publish_gate.rs`

- [ ] **Step 1: Add failing publish-gate helper test**

Before editing `publish_gate.rs`, state the boundary: the shared static publish policy remains in `conary-core`; M4c adds a helper return type for consumers that need the already-verified package.

Add this test near existing publish-gate tests:

```rust
#[test]
fn artifact_gate_candidate_returns_verified_v2_package_for_native_intake() {
    let signer = SigningKeyPair::generate().with_key_id("publisher");
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("native-v2.ccs");
    let mut authority = crate::ccs::v2::test_support::package_authority_with_one_file("native-v2");
    let payloads = crate::ccs::v2::test_support::one_file_payloads_for_tests();
    authority.provenance.origin_class = Some("native-built".to_string());
    authority.provenance.hardening_level = Some("hermetic".to_string());
    let envelope = crate::ccs::attestation::test_support::sample_v2_envelope_for_tests(
        &authority,
        &signer,
        "m2-static-publish-policy-v1",
    );
    crate::ccs::builder::write_v2_ccs_package(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        Some(&envelope),
        None,
    )
    .unwrap();

    let candidate = verify_static_artifact_publish_candidate(
        &package_path,
        &AcceptedStaticSignerSet::from_initial_key("publisher", signer.public_key_base64()),
        "m2-static-publish-policy-v1",
    )
    .unwrap();

    assert!(candidate.package.v2_authority().is_some());
    assert_eq!(candidate.package.name(), "native-v2");
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test -p conary-core artifact_gate_candidate_returns_verified_v2_package_for_native_intake
```

Expected: FAIL because `verify_static_artifact_publish_candidate` does not exist.

- [ ] **Step 3: Add verified candidate helper**

Add in `crates/conary-core/src/repository/static_repo/publish_gate.rs`:

```rust
pub struct StaticArtifactPublishCandidate {
    pub package: CcsPackage,
    pub verification: VerificationResult,
    pub lint: PublishLintReport,
    pub has_v2_authority: bool,
}

pub fn verify_static_artifact_publish_candidate(
    artifact_path: &Path,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<StaticArtifactPublishCandidate> {
    let verification = verify_package_for_static_gate(artifact_path, accepted_signers)?;
    let artifact_path_str = artifact_path
        .to_str()
        .context("artifact path must be valid UTF-8 for CCS parsing")?;
    let has_v2_authority = read_ccs_archive(std::fs::File::open(artifact_path)?)
        .map(|contents| contents.v2_authority.is_some())?;
    let package = if has_v2_authority {
        CcsPackage::parse_verified_v2(artifact_path_str, &verification)
            .map_err(anyhow::Error::from)?
    } else {
        CcsPackage::parse(artifact_path_str).map_err(anyhow::Error::from)?
    };
    let lint = verify_verified_static_artifact_publish_eligibility(
        &package,
        &verification,
        accepted_signers,
        accepted_policy_digest,
    )?;
    Ok(StaticArtifactPublishCandidate {
        package,
        verification,
        lint,
        has_v2_authority,
    })
}
```

Then rewrite `verify_static_artifact_publish_eligibility` to call it:

```rust
pub fn verify_static_artifact_publish_eligibility(
    artifact_path: &Path,
    accepted_signers: &AcceptedStaticSignerSet,
    accepted_policy_digest: &str,
) -> Result<PublishLintReport> {
    Ok(verify_static_artifact_publish_candidate(
        artifact_path,
        accepted_signers,
        accepted_policy_digest,
    )?
    .lint)
}
```

- [ ] **Step 4: Add Remi verification wrapper**

Create `apps/remi/src/server/native_publish/verify.rs`:

```rust
// apps/remi/src/server/native_publish/verify.rs

use super::types::{NativePublishError, NativePublishErrorCode, VerifiedNativeArtifact};
use conary_core::db::models::normalize_native_architecture;
use conary_core::repository::static_repo::publish_gate::{
    AcceptedStaticSignerSet, TrustedArtifactSigner, format_publish_gate_failures,
    verify_static_artifact_publish_candidate,
};
use std::path::Path;

pub const RELEASE_PUBLISH_POLICY_DIGEST: &str = "m2-static-publish-policy-v1";

pub fn accepted_release_signers(
    trusted: &[crate::server::config::TrustedBuildAttestationSigner],
) -> Result<AcceptedStaticSignerSet, NativePublishError> {
    let trusted: Vec<TrustedArtifactSigner> = trusted
        .iter()
        .map(|signer| TrustedArtifactSigner {
            key_id: signer.key_id.clone(),
            public_key: signer.public_key.clone(),
        })
        .collect();
    AcceptedStaticSignerSet::from_trusted_artifact_signers(&trusted).map_err(|error| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::PublishGateFailed,
            error.to_string(),
        )
    })
}

pub fn verify_native_artifact(
    path: &Path,
    size: u64,
    content_hash: String,
    accepted: &AcceptedStaticSignerSet,
) -> Result<VerifiedNativeArtifact, NativePublishError> {
    let candidate = verify_static_artifact_publish_candidate(
        path,
        accepted,
        RELEASE_PUBLISH_POLICY_DIGEST,
    )
    .map_err(classify_native_candidate_error)?;
    if !candidate.has_v2_authority {
        return Err(NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedCcsFormat,
            "release upload must be a native CCS v2 package",
        ));
    }
    if !candidate.lint.is_passed() {
        return Err(NativePublishError::unprocessable(
            NativePublishErrorCode::PublishGateFailed,
            format_publish_gate_failures(&candidate.lint),
        ));
    }
    let authority = candidate.package.v2_authority().ok_or_else(|| {
        NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedCcsFormat,
            "verified package did not expose v2 authority",
        )
    })?;
    if authority.format_version != conary_core::ccs::v2::FORMAT_VERSION_V2 {
        return Err(NativePublishError::unprocessable(
            NativePublishErrorCode::UnsupportedCcsFormat,
            "release upload must use CCS authority format version 2",
        ));
    }
    if authority.provenance.origin_class.as_deref() != Some("native-built")
        || authority.provenance.hardening_level.as_deref() != Some("hermetic")
    {
        return Err(NativePublishError::unprocessable(
            NativePublishErrorCode::PublishGateFailed,
            "release upload must be native-built and hermetic",
        ));
    }
    if authority.identity.release.trim().is_empty() {
        return Err(NativePublishError::unprocessable(
            NativePublishErrorCode::InvalidCcs,
            "native CCS v2 package release must not be empty",
        ));
    }
    Ok(VerifiedNativeArtifact {
        name: authority.identity.name.clone(),
        version: authority.identity.version.clone(),
        package_release: authority.identity.release.clone(),
        architecture: normalize_native_architecture(authority.identity.architecture.as_deref()),
        package_kind: serde_json::to_string(&authority.identity.kind)
            .unwrap_or_else(|_| "package".to_string())
            .trim_matches('"')
            .to_string(),
        authority_format_version: authority.format_version as i64,
        package: candidate.package,
        lint: candidate.lint,
        content_hash,
        total_size: size,
    })
}

fn classify_native_candidate_error(error: anyhow::Error) -> NativePublishError {
    let message = error.to_string();
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("signature") {
        NativePublishErrorCode::PackageSignatureFailed
    } else if lower.contains("attestation") || lower.contains("unaccepted signer") {
        NativePublishErrorCode::PublishGateFailed
    } else {
        NativePublishErrorCode::InvalidCcs
    };
    NativePublishError::unprocessable(code, format!("release artifact gate failed: {message}"))
}
```

Add to `apps/remi/src/server/native_publish/mod.rs`:

```rust
pub mod verify;
```

The wrapper must treat `verify_static_artifact_publish_candidate` as the authority for package signature validation, accepted signer validation, build attestation signature validation, accepted policy digest, hermetic hardening, and output identity matching. Remi-specific checks in this wrapper are only the native publication contract: v2 authority present, format version 2, non-empty `identity.release`, normalized architecture, and native-built/hermetic provenance exposed in the verified authority. Preserve machine-readable error codes: malformed archives return `INVALID_CCS`, non-v2 archives return `UNSUPPORTED_CCS_FORMAT`, package-signature failures return `PACKAGE_SIGNATURE_FAILED`, and publish/attestation gate failures return `PUBLISH_GATE_FAILED`. Add Remi-side negative coverage for a local-dev or host-hardened v2 package returning `PUBLISH_GATE_FAILED` and writing no public state.

- [ ] **Step 5: Run focused publish-gate tests**

Run:

```bash
cargo test -p conary-core artifact_gate_candidate_returns_verified_v2_package_for_native_intake
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p remi local_dev
cargo test -p remi host_hardened
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/conary-core/src/repository/static_repo/publish_gate.rs \
  apps/remi/src/server/native_publish/verify.rs \
  apps/remi/src/server/native_publish/mod.rs
git commit -m "feat(remi): expose verified publish gate candidates"
```

### Task 4: Persist Native Publications Without Converted Rows

**Files:**
- Modify: `apps/remi/src/server/release_publish.rs`
- Modify: `apps/remi/src/server/native_publish/storage.rs`
- Modify: `apps/remi/src/server/native_publish/persistence.rs`
- Modify: `apps/remi/src/server/native_publish/test_support.rs`
- Test: `apps/remi/src/server/release_publish.rs`

- [ ] **Step 1: Flip existing release upload success test expectation**

In `apps/remi/src/server/release_publish.rs`, change `release_upload_with_accepted_signer_publishes_public_metadata` so the accepted upload asserts no converted row and an active native row:

```rust
assert!(!fixture.converted_package_row_exists("hello"));
assert!(fixture.native_publication_row_exists("hello", "1"));
assert!(fixture.public_package_detail_exists("hello"));
assert!(fixture.public_chunk_exists(&artifact.content_hash));
assert!(fixture.tuf_target_exists("hello"));
```

Add this helper to `ReleaseFixture`:

```rust
fn native_publication_row_exists(&self, package: &str, package_release: &str) -> bool {
    let conn = rusqlite::Connection::open(&self.db_path).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM native_package_publications
             WHERE distro = 'test-distro' AND name = ?1 AND package_release = ?2
               AND status = 'public'",
            params![package, package_release],
            |row| row.get(0),
        )
        .unwrap();
    count > 0
}
```

Update `assert_no_public_state` to assert the native row is absent:

```rust
assert!(!fixture.native_publication_row_exists(package, "1"));
```

- [ ] **Step 2: Add supersede/TUF regression tests**

Add tests in `release_publish.rs`:

```rust
#[tokio::test]
async fn release_upload_unsupported_distro_fails_before_storage() {
    let signer = SigningKeyPair::generate().with_key_id("publisher");
    let artifact = attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"payload");
    let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

    let response = fixture.upload_release_to_distro("not-a-target", artifact.bytes).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_text(response).await;
    assert_json_code(&body, "UNSUPPORTED_DISTRO");
    assert_no_public_state(&fixture, "hello", &artifact.content_hash);
}

#[tokio::test]
async fn native_release_replacement_supersedes_old_row_and_target() {
    let signer = SigningKeyPair::generate().with_key_id("publisher");
    let first = attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"first");
    let second = attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"second");
    let fixture = ReleaseFixture::new(vec![trusted_signer(&signer)]);

    assert_eq!(fixture.upload_release(first.bytes).await.status(), StatusCode::CREATED);
    assert_eq!(fixture.upload_release(second.bytes).await.status(), StatusCode::CREATED);

    assert_eq!(fixture.native_status_count("hello", "public"), 1);
    assert_eq!(fixture.native_status_count("hello", "superseded"), 1);
    assert!(!fixture.tuf_target_hash_exists(&first.content_hash));
    assert!(fixture.tuf_target_hash_exists(&second.content_hash));
}

#[tokio::test]
async fn native_release_replacement_failure_keeps_last_public_row() {
    let signer = SigningKeyPair::generate().with_key_id("publisher");
    let first = attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"first");
    let second = attested_release_artifact_with_release(&signer, "hello", "1.0.0", "1", b"second");
    let good = ReleaseFixture::new(vec![trusted_signer(&signer)]);

    assert_eq!(good.upload_release(first.bytes).await.status(), StatusCode::CREATED);
    let broken = good.with_missing_snapshot_key();
    assert_eq!(broken.upload_release(second.bytes).await.status(), StatusCode::INTERNAL_SERVER_ERROR);

    assert_eq!(good.native_status_count("hello", "public"), 1);
    assert!(good.tuf_target_hash_exists(&first.content_hash));
    assert!(!good.tuf_target_hash_exists(&second.content_hash));
    assert!(good.public_chunk_exists(&first.content_hash));
    assert!(!good.public_chunk_exists(&second.content_hash));
}
```

Use existing fixture helpers as the model. If `with_missing_snapshot_key` is awkward, create a second fixture with the same DB path, cache path, and chunk path but missing the snapshot key.

- [ ] **Step 3: Run tests and verify failure**

Run:

```bash
cargo test -p remi release_upload_with_accepted_signer_publishes_public_metadata
cargo test -p remi release_upload_unsupported_distro_fails_before_storage
cargo test -p remi native_release_replacement_supersedes_old_row_and_target
cargo test -p remi native_release_replacement_failure_keeps_last_public_row
```

Expected: FAIL because release upload still writes `converted_packages` and no native persistence exists.

- [ ] **Step 4: Implement storage promotion helpers**

Extend `apps/remi/src/server/native_publish/storage.rs` with:

```rust
pub async fn promote_native_artifact(
    cache_dir: &std::path::Path,
    chunk_dir: &std::path::Path,
    distro: &str,
    staged_path: &std::path::Path,
    artifact: &crate::server::native_publish::VerifiedNativeArtifact,
) -> Result<PromotedNativeArtifact, NativePublishError> {
    let packages_dir = cache_dir.join("releases").join("packages").join(distro);
    tokio::fs::create_dir_all(&packages_dir).await.map_err(|error| {
        NativePublishError::internal(
            NativePublishErrorCode::IoError,
            format!("create native release package directory: {error}"),
        )
    })?;
    let filename = safe_native_ccs_filename(
        &artifact.name,
        &artifact.version,
        &artifact.package_release,
        &artifact.architecture,
        &artifact.content_hash,
    );
    let package_path = packages_dir.join(filename);
    tokio::fs::copy(staged_path, &package_path).await.map_err(|error| {
        NativePublishError::internal(
            NativePublishErrorCode::IoError,
            format!("promote native release package: {error}"),
        )
    })?;
    let chunk_path = crate::server::handlers::cas_object_path(chunk_dir, &artifact.content_hash);
    if let Some(parent) = chunk_path.parent() {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            let _ = tokio::fs::remove_file(&package_path).await;
            return Err(NativePublishError::internal(
                NativePublishErrorCode::IoError,
                format!("create native release chunk directory: {error}"),
            ));
        }
    }
    if let Err(error) = tokio::fs::copy(&package_path, &chunk_path).await {
        let _ = tokio::fs::remove_file(&package_path).await;
        return Err(
            NativePublishError::internal(
                NativePublishErrorCode::IoError,
                format!("promote native release chunk: {error}"),
            )
        );
    }
    Ok(PromotedNativeArtifact {
        package_path,
        chunk_path,
        target_path: native_target_path(
            distro,
            &artifact.name,
            &artifact.version,
            &artifact.package_release,
            &artifact.architecture,
            &artifact.content_hash,
        ),
    })
}

impl PromotedNativeArtifact {
    pub async fn cleanup_public_objects(&self) {
        let _ = tokio::fs::remove_file(&self.package_path).await;
        let _ = tokio::fs::remove_file(&self.chunk_path).await;
    }
}
```

- [ ] **Step 5: Implement persistence transaction**

Create `apps/remi/src/server/native_publish/persistence.rs` with functions:

```rust
// apps/remi/src/server/native_publish/persistence.rs

pub fn commit_native_publication_blocking(
    db_path: &std::path::Path,
    keys_dir: &std::path::Path,
    distro: &str,
    artifact: VerifiedNativeArtifact,
    promoted: PromotedNativeArtifact,
) -> anyhow::Result<()> {
    let mut conn = crate::server::open_runtime_db(db_path)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let repo_id = ensure_release_repository(&tx, distro)?;
    let repo_package_id = upsert_repository_projection(&tx, repo_id, distro, &artifact, &promoted)?;
    let superseded_package_paths = supersede_active_native_identity(&tx, distro, &artifact)?;
    delete_superseded_tuf_targets(&tx, repo_id, distro, &artifact)?;
    insert_native_publication(&tx, repo_id, repo_package_id, distro, &artifact, &promoted)?;
    refresh_release_tuf_metadata(&tx, keys_dir, distro, repo_id, &artifact, &promoted)?;
    tx.commit()?;
    for package_path in superseded_package_paths {
        PromotedNativeArtifact::cleanup_package_path_blocking(&package_path);
    }
    Ok(())
}
```

Add to `apps/remi/src/server/native_publish/mod.rs`:

```rust
pub mod persistence;
```

Keep helper ownership explicit. Put shared release repository and TUF helpers in a small `release_publish/support.rs` module, or leave them in `release_publish.rs` as `pub(crate)` helpers if that is the smaller change. Native-specific helpers stay in `native_publish/persistence.rs`: `supersede_active_native_identity`, `delete_superseded_tuf_targets`, `insert_native_publication`, and `upsert_repository_projection`. Do not call `replace_converted_package`.

Before superseding the active native identity, query the active rows and carry their `package_path`, `content_hash`, and `target_path` through the transaction. Only delete superseded `.ccs` package files after the replacement transaction commits. Do not delete the previous public package file before commit; failed replacement must leave last public state intact.

Use this repository projection metadata:

```rust
let metadata = serde_json::json!({
    "source_kind": "native-ccs",
    "native": true,
    "identity": {
        "name": artifact.name,
        "version": artifact.version,
        "release": artifact.package_release,
        "architecture": artifact.architecture,
    },
    "trust": {
        "status": "verified",
        "hardening_level": "hermetic",
    }
});
```

When inserting the native publication, set `chunk_hashes_json` from the active public artifact hashes, at minimum:

```rust
let chunk_hashes_json = serde_json::to_string(&[artifact.content_hash.clone()])?;
```

The upload-to-GC safety test should publish a package through the release route, call `build_referenced_set`, and assert the artifact `content_hash` is referenced by the native row, not only by manually seeded SQL.

- [ ] **Step 6: Route release upload through native publish**

In `apps/remi/src/server/release_publish.rs`, keep request staging and response shape, but replace artifact inspection/promotion/commit with native equivalents:

```rust
let trusted = state
    .read()
    .await
    .config
    .release_publish
    .trusted_build_attestation_signers
    .clone();
let accepted = native_publish::verify::accepted_release_signers(&trusted)?;
native_publish::verify::validate_supported_release_distro(distro)?;
let content_hash = sha256_file(&staged.path).await?;
let artifact = native_publish::verify::verify_native_artifact(
    &staged.path,
    staged.size,
    content_hash,
    &accepted,
)?;
let promoted = native_publish::storage::promote_native_artifact(
    &cache_dir,
    &chunk_dir,
    distro,
    &staged.path,
    &artifact,
).await?;
let response_package = artifact.name.clone();
let response_version = artifact.version.clone();
let response_release = artifact.package_release.clone();
let response_architecture = artifact.architecture.clone();
let commit = native_publish::persistence::commit_native_publication(
    state,
    distro,
    artifact,
    promoted.clone(),
).await;
if let Err(error) = commit {
    promoted.cleanup_public_objects().await;
    return Err(error);
}
```

Map `NativePublishError` into the existing JSON response format so admin clients still receive `code` and `error`. `validate_supported_release_distro` must check `crate::server::handlers::SUPPORTED_DISTROS` before staging or promotion and return `UNSUPPORTED_DISTRO`.
Build `ReleaseUploadResponse` from the `response_*` values captured before moving `artifact` into the commit call.

The route handler owns commit-failure cleanup. It keeps a clone of `PromotedNativeArtifact`, calls `commit_native_publication`, and calls `promoted.cleanup_public_objects().await` on transaction or TUF metadata failure before returning the error. On success, `commit_native_publication_blocking` removes superseded package files after the commit.

- [ ] **Step 7: Run release upload tests**

Run:

```bash
cargo test -p remi release_upload_
cargo test -p remi remi_release_parity
cargo test -p remi native_release_
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add apps/remi/src/server/release_publish.rs apps/remi/src/server/native_publish
git commit -m "feat(remi): persist native release publications"
```

### Task 5: Serve Native Package Metadata And Downloads

**Files:**
- Modify: `apps/remi/src/server/native_publish/mod.rs`
- Create/modify: `apps/remi/src/server/native_publish/public_lookup.rs`
- Modify: `apps/remi/src/server/handlers/packages.rs`

- [ ] **Step 1: Add package handler tests**

In `apps/remi/src/server/handlers/packages.rs`, add tests near existing publication tests:

```rust
#[test]
fn native_manifest_lookup_prefers_active_native_publication() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "1", "noarch", "/tmp/hello.ccs");

    let manifest = native_manifest_for_package(temp_file.path(), "fedora", "hello", Some("1.0.0"), Some("1"), None)
        .unwrap()
        .expect("native manifest");

    assert_eq!(manifest.name, "hello");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.release.as_deref(), Some("1"));
    assert!(manifest.native);
    assert!(!manifest.converted);
}

#[test]
fn native_manifest_lookup_reports_ambiguous_releases() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "1", "noarch", "/tmp/hello-1.ccs");
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "2", "noarch", "/tmp/hello-2.ccs");

    let error = native_manifest_for_package(temp_file.path(), "fedora", "hello", Some("1.0.0"), None, None)
        .unwrap_err();

    assert!(error.to_string().contains("multiple native releases"));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p remi native_manifest_lookup_
```

Expected: FAIL because native lookup helpers and response fields do not exist.

- [ ] **Step 3: Extend package query and manifest DTOs**

In `apps/remi/src/server/handlers/packages.rs`:

```rust
pub struct PackageQuery {
    pub version: Option<String>,
    pub release: Option<String>,
    #[serde(alias = "architecture")]
    pub arch: Option<String>,
}

pub struct PackageManifest {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    pub distro: String,
    pub chunks: Vec<ChunkRef>,
    pub total_size: u64,
    pub content_hash: String,
    pub native: bool,
    pub converted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scriptlets: Option<ScriptletPackageMetadata>,
}
```

For conversion manifests, set:

```rust
release: None,
native: false,
converted: true,
source_kind: Some("converted".to_string()),
scriptlets: Some(scriptlets),
```

Use the existing constructed scriptlet metadata value for `scriptlets` in
converted manifests. For native manifests, set:

```rust
native: true,
converted: false,
source_kind: Some("native-ccs".to_string()),
release: Some(native.package_release),
scriptlets: None,
```

Native manifest JSON must omit the legacy scriptlet summary entirely; converted
manifest behavior stays unchanged except for wrapping the existing value in
`Some`.

- [ ] **Step 4: Implement native lookup helper**

In `apps/remi/src/server/native_publish/public_lookup.rs`:

```rust
// apps/remi/src/server/native_publish/public_lookup.rs

use conary_core::db::models::{NativePackagePublication, normalize_native_architecture};

pub enum NativeLookup<T> {
    Ready(T),
    Ambiguous(Vec<String>),
    Missing,
}

pub fn active_native_publications(
    db_path: &std::path::Path,
    distro: &str,
    name: &str,
    version: Option<&str>,
    release: Option<&str>,
    architecture: Option<&str>,
) -> anyhow::Result<Vec<NativePackagePublication>> {
    let conn = crate::server::open_runtime_db(db_path)?;
    NativePackagePublication::find_active(
        &conn,
        distro,
        name,
        version,
        release,
        architecture.map(|arch| normalize_native_architecture(Some(arch))).as_deref(),
    )
    .map_err(Into::into)
}
```

After creating the helper, export it from
`apps/remi/src/server/native_publish/mod.rs`:

```rust
pub mod public_lookup;
```

In `packages.rs`, add a blocking native lookup before conversion lookup. If multiple rows remain after version/arch filtering and no release was supplied, return HTTP 409 with:

```json
{
  "code": "NATIVE_RELEASE_AMBIGUOUS",
  "error": "multiple native releases match package request",
  "releases": ["1", "2"]
}
```

- [ ] **Step 5: Route native download before conversion fallback**

In `download_package`, query native active rows before job/converted fallback. For an exact or single native match, stream `native.package_path` through `stream_ccs_file` with analytics. For ambiguous rows, return the same 409 response as metadata.

- [ ] **Step 6: Run package handler tests**

Run:

```bash
cargo test -p remi package_publication_manifest_includes_scriptlets_without_private_path
cargo test -p remi native_manifest_lookup_
cargo test -p remi download_package
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add apps/remi/src/server/native_publish/mod.rs \
  apps/remi/src/server/native_publish/public_lookup.rs \
  apps/remi/src/server/handlers/packages.rs
git commit -m "feat(remi): serve native package downloads"
```

### Task 6: Add Native Rows To Metadata, Sparse Index, Search, Chunks, And GC

**Files:**
- Modify: `apps/remi/src/server/handlers/index.rs`
- Modify: `apps/remi/src/server/handlers/sparse.rs`
- Modify: `apps/remi/src/server/search.rs`
- Modify: `apps/remi/src/server/chunk_gc.rs`
- Modify: `apps/remi/src/server/handlers/chunks.rs`
- Modify: `apps/remi/src/server/handlers/oci.rs`

- [ ] **Step 1: Add metadata/index tests**

In `apps/remi/src/server/handlers/index.rs`, add tests:

```rust
#[test]
fn metadata_includes_native_only_package_as_native_not_converted() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "1", "noarch", "/tmp/hello.ccs");

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
    let hello = metadata.packages.iter().find(|pkg| pkg.name == "hello").unwrap();

    assert_eq!(hello.version, "1.0.0");
    assert_eq!(hello.release.as_deref(), Some("1"));
    assert!(!hello.converted);
    assert_eq!(hello.metadata.as_ref().unwrap()["source_kind"], "native-ccs");
}
```

In `apps/remi/src/server/handlers/sparse.rs`, add:

```rust
#[test]
fn sparse_index_preserves_native_sibling_releases() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "1", "noarch", "/tmp/hello-1.ccs");
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "2", "noarch", "/tmp/hello-2.ccs");

    let entry = build_sparse_entry(temp_file.path(), "fedora", "hello").unwrap().unwrap();

    assert_eq!(entry.versions.len(), 2);
    assert!(entry.versions.iter().any(|v| v.release.as_deref() == Some("1")));
    assert!(entry.versions.iter().any(|v| v.release.as_deref() == Some("2")));
}
```

- [ ] **Step 2: Add search and GC tests**

In `apps/remi/src/server/search.rs`, add:

```rust
#[test]
fn search_rebuild_preserves_native_release_identity_and_converted_false() {
    let (_dir, engine) = create_test_engine();
    let (db, conn) = create_test_db();
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "1", "noarch", "/tmp/hello-1.ccs");
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "2", "noarch", "/tmp/hello-2.ccs");

    engine.rebuild_from_db(db.path()).unwrap();
    let results = engine.search("hello", Some("fedora"), 10).unwrap();

    assert_eq!(results.iter().filter(|result| result.name == "hello").count(), 2);
    assert!(results.iter().all(|result| !result.converted));
}
```

In `apps/remi/src/server/chunk_gc.rs`, extend `test_build_referenced_set`:

```rust
conn.execute(
    "INSERT INTO native_package_publications (
        repository_id, repository_package_id, distro, name, version, package_release,
        architecture, package_kind, authority_format_version, status, content_hash,
        chunk_hashes_json, total_size, package_path, target_path, trust_status
    ) VALUES (1, 1, 'fedora', 'hello', '1.0.0', '1', 'noarch', 'package', 2,
              'public', 'native-content', '[\"native-chunk\"]', 42,
              '/tmp/hello.ccs', 'packages/fedora/hello.ccs', 'verified')",
    [],
)
.unwrap();

assert!(referenced.contains("native-chunk"));
```

In `apps/remi/src/server/handlers/index.rs`, add:

```rust
#[test]
fn native_row_not_filtered_by_conversion_publication_gate() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(&conn, "fedora", "hello", "1.0.0", "1", "noarch", "/tmp/hello.ccs");

    let metadata = build_metadata(temp_file.path(), "fedora").unwrap();
    let hello = metadata.packages.iter().find(|pkg| pkg.name == "hello").unwrap();

    assert!(!hello.converted);
    assert_eq!(hello.metadata.as_ref().unwrap()["source_kind"], "native-ccs");
}
```

- [ ] **Step 3: Run tests and verify failure**

Run:

```bash
cargo test -p remi metadata_includes_native_only_package_as_native_not_converted
cargo test -p remi sparse_index_preserves_native_sibling_releases
cargo test -p remi search_rebuild_preserves_native_release_identity_and_converted_false
cargo test -p remi native_row_not_filtered_by_conversion_publication_gate
cargo test -p remi test_build_referenced_set
```

Expected: FAIL because native rows are not included in these paths.

- [ ] **Step 4: Implement metadata and sparse merges**

Add native row loaders beside converted loaders in `index.rs` and `sparse.rs`. Use `NativePackagePublication::find_active` and produce:

```rust
PackageEntry {
    name: native.name,
    version: native.version,
    release: Some(native.package_release),
    architecture: Some(native.architecture),
    converted: false,
    dependencies: None,
    metadata: Some(serde_json::json!({
        "source_kind": "native-ccs",
        "native": true,
        "trust_status": native.trust_status,
    })),
}
```

Do not let converted rows overwrite a native full key. Update any local
`PackageKey` or map keys used by metadata and sparse generation from
`name + version + architecture` to `name + version + release + architecture`.
For converted rows, use an empty/`None` release component. For native rows, use
the stored `package_release` so sibling native releases cannot collapse into a
single metadata or sparse entry.

- [ ] **Step 5: Implement search identity**

In `apps/remi/src/server/search.rs`:

- add `release: Option<String>` and `source_kind: Option<String>` to `PackageSearchDoc`;
- add `release` and `source_kind` to `SearchResult`;
- add stored Tantivy fields named `release` and `source_kind` in `build_schema`, load them in `SearchEngine::new`, and write/read them in `write_package` and `search`;
- change the delete key from `name + distro` to `name + distro + version + release + architecture` for native-aware documents while keeping `rebuild_from_db` as a full `writer.delete_all_documents()` rebuild;
- include active native rows in `rebuild_from_db` separately from the repository row query so native sibling releases do not get collapsed by the existing "latest row per name/repo" subquery;
- when the repository row query is retained for non-native rows, exclude native projections by checking metadata `source_kind != "native-ccs"` or by using a native full-key set so native rows are indexed only once;
- project native results with `converted = false`.

Use this composite key format:

```rust
fn search_document_key(
    distro: &str,
    name: &str,
    version: &str,
    release: Option<&str>,
    architecture: Option<&str>,
) -> String {
    format!(
        "{}\0{}\0{}\0{}\0{}",
        name,
        distro,
        version,
        release.unwrap_or(""),
        architecture.unwrap_or("")
    )
}
```

Add a search regression test that seeds two native sibling releases and one stale old-key document, runs `rebuild_from_db`, and verifies the two native release documents are present while the old-key document is gone. The stale-document assertion should rely on `rebuild_from_db` clearing the whole Tantivy index before re-indexing, not on deleting old keys one by one.

- [ ] **Step 6: Implement chunk and GC reachability**

In `chunk_gc::build_referenced_set`, add:

```rust
let mut stmt = conn
    .prepare(
        "SELECT chunk_hashes_json FROM native_package_publications
         WHERE status = 'public' AND chunk_hashes_json IS NOT NULL",
    )
    .context("prepare native_package_publications chunk query")?;
```

In public chunk serving paths that call `publication::local_chunk_servable_by_public_gate`, add a native active content lookup before returning not servable:

```rust
if NativePackagePublication::active_by_content_hash(&conn, hash)?.is_some() {
    return Ok(true);
}
```

Keep conversion scriptlet publication gates unchanged for converted rows.
Native rows must not call `classify_converted_package` or require `converted_packages.scriptlet_summary`; their public eligibility is `native_package_publications.status = 'public'` plus verified native trust status.

- [ ] **Step 7: Run focused Remi public surface tests**

Run:

```bash
cargo test -p remi metadata_includes_native_only_package_as_native_not_converted
cargo test -p remi sparse_index_preserves_native_sibling_releases
cargo test -p remi search_rebuild_preserves_native_release_identity_and_converted_false
cargo test -p remi native_row_not_filtered_by_conversion_publication_gate
cargo test -p remi test_build_referenced_set
cargo test -p remi publication
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add apps/remi/src/server/handlers/index.rs \
  apps/remi/src/server/handlers/sparse.rs \
  apps/remi/src/server/search.rs \
  apps/remi/src/server/chunk_gc.rs \
  apps/remi/src/server/handlers/chunks.rs \
  apps/remi/src/server/handlers/oci.rs
git commit -m "feat(remi): expose native publications in public indexes"
```

### Task 7: Add Client Release Query, Resolver Identity, And V2 Preflight

**Files:**
- Modify: `crates/conary-core/src/resolver/identity.rs`
- Modify: `crates/conary-core/src/resolver/provider/mod.rs`
- Modify: `crates/conary-core/src/repository/remi.rs`
- Modify: `crates/conary-core/src/repository/sync/types.rs`
- Modify: `crates/conary-core/src/repository/sync/remi.rs`
- Modify: `apps/conary/src/commands/remi_publish.rs`

- [ ] **Step 1: Add failing client URL tests**

In `crates/conary-core/src/repository/remi.rs`, add:

```rust
#[test]
fn test_build_package_url_with_version_release_and_architecture() {
    let core = RemiClientCore::new("https://remi.example.test").unwrap();
    let url = core.package_url(
        "fedora",
        "hello",
        Some("1.0.0"),
        Some("1"),
        Some("noarch"),
    );
    assert_eq!(
        url,
        "https://remi.example.test/v1/fedora/packages/hello?version=1.0.0&release=1&arch=noarch"
    );
}

#[test]
fn test_build_download_url_with_release() {
    let core = RemiClientCore::new("https://remi.example.test").unwrap();
    let url = core.download_url("fedora", "hello", Some("1.0.0"), Some("1"), Some("noarch"));
    assert_eq!(
        url,
        "https://remi.example.test/v1/fedora/packages/hello/download?version=1.0.0&release=1&arch=noarch"
    );
}
```

- [ ] **Step 2: Add resolver identity test**

In `crates/conary-core/src/resolver/identity.rs`, add:

```rust
#[test]
fn package_identity_loads_repository_package_release() {
    let conn = Connection::open_in_memory().unwrap();
    crate::db::schema::migrate(&conn).unwrap();
    conn.execute("INSERT INTO repositories (name, url, enabled) VALUES ('fedora', 'https://example.test', 1)", [])
        .unwrap();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, package_release, checksum, size, download_url)
         VALUES (1, 'hello', '1.0.0', '2', 'sha256:hello', 42, '/v1/chunks/hello')",
        [],
    )
    .unwrap();

    let packages = PackageIdentity::find_all_by_name(&conn, "hello").unwrap();
    assert_eq!(packages[0].package_release.as_deref(), Some("2"));
}
```

- [ ] **Step 3: Add sync release and v2 preflight tests**

In `crates/conary-core/src/repository/sync/remi.rs`, add:

```rust
#[test]
fn remi_sync_row_preserves_release_and_exact_download_url() {
    let row = remi_sync_row(
        7,
        "https://remi.example.test".to_string(),
        "fedora".to_string(),
        RemiPackageEntry {
            name: "hello".to_string(),
            version: "1.0.0".to_string(),
            release: Some("2".to_string()),
            converted: false,
            architecture: Some("noarch".to_string()),
            dependencies: None,
            metadata: None,
        },
    );

    assert_eq!(row.package.package_release, "2");
    assert_eq!(
        row.package.download_url,
        "https://remi.example.test/v1/fedora/packages/hello/download?version=1.0.0&release=2&arch=noarch"
    );
}
```

In `apps/conary/src/commands/remi_publish.rs`, add:

```rust
#[test]
fn remi_publish_preflight_accepts_v2_package_structure() {
    let temp = tempfile::tempdir().unwrap();
    let signer = conary_core::ccs::signing::SigningKeyPair::generate().with_key_id("local-dev");
    let package_path = temp.path().join("native-v2.ccs");
    let payload = b"hello world\n".to_vec();
    let authority = minimal_v2_authority_for_preflight("hello", &payload);
    let payloads = std::collections::BTreeMap::from([(
        "/usr/bin/hello".to_string(),
        payload,
    )]);
    conary_core::ccs::builder::write_v2_ccs_package(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        None,
        None,
    )
    .unwrap();

    preflight_release_artifact(&package_path).unwrap();
}

fn minimal_v2_authority_for_preflight(
    name: &str,
    payload: &[u8],
) -> conary_core::ccs::v2::schema::AuthorityDocumentV2 {
    use conary_core::ccs::v2::schema::{
        AuthorityDocumentV2, ComponentAuthorityV2, ConflictPolicyV2, FileAuthorityV2, FileTypeV2,
        FORMAT_VERSION_V2, LifecycleAuthorityV2, PackageDataV2, PackageIdentityV2,
        PackageKindTagV2, PackageKindV2, PackagePolicyV2, ProvenanceAuthorityV2,
    };
    use std::collections::BTreeMap;

    AuthorityDocumentV2 {
        format_version: FORMAT_VERSION_V2,
        identity: PackageIdentityV2 {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            release: "1".to_string(),
            architecture: Some("noarch".to_string()),
            platform: Some("linux".to_string()),
            kind: PackageKindTagV2::Package,
        },
        kind: PackageKindV2::Package(PackageDataV2 {
            files: vec![FileAuthorityV2 {
                path: "/usr/bin/hello".to_string(),
                sha256: conary_core::hash::sha256(payload),
                size: payload.len() as u64,
                file_type: FileTypeV2::Regular,
                mode: 0o755,
                owner: "root".to_string(),
                group: "root".to_string(),
                component: "main".to_string(),
                symlink_target: None,
                config: None,
                conflict: ConflictPolicyV2::Error,
            }],
            config: Vec::new(),
            policy: PackagePolicyV2::default(),
        }),
        provides: Vec::new(),
        requires: Vec::new(),
        components: BTreeMap::from([(
            "main".to_string(),
            ComponentAuthorityV2 {
                name: "main".to_string(),
                default: true,
                file_count: 1,
                total_size: payload.len() as u64,
            },
        )]),
        lifecycle: LifecycleAuthorityV2::default(),
        provenance: ProvenanceAuthorityV2 {
            origin_class: Some("native-built".to_string()),
            hardening_level: Some("hermetic".to_string()),
            build_input_identity: Some("sha256:build-input".to_string()),
            hermetic_evidence_hash: Some("sha256:evidence".to_string()),
            foreign_conversion_boundary_hash: None,
        },
        debug_toml_sha256: None,
    }
}
```

- [ ] **Step 4: Run tests and verify failure**

Run:

```bash
cargo test -p conary-core test_build_package_url_with_version_release_and_architecture
cargo test -p conary-core test_build_download_url_with_release
cargo test -p conary-core package_identity_loads_repository_package_release
cargo test -p conary-core remi_sync_row_preserves_release_and_exact_download_url
cargo test -p conary --lib remi_publish_preflight_accepts_v2_package_structure
```

Expected: FAIL because release parameters, resolver release, and v2 preflight are missing.

- [ ] **Step 5: Update Remi client helpers**

Change `package_url`, `download_url`, and callers in `crates/conary-core/src/repository/remi.rs` to include `release: Option<&str>` between `version` and `architecture`:

```rust
if let Some(release) = release {
    let encoded_release = urlencoding::encode(release);
    query.push(format!("release={encoded_release}"));
}
```

Update all call sites with `None` for conversion-era requests until native release metadata is available.

- [ ] **Step 6: Update resolver identity**

Add to `PackageIdentity`:

```rust
pub package_release: Option<String>,
```

Select `rp.package_release` in `find_all_by_name` and set:

```rust
package_release: row.get(4)?,
```

Adjust subsequent row indexes deliberately. Update all `PackageIdentity` test literals with `package_release: None` or `Some("...".to_string())`.
Also update direct `PackageIdentity` literals in
`crates/conary-core/src/resolver/provider/mod.rs` with `package_release: None`
unless the test explicitly needs a native release value.

- [ ] **Step 7: Update sync and preflight**

In `crates/conary-core/src/repository/sync/types.rs`, add release deserialization:

```rust
pub(super) release: Option<String>,
```

Update existing `RemiPackageEntry` test literals in
`crates/conary-core/src/repository/sync/remi.rs` with `release: None` before
adding the native-release regression that uses `Some("2".to_string())`.

In `crates/conary-core/src/repository/sync/remi.rs`, parse native metadata `identity.release` or top-level `release` into `RepositoryPackage.package_release`:

```rust
let package_release = entry
    .release
    .clone()
    .or_else(|| {
        entry
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/identity/release"))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    })
    .unwrap_or_default();
package.package_release = package_release.clone();
```

Build synced download URLs with exact identity parameters so a later install does not hit `NATIVE_RELEASE_AMBIGUOUS`:

```rust
let mut query = vec![format!("version={}", urlencoding::encode(&entry.version))];
if !package_release.is_empty() {
    query.push(format!("release={}", urlencoding::encode(&package_release)));
}
if let Some(architecture) = entry.architecture.as_deref() {
    query.push(format!("arch={}", urlencoding::encode(architecture)));
}
let download_url = format!(
    "{endpoint}/v1/{distro}/packages/{}/download?{}",
    urlencoding::encode(&entry.name),
    query.join("&")
);
```

In `apps/conary/src/commands/remi_publish.rs`, replace legacy parse-only preflight with structural archive detection:

```rust
fn preflight_release_artifact(artifact_path: &Path) -> Result<()> {
    let file = std::fs::File::open(artifact_path)
        .with_context(|| format!("open Remi release artifact {}", artifact_path.display()))?;
    let contents = conary_core::ccs::archive_reader::read_ccs_archive(file)
        .with_context(|| format!("preflight CCS artifact {}", artifact_path.display()))?;
    if contents.v2_authority.is_some() {
        return Ok(());
    }
    let path = artifact_path
        .to_str()
        .context("Remi release artifact path must be valid UTF-8")?;
    conary_core::ccs::CcsPackage::parse(path)
        .map(|_| ())
        .map_err(anyhow::Error::from)
        .with_context(|| format!("preflight CCS artifact {}", artifact_path.display()))
}
```

This preflight is structural only; Remi server verification remains authoritative.

- [ ] **Step 8: Run client/resolver tests**

Run:

```bash
cargo test -p conary-core test_build_package_url_with_version_release_and_architecture
cargo test -p conary-core test_build_download_url_with_release
cargo test -p conary-core package_identity_loads_repository_package_release
cargo test -p conary-core remi_sync_row_preserves_release_and_exact_download_url
cargo test -p conary --lib remi_publish_preflight_accepts_v2_package_structure
```

Expected: pass.

- [ ] **Step 9: Commit**

```bash
git add crates/conary-core/src/resolver/identity.rs \
  crates/conary-core/src/resolver/provider/mod.rs \
  crates/conary-core/src/repository/remi.rs \
  crates/conary-core/src/repository/sync/types.rs \
  crates/conary-core/src/repository/sync/remi.rs \
  apps/conary/src/commands/remi_publish.rs
git commit -m "feat(remi): carry native release identity in clients"
```

### Task 8: Add End-To-End Local Remi Native Publish Proof

**Files:**
- Create: `apps/conary/tests/packaging_m4c.rs`
- Modify: `apps/conary/tests/common/mod.rs` if shared test helpers are needed

- [ ] **Step 1: Add integration test skeleton**

Create `apps/conary/tests/packaging_m4c.rs`:

```rust
// apps/conary/tests/packaging_m4c.rs

use std::process::{Command, Output};
use tempfile::TempDir;

#[test]
fn remi_native_publication_fetches_and_installs_without_conversion_row() {
    let fixture = M4cFixture::new();
    let package = fixture.build_release_eligible_v2_package();
    fixture.start_remi();
    fixture.publish_to_remi(&package);
    fixture.sync_remi_repo();
    fixture.download_native_package("hello", "1.0.0", "1", "noarch");
    fixture.install_downloaded_package_dry_run("hello");
    fixture.assert_no_converted_row("hello");
}
```

- [ ] **Step 2: Implement fixture commands**

Implement `M4cFixture` in the same file with helpers that:

- create a temporary Conary database;
- create Remi cache/chunk/key dirs;
- write release TUF role keys;
- create a Remi admin token;
- build or copy a release-eligible v2 fixture using the same fixture builder from Remi tests or a shared test helper;
- start a local Remi router/server;
- run `conary publish` or direct HTTP upload;
- run repository sync against the local Remi endpoint;
- run package download and dry-run install.

Use this command assertion style:

```rust
let status = Command::new(env!("CARGO_BIN_EXE_conary"))
    .args(["publish", package_path.to_str().unwrap(), "--to", &self.remi_url])
    .env("CONARY_REMI_ADMIN_TOKEN", self.admin_token())
    .status()
    .expect("run conary publish");
assert!(status.success(), "conary publish failed");
```

If spinning up the full server is too slow for the first test, use the existing Axum router with `tower::ServiceExt` for publish/fetch and keep a CLI-level dry-run install assertion on the downloaded `.ccs`.

- [ ] **Step 3: Run integration test and verify failure**

Run:

```bash
cargo test -p conary --test packaging_m4c
```

Expected: FAIL until the fixture server and client calls are wired.

- [ ] **Step 4: Wire fixture to implemented M4c surfaces**

Finish the fixture so the test proves:

- upload returns created;
- metadata includes `source_kind = "native-ccs"`;
- download returns the v2 `.ccs`;
- install uses existing v2 CCS install or dry-run test path;
- Remi DB has no `converted_packages` row for the package.

- [ ] **Step 5: Run M4c integration proof**

Run:

```bash
cargo test -p conary --test packaging_m4c
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/conary/tests/packaging_m4c.rs apps/conary/tests/common/mod.rs
git commit -m "test(remi): prove native ccs publication flow"
```

### Task 9: Update Docs, Ledgers, And Final Verification

**Files:**
- Modify: `apps/remi/src/server/handlers/openapi.rs`
- Modify: `docs/modules/remi.md`
- Modify: `docs/modules/test-fixtures.md`
- Modify: `docs/modules/feature-ownership.md`
- Modify: `docs/llms/subsystem-map.md`
- Modify: `docs/superpowers/feature-coherency-ledger.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-inventory.tsv`
- Modify: `docs/superpowers/documentation-accuracy-audit-ledger.tsv`

- [ ] **Step 1: Update Remi docs**

In `docs/modules/remi.md`, update the release upload section to state:

```markdown
Remi release push is the first native CCS publication intake surface. The
route remains `POST /v1/admin/releases/{distro}`, but accepted CCS v2 uploads
are stored in `native_package_publications` and projected into
`repository_packages`; they are not synthetic `converted_packages` rows.
Native uploads stage privately, run the shared static publish gate against
`release_publish.trusted_build_attestation_signers`, and publish package rows,
native rows, chunks, and TUF targets only after the gate and metadata commit
pass.
```

In `apps/remi/src/server/handlers/openapi.rs`, update the release-upload
operation to document native CCS v2 success responses plus machine-readable
native failure codes: `UNSUPPORTED_DISTRO`, `UNSUPPORTED_CCS_FORMAT`,
`INVALID_CCS`, `PACKAGE_SIGNATURE_FAILED`, and `PUBLISH_GATE_FAILED`.

- [ ] **Step 2: Update fixture ownership docs**

In `docs/modules/test-fixtures.md`, add a Remi native publication fixture family:

```markdown
### remi-native-ccs-publication

- **Owner:** Remi native publication: `apps/remi/src/server/native_publish/`
  plus release upload route tests in `apps/remi/src/server/release_publish.rs`.
- **Purpose:** Release-eligible CCS v2 artifacts published through local Remi
  without conversion-shaped storage.
- **Consumes:** `cargo test -p remi release_upload_`;
  `cargo test -p conary --test packaging_m4c`.
- **Safety notes:** fixtures must prove no `converted_packages` row is written,
  local-dev artifacts are refused by the publish gate, and active native chunks
  are protected from garbage collection.
```

- [ ] **Step 3: Update ownership maps**

In `docs/modules/feature-ownership.md`, add `apps/remi/src/server/native_publish/` to the Remi publication start-here list and safety notes.

In `docs/llms/subsystem-map.md`, add `apps/remi/src/server/native_publish/` beside `release_publish.rs` in the Remi routing bullet.

- [ ] **Step 4: Update docs-audit files**

Run:

```bash
bash scripts/docs-audit-inventory.sh > docs/superpowers/documentation-accuracy-audit-inventory.tsv
```

Add or update ledger rows for:

- `docs/superpowers/plans/2026-06-18-m4c-remi-native-ccs-publication-implementation-plan.md`
- `docs/modules/remi.md`
- `docs/modules/test-fixtures.md`
- `docs/modules/feature-ownership.md`
- `docs/llms/subsystem-map.md`

Before changing public route/docs claims, check the feature-coherency ledger for
existing rows tied to Remi release upload, package metadata, sparse metadata,
search, chunk serving, and `conary publish`:

```bash
rg -n "remi|release|publish|metadata|sparse|search|chunk|conary publish" docs/superpowers/feature-coherency-ledger.tsv
```

Update or add coherency rows when a public claim or route behavior changes, then
validate the ledger in the broad verification step.

- [ ] **Step 5: Run final focused regression suites**

Run:

```bash
cargo test -p remi release_upload_
cargo test -p remi remi_release_parity
cargo test -p remi publication
cargo test -p remi
cargo test -p conary --test packaging_m4a
cargo test -p conary --test packaging_m4b
cargo test -p conary --test packaging_m4c
cargo test -p conary --test packaging_m2a
cargo test -p conary --lib commands::publish
cargo test -p conary-core repository::static_repo::publish_gate
cargo test -p conary-core
```

Expected: all pass.

- [ ] **Step 6: Run broad verification**

Run:

```bash
bash -n scripts/docs-audit-inventory.sh
bash -n scripts/check-doc-audit-ledger.sh
bash -n scripts/check-coherency-ledger.sh
bash -n scripts/check-coherency-wave-scopes.sh
bash scripts/docs-audit-inventory.sh | diff -u docs/superpowers/documentation-accuracy-audit-inventory.tsv -
bash scripts/check-doc-audit-ledger.sh docs/superpowers/documentation-accuracy-audit-ledger.tsv --require-complete
bash scripts/check-coherency-ledger.sh docs/superpowers/feature-coherency-ledger.tsv
bash scripts/check-coherency-wave-scopes.sh docs/superpowers/feature-coherency-ledger.tsv docs/superpowers/feature-coherency-wave-scopes.tsv
bash scripts/check-doc-truth.sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add apps/remi/src/server/handlers/openapi.rs \
  docs/modules/remi.md \
  docs/modules/test-fixtures.md \
  docs/modules/feature-ownership.md \
  docs/llms/subsystem-map.md \
  docs/superpowers/feature-coherency-ledger.tsv \
  docs/superpowers/documentation-accuracy-audit-inventory.tsv \
  docs/superpowers/documentation-accuracy-audit-ledger.tsv
git commit -m "docs(remi): document native ccs publication"
```

## Final Completion Gate

Before marking M4c implemented, verify:

- `native_package_publications` is the native source of truth.
- Native release uploads write no `converted_packages` row.
- Local-dev or host-hardened M4b packages fail release publication.
- A release-eligible v2 package publishes to local Remi.
- Replacement for the same native identity supersedes old rows and removes/deactivates old TUF targets.
- Failed replacement preserves the last public native publication.
- Failed replacement removes the newly promoted package and chunk files.
- Successful replacement removes/deactivates old TUF targets and removes the superseded `.ccs` package file only after commit.
- Native package metadata/download works by `version + release + architecture`.
- Version-only native ambiguity returns a conflict with available releases.
- Metadata, sparse index, search, chunk serving, and GC all include active native rows.
- Search distinguishes sibling releases and keeps native results `converted = false`.
- Remi sync stores native `package_release` and exact download URLs with `version`, `release`, and `arch` query parameters.
- Conary can fetch and install the Remi-published native package through existing v2 install paths.
- M2 publish-gate regression suites still pass.

## Review Before Implementation

This plan should receive the normal review loop before `/goal` implementation:

```bash
scripts/agentic-plan-review.sh docs/superpowers/plans/2026-06-18-m4c-remi-native-ccs-publication-implementation-plan.md --review-kind plan --only all
```

Then run the local agentic review, patch findings, lock the plan, update docs-audit records, and only then start the implementation goal.
