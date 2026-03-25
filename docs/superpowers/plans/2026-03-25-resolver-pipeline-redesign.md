# Resolver Pipeline Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the lossy resolver pipeline with one that carries full package identity (name, version, arch, repo, version_scheme, canonical_id) from sync through resolution, modeled after libsolv's Solvable.

**Architecture:** Add `canonical_id` to `repository_packages`. Create `PackageIdentity` type loaded from a single join. Build a `ProvidesIndex` at resolution start. Replace `ConaryPackage`/`ResolverCandidate` with `PackageIdentity`. Delete graph resolver. Enrich AppStream ingestion (origin + provides). Ingest Repology rules YAML.

**Tech Stack:** Rust 1.94, SQLite (rusqlite), resolvo 0.10.2, serde_yaml (for Repology rules), AppStream XML/YAML parsing

**Spec:** `docs/superpowers/specs/2026-03-25-resolver-pipeline-redesign.md`

---

## File Map

### New files
| File | Responsibility |
|------|---------------|
| `conary-core/src/resolver/identity.rs` | `PackageIdentity` struct, DB loading queries, `find_all_by_name()`, `find_by_canonical()` |
| `conary-core/src/resolver/provides_index.rs` | `ProvidesIndex` struct, `build()`, `find_providers()` |

### Modified files
| File | Change |
|------|--------|
| `conary-core/src/db/migrations/v41_current.rs` | v59 migration: `canonical_id` column, `appstream_provides` table |
| `conary-core/src/db/schema.rs` | Bump to v59, register `migrate_v59` |
| `conary-core/src/resolver/mod.rs` | Gutted and rewritten: only exports identity, provides_index, sat, provider, canonical, conflict, component_resolver |
| `conary-core/src/resolver/sat.rs` | Use `PackageIdentity` + `ProvidesIndex` |
| `conary-core/src/resolver/provider/mod.rs` | Rewrite: load `PackageIdentity`, use `ProvidesIndex`, delete all ConaryPackage code |
| `conary-core/src/resolver/provider/types.rs` | Delete `ConaryPackage`, `ConaryPackageVersion`. Keep `SolverDep`/`ConaryConstraint` |
| `conary-core/src/resolver/provider/traits.rs` | Rewrite: `get_candidates` uses `PackageIdentity`, canonical always included, `sort_candidates` ranks by identity |
| `conary-core/src/resolver/provider/loading.rs` | Rewrite: use `identity.version_scheme` directly, no inference |
| `conary-core/src/resolver/provider/matching.rs` | Rewrite: match with real scheme from identity, delete all fallback/inference paths |
| `conary-core/src/resolver/canonical.rs` | Rewrite: delete `ResolverCandidate`, `CanonicalResolver`. Replace with thin SQL queries on `canonical_id` |
| `conary-core/src/canonical/appstream.rs` | Ingest `origin` + `<provides>` (library, binary, python3, dbus) |
| `conary-core/src/canonical/sync.rs` | Re-link canonical_id, Repology rules ingestion |
| `conary-core/src/canonical/repology.rs` | Add rules YAML parsing alongside API |
| `conary-core/src/repository/sync.rs` | Set `canonical_id` during sync |
| `conary-core/src/repository/resolution_policy.rs` | Rewrite: delete `CandidateOrigin`, accept `&PackageIdentity`, delete all string inference |
| `conary-core/src/repository/selector.rs` | Delete `infer_repo_flavor()` and all flavor inference code |
| `src/commands/install/mod.rs` | Rewrite resolution path: `solve_install()` only, no `Resolver` |
| `src/commands/install/dependencies.rs` | Rewrite: `SatResolution` only, delete `ResolutionPlan` usage |
| `src/commands/install/dep_resolution.rs` | Rewrite: `SatResolution` only, delete `ResolutionPlan` usage |
| `src/commands/install/conversion.rs` | Rewrite: `SatResolution` only |
| `src/commands/remove.rs` | Rewrite: `solve_removal()` only, no `Resolver` |
| `src/commands/query/dependency.rs` | Rewrite: `solve_removal()` instead of `Resolver::check_removal()` |
| `conary-core/tests/canonical.rs` | Update for `CanonicalResolver` changes |
| `conary-core/src/repository/dependencies.rs` | Update resolver type usage |

### Deleted files
| File | Reason |
|------|--------|
| `conary-core/src/resolver/graph.rs` (1,006 lines) | Graph resolver deleted. SAT is the only path. |
| `conary-core/src/resolver/engine.rs` (592 lines) | `Resolver` struct deleted. `solve_install()` / `solve_removal()` are the API. |

### Kept (NOT deleted)
| File | Reason |
|------|--------|
| `conary-core/src/resolver/plan.rs` (27 lines) | `ResolutionPlan` / `MissingDependency` used extensively in install CLI (~15 references). Populated from `SatResolution`. |

**Approach:** No users, no backwards compatibility. Rewrite aggressively. Delete dead code immediately. No shims, no fallback paths to old types. If something references the old API, rewrite it.

---

## Task 1: Schema migration (v59)

**Files:**
- Modify: `conary-core/src/db/migrations/v41_current.rs`
- Modify: `conary-core/src/db/schema.rs`

- [ ] **Step 1: Write failing test for v59 migration**

In `conary-core/src/db/migrations/v41_current.rs` tests section:

```rust
#[test]
fn test_migrate_v59_canonical_id_and_appstream_provides() {
    let conn = Connection::open_in_memory().unwrap();
    migrate(&conn).unwrap();

    // Verify canonical_id column exists on repository_packages
    conn.execute(
        "SELECT canonical_id FROM repository_packages LIMIT 0",
        [],
    ).unwrap();

    // Verify appstream_provides table exists
    conn.execute(
        "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
         VALUES (1, 'library', 'libssl.so.3')",
        [],
    ).expect_err("should fail FK -- no canonical_packages row");

    // Insert a real canonical package, then appstream_provides
    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('openssl', 'package')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
         VALUES (1, 'library', 'libssl.so.3')",
        [],
    ).unwrap();

    // Verify schema version
    let version: i32 = conn.query_row(
        "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(version, crate::db::schema::SCHEMA_VERSION);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_migrate_v59 -- --nocapture`
Expected: FAIL (function not found)

- [ ] **Step 3: Write the migration function**

In `conary-core/src/db/migrations/v41_current.rs`, after `migrate_v58`:

```rust
pub fn migrate_v59(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 59");

    conn.execute_batch(
        "
        -- Add canonical_id to repository_packages for cross-distro identity
        ALTER TABLE repository_packages ADD COLUMN canonical_id INTEGER
            REFERENCES canonical_packages(id) ON DELETE SET NULL;
        CREATE INDEX idx_repo_packages_canonical ON repository_packages(canonical_id);

        -- Backfill from existing package_implementations data
        UPDATE repository_packages SET canonical_id = (
            SELECT pi.canonical_id FROM package_implementations pi
            JOIN repositories r ON repository_packages.repository_id = r.id
            WHERE pi.distro_name = repository_packages.name
              AND pi.distro = r.default_strategy_distro
            LIMIT 1
        ) WHERE canonical_id IS NULL;

        -- Cross-distro provides from AppStream metadata
        CREATE TABLE appstream_provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id) ON DELETE CASCADE,
            provide_type TEXT NOT NULL,
            capability TEXT NOT NULL,
            UNIQUE(canonical_id, provide_type, capability)
        );
        CREATE INDEX idx_appstream_provides_cap ON appstream_provides(capability);
        ",
    )?;

    info!("Schema version 59 applied (canonical_id + appstream_provides)");
    Ok(())
}
```

- [ ] **Step 4: Register migration in schema.rs**

In `conary-core/src/db/schema.rs`:
- Change `SCHEMA_VERSION` from 58 to 59
- Add `59 => migrations::migrate_v59(conn),` to the match

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p conary-core test_migrate_v59 -- --nocapture`
Expected: PASS

- [ ] **Step 6: Run full test suite**

Run: `cargo test`
Expected: All pass (migration is additive)

- [ ] **Step 7: Commit**

```bash
git add conary-core/src/db/migrations/v41_current.rs conary-core/src/db/schema.rs
git commit -m "feat(db): v59 migration -- canonical_id on repository_packages, appstream_provides table"
```

---

## Task 2: PackageIdentity type

**Files:**
- Create: `conary-core/src/resolver/identity.rs`
- Modify: `conary-core/src/resolver/mod.rs`

- [ ] **Step 1: Write failing test**

In `conary-core/src/resolver/identity.rs` tests section:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn test_find_all_by_name_returns_enriched_identity() {
        let (_temp, conn) = create_test_db();

        // Insert a repo + package
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy_distro)
             VALUES ('fedora-41', 'https://example.com', 1, 10, 'fedora-41')",
            [],
        ).unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, architecture, checksum, size, download_url, version_scheme)
             VALUES (?1, 'nginx', '1.24.0', 'x86_64', 'sha256:abc', 1024, 'https://example.com/nginx.rpm', 'rpm')",
            [repo_id],
        ).unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert_eq!(results.len(), 1);

        let id = &results[0];
        assert_eq!(id.name, "nginx");
        assert_eq!(id.version, "1.24.0");
        assert_eq!(id.architecture.as_deref(), Some("x86_64"));
        assert_eq!(id.version_scheme, VersionScheme::Rpm);
        assert_eq!(id.repository_name, "fedora-41");
        assert_eq!(id.repository_distro.as_deref(), Some("fedora-41"));
        assert_eq!(id.repository_priority, 10);
        assert!(id.canonical_id.is_none());
    }

    #[test]
    fn test_find_all_by_name_includes_canonical() {
        let (_temp, conn) = create_test_db();

        // Insert canonical mapping
        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('nginx-web', 'package')",
            [],
        ).unwrap();
        let canonical_id = conn.last_insert_rowid();

        // Insert repo + package with canonical_id
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('fedora-41', 'https://example.com', 1, 10)",
            [],
        ).unwrap();
        let repo_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id)
             VALUES (?1, 'nginx', '1.24.0', 'sha256:abc', 1024, 'https://example.com/nginx.rpm', ?2)",
            rusqlite::params![repo_id, canonical_id],
        ).unwrap();

        let results = PackageIdentity::find_all_by_name(&conn, "nginx").unwrap();
        assert_eq!(results[0].canonical_id, Some(canonical_id));
        assert_eq!(results[0].canonical_name.as_deref(), Some("nginx-web"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_find_all_by_name -- --nocapture`
Expected: FAIL (module not found)

- [ ] **Step 3: Write PackageIdentity struct and loading queries**

Create `conary-core/src/resolver/identity.rs`:

```rust
// conary-core/src/resolver/identity.rs

//! Enriched package identity for resolution.
//!
//! Modeled after libsolv's Solvable: every candidate the resolver considers
//! carries its full provenance (name, version, arch, repo, version_scheme,
//! canonical identity). Loaded from a single join across repository_packages,
//! repositories, and canonical_packages.

use crate::error::Result;
use crate::repository::versioning::VersionScheme;
use rusqlite::{Connection, params};

/// Full package identity for resolution, replacing ConaryPackage and ResolverCandidate.
#[derive(Debug, Clone)]
pub struct PackageIdentity {
    // From repository_packages
    pub repo_package_id: i64,
    pub name: String,
    pub version: String,
    pub architecture: Option<String>,
    pub version_scheme: VersionScheme,

    // From repositories (via join)
    pub repository_id: i64,
    pub repository_name: String,
    pub repository_distro: Option<String>,
    pub repository_priority: i32,

    // From canonical_packages (via canonical_id join, nullable)
    pub canonical_id: Option<i64>,
    pub canonical_name: Option<String>,

    // Installed state (set when matching an installed trove)
    pub installed_trove_id: Option<i64>,
}

impl PackageIdentity {
    /// Load all candidates for a package name across all enabled repos.
    pub fn find_all_by_name(conn: &Connection, name: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT rp.id, rp.name, rp.version, rp.architecture, rp.version_scheme,
                    rp.repository_id, r.name, r.default_strategy_distro, r.priority,
                    rp.canonical_id, cp.name
             FROM repository_packages rp
             JOIN repositories r ON rp.repository_id = r.id
             LEFT JOIN canonical_packages cp ON rp.canonical_id = cp.id
             WHERE rp.name = ?1 AND r.enabled = 1",
        )?;

        let rows = stmt.query_map(params![name], |row| {
            let scheme_str: Option<String> = row.get(4)?;
            let distro_str: Option<String> = row.get(7)?;
            let scheme = parse_version_scheme(scheme_str.as_deref(), distro_str.as_deref());

            Ok(PackageIdentity {
                repo_package_id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                architecture: row.get(3)?,
                version_scheme: scheme,
                repository_id: row.get(5)?,
                repository_name: row.get(6)?,
                repository_distro: row.get(7)?,
                repository_priority: row.get(8)?,
                canonical_id: row.get(9)?,
                canonical_name: row.get(10)?,
                installed_trove_id: None,
            })
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Find all cross-distro equivalent names via canonical_id.
    pub fn find_canonical_equivalents(conn: &Connection, name: &str) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT rp2.name FROM repository_packages rp1
             JOIN repository_packages rp2 ON rp1.canonical_id = rp2.canonical_id
             WHERE rp1.name = ?1 AND rp2.name != ?1 AND rp1.canonical_id IS NOT NULL",
        )?;

        let rows = stmt.query_map(params![name], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

/// Parse version_scheme string with fallback to distro inference.
fn parse_version_scheme(explicit: Option<&str>, distro: Option<&str>) -> VersionScheme {
    match explicit {
        Some("debian") => VersionScheme::Debian,
        Some("arch") => VersionScheme::Arch,
        Some("rpm") => VersionScheme::Rpm,
        Some(_) => VersionScheme::Rpm,
        None => match distro {
            Some(d) if d.starts_with("debian") || d.starts_with("ubuntu") => VersionScheme::Debian,
            Some(d) if d.starts_with("arch") => VersionScheme::Arch,
            _ => VersionScheme::Rpm,
        },
    }
}
```

- [ ] **Step 4: Register module in resolver/mod.rs**

Add `pub mod identity;` to `conary-core/src/resolver/mod.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p conary-core test_find_all_by_name -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add conary-core/src/resolver/identity.rs conary-core/src/resolver/mod.rs
git commit -m "feat(resolver): add PackageIdentity type with enriched loading queries"
```

---

## Task 3: ProvidesIndex

**Files:**
- Create: `conary-core/src/resolver/provides_index.rs`
- Modify: `conary-core/src/resolver/mod.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_provides_index_finds_providers() {
    let (_temp, conn) = create_test_db();
    // Insert repo + package + provides
    // ... (setup a repo package that provides "libssl.so.3")
    let index = ProvidesIndex::build(&conn).unwrap();
    let providers = index.find_providers("libssl.so.3");
    assert_eq!(providers.len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_provides_index -- --nocapture`

- [ ] **Step 3: Write ProvidesIndex**

Create `conary-core/src/resolver/provides_index.rs` with:
- `ProvidesIndex` struct with `HashMap<String, Vec<ProviderEntry>>`
- `ProviderEntry` with `repo_package_id`, `provide_version`, `version_scheme`, `installed_trove_id`
- `build(conn)` loads from `repository_provides` + `provides` (installed) + `appstream_provides`
- `find_providers(capability)` returns `&[ProviderEntry]`
- `find_providers_constrained(capability, constraint, scheme)` filters by version

- [ ] **Step 4: Register in mod.rs, run tests**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(resolver): add ProvidesIndex for O(1) capability lookup"
```

---

## Task 4: Sync sets canonical_id

**Files:**
- Modify: `conary-core/src/repository/sync.rs`

- [ ] **Step 1: Write failing test**

Test that after sync, `repository_packages.canonical_id` is populated when a matching `package_implementations` entry exists.

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Add canonical_id lookup to sync path**

In `normalized_repository_capabilities()` and the Remi sync path, after inserting/updating the repo package row, look up canonical_id from `package_implementations` and set it.

- [ ] **Step 4: Run tests**

Run: `cargo test -p conary-core sync -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(sync): populate canonical_id on repository_packages during sync"
```

---

## Task 5: AppStream origin + provides ingestion

**Files:**
- Modify: `conary-core/src/canonical/appstream.rs`
- Modify: `conary-core/src/db/models/appstream_cache.rs` (or new model file)

- [ ] **Step 1: Write failing test**

Test that AppStream XML with `<provides><library>libfoo.so.1</library></provides>` populates `appstream_provides`.

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Enhance AppStream parser**

Parse `<provides>` children (`library`, `binary`, `python3`, `dbus`) and insert into `appstream_provides` table. Parse `origin` attribute and match to repository.

- [ ] **Step 4: Run tests**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(canonical): ingest AppStream origin and provides (library, binary, python3, dbus)"
```

---

## Task 6: Repology rules YAML ingestion

**Files:**
- Modify: `conary-core/src/canonical/repology.rs`
- Modify: `conary-core/src/canonical/sync.rs`

- [ ] **Step 1: Write failing test**

Test that a YAML rule `{name: httpd, setname: apache}` creates/updates canonical mappings.

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Add rules parser**

Parse Repology rules YAML (specifically `800.renames-and-merges/` rules). Apply `setname` rules to create `canonical_packages` + `package_implementations` entries. Wire into `conary canonical rebuild`.

- [ ] **Step 4: Run tests**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(canonical): ingest Repology rules YAML for cross-distro name mapping"
```

---

## Task 7: SAT provider uses PackageIdentity

**Files:**
- Modify: `conary-core/src/resolver/provider/mod.rs`
- Modify: `conary-core/src/resolver/provider/types.rs`
- Modify: `conary-core/src/resolver/provider/traits.rs`
- Modify: `conary-core/src/resolver/provider/loading.rs`
- Modify: `conary-core/src/resolver/provider/matching.rs`

- [ ] **Step 1: Write failing test**

Test that `ConaryProvider` loads `PackageIdentity` candidates with correct version_scheme and architecture.

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Replace ConaryPackage with PackageIdentity in provider**

- `solvables: Vec<ConaryPackage>` becomes `solvables: Vec<PackageIdentity>`
- `load_repo_packages_for_names()` becomes `load_candidates()` using `PackageIdentity::find_all_by_name()`
- `traits.rs`: `get_candidates` always includes canonical equivalents via `PackageIdentity::find_canonical_equivalents()`
- `sort_candidates` ranks exact-name above canonical using `identity.name` comparison
- `loading.rs`: reads `identity.version_scheme` directly (no `infer_version_scheme`)
- `matching.rs`: uses real scheme from identity
- Remove `ConaryPackage` from `types.rs`

- [ ] **Step 4: Integrate ProvidesIndex**

Replace per-dependency `ProvideEntry` queries with `ProvidesIndex::find_providers()`.

- [ ] **Step 5: Run SAT tests**

Run: `cargo test -p conary-core resolver::sat -- --nocapture`

- [ ] **Step 6: Run full tests**

Run: `cargo test`

- [ ] **Step 7: Commit**

```bash
git commit -m "refactor(resolver): SAT provider uses PackageIdentity + ProvidesIndex"
```

---

## Task 8: Policy enforcement uses PackageIdentity

**Files:**
- Modify: `conary-core/src/repository/resolution_policy.rs`
- Modify: `conary-core/src/repository/selector.rs`
- Modify: `conary-core/src/resolver/canonical.rs`

- [ ] **Step 1: Write failing test**

Test that `accepts_candidate(&identity)` correctly filters by `repository_name` for `RequestScope::Repository`.

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Refactor policy**

- Delete `CandidateOrigin` struct
- Change `accepts_candidate()` to take `&PackageIdentity`
- `RequestScope::Repository` compares `identity.repository_name`
- `RequestScope::DistroFlavor` compares `identity.version_scheme`
- `SourceSelectionProfile.allowed_repositories` checks `identity.repository_name`
- Remove `infer_repo_flavor()` from `selector.rs`
- Simplify `canonical.rs`: remove `ResolverCandidate`, ranking uses `PackageIdentity` fields directly

- [ ] **Step 4: Run policy tests**

Run: `cargo test -p conary-core resolution_policy -- --nocapture`
Run: `cargo test -p conary-core canonical -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git commit -m "refactor(policy): enforcement uses PackageIdentity, delete CandidateOrigin"
```

---

## Task 9: Delete graph resolver + rewrite all callers (combined)

These were originally two tasks but share compilation breakage -- deleting graph.rs/engine.rs breaks every caller simultaneously. They must be done as one atomic task.

**Files:**
- Delete: `conary-core/src/resolver/graph.rs`
- Delete: `conary-core/src/resolver/engine.rs`
- Modify: `conary-core/src/resolver/mod.rs` -- gut and rewrite
- Modify: `conary-core/src/resolver/plan.rs` -- keep but populate from SatResolution
- Modify: `conary-core/src/resolver/sat.rs` -- delete tests that import Resolver/DependencyEdge (lines ~504-552)
- Modify: `src/commands/install/mod.rs` -- replace Resolver with solve_install()
- Modify: `src/commands/install/dependencies.rs` -- replace Resolver/DependencyEdge/ResolutionPlan
- Modify: `src/commands/install/dep_resolution.rs` -- replace ResolutionPlan
- Modify: `src/commands/install/conversion.rs` -- replace ResolutionPlan
- Modify: `src/commands/remove.rs` -- replace Resolver::check_removal with solve_removal()
- Modify: `src/commands/query/dependency.rs` -- replace Resolver::check_removal with solve_removal()
- Modify: `conary-core/tests/canonical.rs` -- update CanonicalResolver usage

- [ ] **Step 1: Delete graph.rs and engine.rs**

```bash
rm conary-core/src/resolver/graph.rs
rm conary-core/src/resolver/engine.rs
```

- [ ] **Step 2: Rewrite resolver/mod.rs**

Strip down to only remaining modules. Delete all graph-resolver tests (lines ~38-370). New re-exports:

```rust
pub mod canonical;
pub mod conflict;
pub mod component_resolver;
pub mod identity;
pub mod plan;
pub mod provider;
pub mod provides_index;
pub mod sat;

pub use conflict::Conflict;
pub use component_resolver::{ComponentResolver, ComponentResolutionPlan, ComponentSpec, MissingComponent};
pub use identity::PackageIdentity;
pub use plan::{ResolutionPlan, MissingDependency};
pub use provides_index::ProvidesIndex;
pub use sat::{SatPackage, SatResolution, SatSource, solve_install, solve_removal};
```

- [ ] **Step 3: Rewrite install/mod.rs**

Delete `Resolver::new()` / `resolver.resolve_install()` calls (~lines 53, 541-545, 851). Replace with `solve_install()`.

- [ ] **Step 4: Rewrite install/dependencies.rs**

Delete all `Resolver`, `DependencyEdge` imports (~line 23). Dependency checking uses `solve_install()`. Missing deps come from resolvo error messages.

- [ ] **Step 5: Rewrite install/dep_resolution.rs + conversion.rs**

Replace `ResolutionPlan` usage -- either keep the type and populate from `SatResolution`, or use `SatResolution` directly if the type is thin enough.

- [ ] **Step 6: Rewrite remove.rs**

Delete `Resolver::new(&conn)?.check_removal()` (~line 90). Use `solve_removal()`.

- [ ] **Step 7: Rewrite query/dependency.rs**

Delete `Resolver::new(&conn)?.check_removal()` (~lines 88-89). Use `solve_removal()`.

- [ ] **Step 8: Fix sat.rs tests**

Delete test functions that import `Resolver`/`DependencyEdge` (~lines 504-552). Replace with tests that use `solve_install()` directly.

- [ ] **Step 9: Update tests/canonical.rs**

Update the ~11 test functions that use `CanonicalResolver::new()` to work with the simplified canonical module.

- [ ] **Step 10: Iterate cargo check until clean**

Run `cargo check` repeatedly, fixing each broken reference. No shims.

- [ ] **Step 11: Run full tests + clippy**

```bash
cargo test && cargo clippy -- -D warnings
```

- [ ] **Step 12: Commit**

```bash
git commit -m "refactor(resolver): delete graph resolver, rewrite all callers to SAT-only"
```

---

## Task 10: Integration tests

**Files:**
- Add test in: `conary-core/src/resolver/sat.rs` or `tests/` directory

- [ ] **Step 1: Write cross-distro resolution test**

Test the full pipeline: sync a Fedora package "httpd" and a Debian package "apache2" with the same canonical_id. Resolve a dependency on the canonical name. Verify the correct package is selected based on the system's distro pin.

- [ ] **Step 2: Write multi-arch test**

Install glibc.x86_64 and glibc.i686 as separate candidates. Verify both appear and neither overwrites the other.

- [ ] **Step 3: Write policy enforcement test**

Test `--repo fedora` restricts candidates to `repository_name == "fedora"` using real `PackageIdentity` fields.

- [ ] **Step 4: Write provides index test**

Test that an AppStream-sourced library provide (`libssl.so.3`) resolves a dependency across distros via the provides index.

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: All pass including new integration tests

- [ ] **Step 6: Commit**

```bash
git commit -m "test(resolver): integration tests for cross-distro, multi-arch, policy, provides"
```

---

## Task 11: Cleanup and documentation

**Files:**
- Modify: `CLAUDE.md` (schema version reference)
- Modify: `.claude/rules/architecture.md`
- Modify: `.claude/rules/resolver.md`

- [ ] **Step 1: Update CLAUDE.md**

Update schema version reference from v58 to v59. Update architecture glossary if resolver description changed.

- [ ] **Step 2: Update .claude/rules/resolver.md**

Reflect the new architecture: PackageIdentity, ProvidesIndex, no graph resolver, SAT-only resolution.

- [ ] **Step 3: Update .claude/rules/architecture.md**

Update the resolver module description in the key modules table.

- [ ] **Step 4: Final full test + clippy**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt --check
```

- [ ] **Step 5: Commit**

```bash
git commit -m "docs: update CLAUDE.md and rules for resolver pipeline redesign"
```
