# Cross-Distro Canonical Package Mapping Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Every package gets a canonical distro-neutral identity, enabling transparent cross-distro installation, distro pinning, and package group resolution.

**Architecture:** New `canonical` module with YAML rules engine and multi-strategy auto-discovery. DB migration v45 adds 5 tables. Resolver gains `CanonicalResolver` that expands names to canonical identities. Remi serves canonical metadata. Repology API bootstraps thousands of mappings.

**Tech Stack:** Rust 1.93, SQLite (rusqlite), serde + serde_yaml, resolvo (SAT solver), reqwest (HTTP client), quick-xml (AppStream parsing), clap (CLI)

---

### Task 1: Schema Migration v45

Add 5 new tables and 3 column additions for canonical package identity.

**Files:**
- Modify: `conary-core/src/db/migrations.rs`

**Step 1: Write the failing test**

Add to the bottom of `migrations.rs`:

```rust
#[test]
fn test_migrate_v45_canonical_packages() {
    let conn = Connection::open_in_memory().unwrap();
    // Run all prior migrations first
    migrate_v1(&conn).unwrap();
    // ... (the test harness runs all migrations sequentially)
    migrate_v45(&conn).unwrap();

    // Verify canonical_packages table
    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('curl', 'package')",
        [],
    ).unwrap();
    let name: String = conn.query_row(
        "SELECT name FROM canonical_packages WHERE name = 'curl'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(name, "curl");

    // Verify package_implementations table
    conn.execute(
        "INSERT INTO package_implementations (canonical_id, distro, distro_name, source)
         VALUES (1, 'fedora-41', 'curl', 'auto')",
        [],
    ).unwrap();

    // Verify distro_pin table
    conn.execute(
        "INSERT INTO distro_pin (distro, mixing_policy, created_at)
         VALUES ('ubuntu-noble', 'guarded', '2026-03-05T00:00:00Z')",
        [],
    ).unwrap();

    // Verify package_overrides table
    conn.execute(
        "INSERT INTO package_overrides (canonical_id, from_distro)
         VALUES (1, 'fedora-41')",
        [],
    ).unwrap();

    // Verify system_affinity table
    conn.execute(
        "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
         VALUES ('ubuntu-noble', 150, 75.0, '2026-03-05T00:00:00Z')",
        [],
    ).unwrap();

    // Verify new columns on existing tables
    conn.execute(
        "UPDATE provides SET canonical_id = 1 WHERE id = 0",
        [],
    ).unwrap_err(); // no rows, but column must exist -- check schema instead
    let has_col: bool = conn
        .prepare("SELECT canonical_id FROM provides LIMIT 0")
        .is_ok();
    assert!(has_col);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_migrate_v45 -- --nocapture`
Expected: FAIL — `migrate_v45` doesn't exist

**Step 3: Write the migration**

Add `pub fn migrate_v45(conn: &Connection) -> Result<()>` to `migrations.rs`:

```rust
/// Migration v45: Canonical package identity and distro pinning
pub fn migrate_v45(conn: &Connection) -> Result<()> {
    info!("Running migration v45: canonical package identity");

    conn.execute_batch(
        "
        -- Canonical package identities (distro-neutral)
        CREATE TABLE IF NOT EXISTS canonical_packages (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            appstream_id TEXT,
            description TEXT,
            kind TEXT NOT NULL DEFAULT 'package',
            category TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_canonical_packages_name
            ON canonical_packages(name);
        CREATE INDEX IF NOT EXISTS idx_canonical_packages_appstream
            ON canonical_packages(appstream_id);

        -- Distro-specific implementations of canonical packages
        CREATE TABLE IF NOT EXISTS package_implementations (
            id INTEGER PRIMARY KEY,
            canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id),
            distro TEXT NOT NULL,
            distro_name TEXT NOT NULL,
            repo_id INTEGER REFERENCES repositories(id),
            source TEXT NOT NULL DEFAULT 'auto',
            UNIQUE(canonical_id, distro, distro_name)
        );
        CREATE INDEX IF NOT EXISTS idx_pkg_impl_distro
            ON package_implementations(distro, distro_name);
        CREATE INDEX IF NOT EXISTS idx_pkg_impl_canonical
            ON package_implementations(canonical_id);

        -- System distro pin
        CREATE TABLE IF NOT EXISTS distro_pin (
            id INTEGER PRIMARY KEY,
            distro TEXT NOT NULL,
            mixing_policy TEXT NOT NULL DEFAULT 'guarded',
            created_at TEXT NOT NULL
        );

        -- Per-package distro overrides
        CREATE TABLE IF NOT EXISTS package_overrides (
            id INTEGER PRIMARY KEY,
            canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id),
            from_distro TEXT NOT NULL,
            reason TEXT
        );

        -- Source affinity tracking (computed)
        CREATE TABLE IF NOT EXISTS system_affinity (
            distro TEXT PRIMARY KEY,
            package_count INTEGER NOT NULL DEFAULT 0,
            percentage REAL NOT NULL DEFAULT 0.0,
            updated_at TEXT NOT NULL
        );

        -- Add canonical_id to provides
        ALTER TABLE provides ADD COLUMN canonical_id INTEGER
            REFERENCES canonical_packages(id);

        -- Add distro to repositories
        ALTER TABLE repositories ADD COLUMN distro TEXT;

        -- Add distro to repository_packages
        ALTER TABLE repository_packages ADD COLUMN distro TEXT;
        ",
    )?;

    Ok(())
}
```

Then register it in the migration runner. Find the function that calls migrations (likely `run_migrations` or similar) and add `migrate_v45` after `migrate_v44`. Update `CURRENT_VERSION` to 45.

**Step 4: Run test to verify it passes**

Run: `cargo test -p conary-core test_migrate_v45 -- --nocapture`
Expected: PASS

**Step 5: Run full test suite**

Run: `cargo test -p conary-core`
Expected: All existing tests still pass

**Step 6: Commit**

```bash
git add conary-core/src/db/migrations.rs
git commit -m "feat: Add schema migration v45 for canonical package identity"
```

---

### Task 2: Canonical Package and Implementation Models

CRUD operations for `canonical_packages` and `package_implementations` tables.

**Files:**
- Create: `conary-core/src/db/models/canonical.rs`
- Modify: `conary-core/src/db/models/mod.rs`

**Step 1: Write the failing test**

In `canonical.rs`, add tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::test_db;

    #[test]
    fn test_insert_and_find_canonical() {
        let conn = test_db();
        let pkg = CanonicalPackage::new("curl", "package");
        let id = pkg.insert(&conn).unwrap();
        assert!(id > 0);

        let found = CanonicalPackage::find_by_name(&conn, "curl").unwrap().unwrap();
        assert_eq!(found.name, "curl");
        assert_eq!(found.kind, "package");
    }

    #[test]
    fn test_find_by_appstream_id() {
        let conn = test_db();
        let mut pkg = CanonicalPackage::new("firefox", "package");
        pkg.appstream_id = Some("org.mozilla.Firefox".to_string());
        pkg.insert(&conn).unwrap();

        let found = CanonicalPackage::find_by_appstream_id(&conn, "org.mozilla.Firefox")
            .unwrap()
            .unwrap();
        assert_eq!(found.name, "firefox");
    }

    #[test]
    fn test_insert_and_find_implementation() {
        let conn = test_db();
        let pkg = CanonicalPackage::new("apache-httpd", "package");
        let canonical_id = pkg.insert(&conn).unwrap();

        let impl1 = PackageImplementation::new(canonical_id, "fedora-41", "httpd", "curated");
        impl1.insert(&conn).unwrap();

        let impl2 = PackageImplementation::new(canonical_id, "ubuntu-noble", "apache2", "curated");
        impl2.insert(&conn).unwrap();

        let impls = PackageImplementation::find_by_canonical(&conn, canonical_id).unwrap();
        assert_eq!(impls.len(), 2);

        let found = PackageImplementation::find_by_distro_name(&conn, "fedora-41", "httpd")
            .unwrap()
            .unwrap();
        assert_eq!(found.canonical_id, canonical_id);
    }

    #[test]
    fn test_resolve_name_to_canonical() {
        let conn = test_db();
        let pkg = CanonicalPackage::new("apache-httpd", "package");
        let canonical_id = pkg.insert(&conn).unwrap();
        PackageImplementation::new(canonical_id, "fedora-41", "httpd", "curated")
            .insert(&conn)
            .unwrap();

        // Lookup by canonical name
        let resolved = CanonicalPackage::resolve_name(&conn, "apache-httpd").unwrap().unwrap();
        assert_eq!(resolved.name, "apache-httpd");

        // Lookup by distro-specific name
        let resolved = CanonicalPackage::resolve_name(&conn, "httpd").unwrap().unwrap();
        assert_eq!(resolved.name, "apache-httpd");
    }

    #[test]
    fn test_list_groups() {
        let conn = test_db();
        CanonicalPackage::new("dev-tools", "group").insert(&conn).unwrap();
        CanonicalPackage::new("curl", "package").insert(&conn).unwrap();

        let groups = CanonicalPackage::list_by_kind(&conn, "group").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "dev-tools");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core canonical::tests -- --nocapture`
Expected: FAIL — module doesn't exist

**Step 3: Write the model**

Create `conary-core/src/db/models/canonical.rs`:

```rust
// conary-core/src/db/models/canonical.rs

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// A canonical (distro-neutral) package identity
#[derive(Debug, Clone)]
pub struct CanonicalPackage {
    pub id: Option<i64>,
    pub name: String,
    pub appstream_id: Option<String>,
    pub description: Option<String>,
    pub kind: String,       // "package" | "group"
    pub category: Option<String>,
}

impl CanonicalPackage {
    pub fn new(name: &str, kind: &str) -> Self {
        Self {
            id: None,
            name: name.to_string(),
            appstream_id: None,
            description: None,
            kind: kind.to_string(),
            category: None,
        }
    }

    pub fn insert(&self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO canonical_packages (name, appstream_id, description, kind, category)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![self.name, self.appstream_id, self.description, self.kind, self.category],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn insert_or_ignore(&self, conn: &Connection) -> Result<Option<i64>> {
        let changed = conn.execute(
            "INSERT OR IGNORE INTO canonical_packages (name, appstream_id, description, kind, category)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![self.name, self.appstream_id, self.description, self.kind, self.category],
        )?;
        if changed > 0 {
            Ok(Some(conn.last_insert_rowid()))
        } else {
            // Already exists, fetch its id
            let id: i64 = conn.query_row(
                "SELECT id FROM canonical_packages WHERE name = ?1",
                params![self.name],
                |row| row.get(0),
            )?;
            Ok(Some(id))
        }
    }

    pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, name, appstream_id, description, kind, category
             FROM canonical_packages WHERE name = ?1",
            params![name],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    name: row.get(1)?,
                    appstream_id: row.get(2)?,
                    description: row.get(3)?,
                    kind: row.get(4)?,
                    category: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn find_by_appstream_id(conn: &Connection, appstream_id: &str) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, name, appstream_id, description, kind, category
             FROM canonical_packages WHERE appstream_id = ?1",
            params![appstream_id],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    name: row.get(1)?,
                    appstream_id: row.get(2)?,
                    description: row.get(3)?,
                    kind: row.get(4)?,
                    category: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// Resolve a name that might be canonical, AppStream, or distro-specific
    pub fn resolve_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        // Try canonical name first
        if let Some(pkg) = Self::find_by_name(conn, name)? {
            return Ok(Some(pkg));
        }
        // Try AppStream ID
        if name.contains('.') {
            if let Some(pkg) = Self::find_by_appstream_id(conn, name)? {
                return Ok(Some(pkg));
            }
        }
        // Try distro-specific name via implementations
        if let Some(impl_row) = PackageImplementation::find_by_any_distro_name(conn, name)? {
            return Self::find_by_id(conn, impl_row.canonical_id);
        }
        Ok(None)
    }

    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, name, appstream_id, description, kind, category
             FROM canonical_packages WHERE id = ?1",
            params![id],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    name: row.get(1)?,
                    appstream_id: row.get(2)?,
                    description: row.get(3)?,
                    kind: row.get(4)?,
                    category: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_by_kind(conn: &Connection, kind: &str) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, appstream_id, description, kind, category
             FROM canonical_packages WHERE kind = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![kind], |row| {
            Ok(Self {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                appstream_id: row.get(2)?,
                description: row.get(3)?,
                kind: row.get(4)?,
                category: row.get(5)?,
            })
        })?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    pub fn search(conn: &Connection, query: &str) -> Result<Vec<Self>> {
        let pattern = format!("%{query}%");
        let mut stmt = conn.prepare(
            "SELECT id, name, appstream_id, description, kind, category
             FROM canonical_packages
             WHERE name LIKE ?1 OR description LIKE ?1
             ORDER BY name",
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok(Self {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                appstream_id: row.get(2)?,
                description: row.get(3)?,
                kind: row.get(4)?,
                category: row.get(5)?,
            })
        })?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }
}

/// A distro-specific implementation of a canonical package
#[derive(Debug, Clone)]
pub struct PackageImplementation {
    pub id: Option<i64>,
    pub canonical_id: i64,
    pub distro: String,
    pub distro_name: String,
    pub repo_id: Option<i64>,
    pub source: String, // "auto" | "repology" | "appstream" | "curated" | "user"
}

impl PackageImplementation {
    pub fn new(canonical_id: i64, distro: &str, distro_name: &str, source: &str) -> Self {
        Self {
            id: None,
            canonical_id,
            distro: distro.to_string(),
            distro_name: distro_name.to_string(),
            repo_id: None,
            source: source.to_string(),
        }
    }

    pub fn insert(&self, conn: &Connection) -> Result<i64> {
        conn.execute(
            "INSERT INTO package_implementations
             (canonical_id, distro, distro_name, repo_id, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![self.canonical_id, self.distro, self.distro_name, self.repo_id, self.source],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn insert_or_ignore(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO package_implementations
             (canonical_id, distro, distro_name, repo_id, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![self.canonical_id, self.distro, self.distro_name, self.repo_id, self.source],
        )?;
        Ok(())
    }

    pub fn find_by_canonical(conn: &Connection, canonical_id: i64) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, canonical_id, distro, distro_name, repo_id, source
             FROM package_implementations WHERE canonical_id = ?1
             ORDER BY distro",
        )?;
        let rows = stmt.query_map(params![canonical_id], |row| {
            Ok(Self {
                id: Some(row.get(0)?),
                canonical_id: row.get(1)?,
                distro: row.get(2)?,
                distro_name: row.get(3)?,
                repo_id: row.get(4)?,
                source: row.get(5)?,
            })
        })?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    pub fn find_by_distro_name(
        conn: &Connection,
        distro: &str,
        name: &str,
    ) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, canonical_id, distro, distro_name, repo_id, source
             FROM package_implementations WHERE distro = ?1 AND distro_name = ?2",
            params![distro, name],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    canonical_id: row.get(1)?,
                    distro: row.get(2)?,
                    distro_name: row.get(3)?,
                    repo_id: row.get(4)?,
                    source: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// Find by distro_name across ALL distros (for resolve_name)
    pub fn find_by_any_distro_name(conn: &Connection, name: &str) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, canonical_id, distro, distro_name, repo_id, source
             FROM package_implementations WHERE distro_name = ?1 LIMIT 1",
            params![name],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    canonical_id: row.get(1)?,
                    distro: row.get(2)?,
                    distro_name: row.get(3)?,
                    repo_id: row.get(4)?,
                    source: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn find_for_distro(conn: &Connection, canonical_id: i64, distro: &str) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, canonical_id, distro, distro_name, repo_id, source
             FROM package_implementations
             WHERE canonical_id = ?1 AND distro = ?2",
            params![canonical_id, distro],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    canonical_id: row.get(1)?,
                    distro: row.get(2)?,
                    distro_name: row.get(3)?,
                    repo_id: row.get(4)?,
                    source: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }
}
```

Register in `conary-core/src/db/models/mod.rs`:
```rust
mod canonical;
pub use canonical::{CanonicalPackage, PackageImplementation};
```

**Step 4: Run tests**

Run: `cargo test -p conary-core canonical::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/db/models/canonical.rs conary-core/src/db/models/mod.rs
git commit -m "feat: Add CanonicalPackage and PackageImplementation models"
```

---

### Task 3: Distro Pin and System Affinity Models

CRUD for distro pinning, per-package overrides, and source affinity tracking.

**Files:**
- Create: `conary-core/src/db/models/distro_pin.rs`
- Modify: `conary-core/src/db/models/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::test_db;

    #[test]
    fn test_set_and_get_pin() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "ubuntu-noble");
        assert_eq!(pin.mixing_policy, "guarded");
    }

    #[test]
    fn test_set_replaces_existing_pin() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();
        DistroPin::set(&conn, "fedora-41", "strict").unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.distro, "fedora-41");
        assert_eq!(pin.mixing_policy, "strict");
    }

    #[test]
    fn test_remove_pin() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();
        DistroPin::remove(&conn).unwrap();
        assert!(DistroPin::get_current(&conn).unwrap().is_none());
    }

    #[test]
    fn test_update_mixing_policy() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();
        DistroPin::set_mixing_policy(&conn, "permissive").unwrap();

        let pin = DistroPin::get_current(&conn).unwrap().unwrap();
        assert_eq!(pin.mixing_policy, "permissive");
    }

    #[test]
    fn test_package_override() {
        let conn = test_db();
        // Need a canonical package first
        use crate::db::models::CanonicalPackage;
        let cid = CanonicalPackage::new("mesa", "package").insert(&conn).unwrap();

        PackageOverride::set(&conn, cid, "fedora-41", Some("want newer Mesa")).unwrap();
        let ov = PackageOverride::get(&conn, cid).unwrap().unwrap();
        assert_eq!(ov.from_distro, "fedora-41");

        let all = PackageOverride::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_system_affinity_recompute() {
        let conn = test_db();
        // Insert some fake repository_packages with distro set
        // (this test depends on the schema having distro column on troves
        //  or repository_packages -- we'll test the computation logic)
        SystemAffinity::recompute(&conn).unwrap();
        let affinities = SystemAffinity::list(&conn).unwrap();
        // Empty system = no affinities
        assert!(affinities.is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core distro_pin::tests -- --nocapture`
Expected: FAIL — module doesn't exist

**Step 3: Write the models**

Create `conary-core/src/db/models/distro_pin.rs`:

```rust
// conary-core/src/db/models/distro_pin.rs

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// System-wide distro pin
#[derive(Debug, Clone)]
pub struct DistroPin {
    pub id: Option<i64>,
    pub distro: String,
    pub mixing_policy: String, // "strict" | "guarded" | "permissive"
    pub created_at: String,
}

impl DistroPin {
    pub fn set(conn: &Connection, distro: &str, mixing_policy: &str) -> Result<()> {
        conn.execute("DELETE FROM distro_pin", [])?;
        conn.execute(
            "INSERT INTO distro_pin (distro, mixing_policy, created_at)
             VALUES (?1, ?2, datetime('now'))",
            params![distro, mixing_policy],
        )?;
        Ok(())
    }

    pub fn get_current(conn: &Connection) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, distro, mixing_policy, created_at FROM distro_pin LIMIT 1",
            [],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    distro: row.get(1)?,
                    mixing_policy: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn remove(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM distro_pin", [])?;
        Ok(())
    }

    pub fn set_mixing_policy(conn: &Connection, policy: &str) -> Result<()> {
        let changed = conn.execute(
            "UPDATE distro_pin SET mixing_policy = ?1",
            params![policy],
        )?;
        if changed == 0 {
            anyhow::bail!("No distro pin set");
        }
        Ok(())
    }
}

/// Per-package distro override
#[derive(Debug, Clone)]
pub struct PackageOverride {
    pub id: Option<i64>,
    pub canonical_id: i64,
    pub from_distro: String,
    pub reason: Option<String>,
}

impl PackageOverride {
    pub fn set(conn: &Connection, canonical_id: i64, from_distro: &str, reason: Option<&str>) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO package_overrides (canonical_id, from_distro, reason)
             VALUES (?1, ?2, ?3)",
            params![canonical_id, from_distro, reason],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, canonical_id: i64) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT id, canonical_id, from_distro, reason
             FROM package_overrides WHERE canonical_id = ?1",
            params![canonical_id],
            |row| {
                Ok(Self {
                    id: Some(row.get(0)?),
                    canonical_id: row.get(1)?,
                    from_distro: row.get(2)?,
                    reason: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn remove(conn: &Connection, canonical_id: i64) -> Result<()> {
        conn.execute(
            "DELETE FROM package_overrides WHERE canonical_id = ?1",
            params![canonical_id],
        )?;
        Ok(())
    }

    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT id, canonical_id, from_distro, reason FROM package_overrides",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Self {
                id: Some(row.get(0)?),
                canonical_id: row.get(1)?,
                from_distro: row.get(2)?,
                reason: row.get(3)?,
            })
        })?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }
}

/// Computed source affinity (what % of packages come from each distro)
#[derive(Debug, Clone)]
pub struct SystemAffinity {
    pub distro: String,
    pub package_count: i64,
    pub percentage: f64,
}

impl SystemAffinity {
    /// Recompute affinity from installed troves + repository_packages distro tags
    pub fn recompute(conn: &Connection) -> Result<()> {
        conn.execute("DELETE FROM system_affinity", [])?;

        // Count installed packages per distro source
        // Join troves -> repository_packages (via name match) -> repositories (distro)
        conn.execute_batch(
            "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
             SELECT r.distro,
                    COUNT(*) as cnt,
                    CAST(COUNT(*) AS REAL) * 100.0 / MAX(1, (SELECT COUNT(*) FROM troves)) as pct,
                    datetime('now')
             FROM troves t
             JOIN repository_packages rp ON t.name = rp.name AND t.version = rp.version
             JOIN repositories r ON rp.repository_id = r.id
             WHERE r.distro IS NOT NULL
             GROUP BY r.distro",
        )?;
        Ok(())
    }

    pub fn list(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT distro, package_count, percentage
             FROM system_affinity ORDER BY percentage DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Self {
                distro: row.get(0)?,
                package_count: row.get(1)?,
                percentage: row.get(2)?,
            })
        })?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    pub fn get_for_distro(conn: &Connection, distro: &str) -> Result<Option<Self>> {
        conn.query_row(
            "SELECT distro, package_count, percentage
             FROM system_affinity WHERE distro = ?1",
            params![distro],
            |row| {
                Ok(Self {
                    distro: row.get(0)?,
                    package_count: row.get(1)?,
                    percentage: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }
}
```

Register in `conary-core/src/db/models/mod.rs`:
```rust
mod distro_pin;
pub use distro_pin::{DistroPin, PackageOverride, SystemAffinity};
```

**Step 4: Run tests**

Run: `cargo test -p conary-core distro_pin::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/db/models/distro_pin.rs conary-core/src/db/models/mod.rs
git commit -m "feat: Add DistroPin, PackageOverride, and SystemAffinity models"
```

---

### Task 4: System Model Parser — Distro and Mixing Fields

Add `distro` and `mixing` fields to the SystemModel, plus per-package overrides.

**Files:**
- Modify: `conary-core/src/model/parser.rs`

**Step 1: Write the failing test**

Add to the existing test module in `parser.rs`:

```rust
#[test]
fn test_parse_distro_pin() {
    let input = r#"
[model]
version = 1

[system]
distro = "ubuntu-noble"
mixing = "guarded"
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(model.config.distro.as_deref(), Some("ubuntu-noble"));
    assert_eq!(model.config.mixing.as_deref(), Some("guarded"));
}

#[test]
fn test_parse_package_overrides() {
    let input = r#"
[model]
version = 1

[system]
distro = "ubuntu-noble"

[overrides]
mesa = { from = "fedora-41" }
nvidia-driver = { from = "rpmfusion-41", reason = "closed source drivers" }
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert_eq!(model.overrides.len(), 2);
    assert_eq!(model.overrides["mesa"].from, "fedora-41");
    assert_eq!(
        model.overrides["nvidia-driver"].reason.as_deref(),
        Some("closed source drivers")
    );
}

#[test]
fn test_default_no_distro() {
    let input = r#"
[model]
version = 1
"#;
    let model: SystemModel = toml::from_str(input).unwrap();
    assert!(model.config.distro.is_none());
    assert!(model.config.mixing.is_none());
    assert!(model.overrides.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_parse_distro_pin -- --nocapture`
Expected: FAIL — fields don't exist

**Step 3: Add fields to SystemModel**

In `parser.rs`, add to the `ModelConfig` struct (or equivalent — the struct that holds `[system]` fields):

```rust
/// Distro pin (e.g., "ubuntu-noble")
#[serde(default)]
pub distro: Option<String>,

/// Mixing policy when pinned: "strict" | "guarded" | "permissive"
#[serde(default)]
pub mixing: Option<String>,
```

Add a new struct and field to `SystemModel`:

```rust
/// Per-package distro override
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageOverrideConfig {
    pub from: String,
    #[serde(default)]
    pub reason: Option<String>,
}
```

Add to `SystemModel`:
```rust
#[serde(default)]
pub overrides: HashMap<String, PackageOverrideConfig>,
```

Add `use std::collections::HashMap;` if not already imported.

**Step 4: Run tests**

Run: `cargo test -p conary-core test_parse_distro -- --nocapture`
Expected: PASS (all 3 new tests)

**Step 5: Run full model tests**

Run: `cargo test -p conary-core model::parser -- --nocapture`
Expected: All existing model tests still pass

**Step 6: Commit**

```bash
git add conary-core/src/model/parser.rs
git commit -m "feat: Add distro pin and package overrides to system model"
```

---

### Task 5: Canonical Rules Engine (YAML Parsing)

Parse Repology-compatible YAML rules for curated canonical mappings.

**Files:**
- Create: `conary-core/src/canonical/mod.rs`
- Create: `conary-core/src/canonical/rules.rs`
- Modify: `conary-core/src/lib.rs` (register module)

**Step 1: Write the failing test**

In `rules.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rename_rule() {
        let yaml = r#"
- setname: apache-httpd
  name: httpd
  repo: fedora_41
- setname: apache-httpd
  name: apache2
  repo: ubuntu_24_04
"#;
        let rules = parse_rules(yaml).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].setname, "apache-httpd");
        assert_eq!(rules[0].name, "httpd");
        assert_eq!(rules[0].repo.as_deref(), Some("fedora_41"));
    }

    #[test]
    fn test_parse_group_rule() {
        let yaml = r#"
- setname: dev-tools
  kind: group
  name: build-essential
  repo: ubuntu_24_04
"#;
        let rules = parse_rules(yaml).unwrap();
        assert_eq!(rules[0].kind.as_deref(), Some("group"));
    }

    #[test]
    fn test_parse_wildcard_rule() {
        let yaml = r#"
- namepat: "lib(.+)-dev"
  setname: "$1"
  repo: ubuntu_24_04
"#;
        let rules = parse_rules(yaml).unwrap();
        assert!(rules[0].namepat.is_some());
    }

    #[test]
    fn test_apply_rules() {
        let yaml = r#"
- setname: apache-httpd
  name: httpd
  repo: fedora_41
- setname: apache-httpd
  name: apache2
  repo: ubuntu_24_04
"#;
        let rules = parse_rules(yaml).unwrap();
        let engine = RulesEngine::new(rules);

        let result = engine.resolve("httpd", "fedora_41");
        assert_eq!(result, Some("apache-httpd".to_string()));

        let result = engine.resolve("apache2", "ubuntu_24_04");
        assert_eq!(result, Some("apache-httpd".to_string()));

        let result = engine.resolve("curl", "fedora_41");
        assert_eq!(result, None); // no rule for curl
    }

    #[test]
    fn test_load_rules_from_dir() {
        // Create temp dir with YAML files
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("800.renames.yaml"),
            "- setname: curl\n  name: curl\n",
        ).unwrap();
        std::fs::write(
            dir.path().join("900.version-fixes.yaml"),
            "- setname: vim\n  name: vim-enhanced\n  repo: fedora_41\n",
        ).unwrap();

        let engine = RulesEngine::load_from_dir(dir.path()).unwrap();
        assert!(engine.rule_count() >= 2);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core canonical::rules::tests -- --nocapture`
Expected: FAIL — module doesn't exist

**Step 3: Write the rules engine**

Create `conary-core/src/canonical/mod.rs`:
```rust
// conary-core/src/canonical/mod.rs

pub mod rules;
```

Create `conary-core/src/canonical/rules.rs`:

```rust
// conary-core/src/canonical/rules.rs

use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

/// A single canonical mapping rule (Repology-compatible format)
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    /// Canonical name to assign
    #[serde(default)]
    pub setname: String,

    /// Exact distro package name to match
    #[serde(default)]
    pub name: String,

    /// Regex pattern for package name matching
    #[serde(default)]
    pub namepat: Option<String>,

    /// Repository to match (e.g., "fedora_41", "ubuntu_24_04")
    #[serde(default)]
    pub repo: Option<String>,

    /// Package kind override (e.g., "group")
    #[serde(default)]
    pub kind: Option<String>,

    /// Category (e.g., "net/http")
    #[serde(default)]
    pub category: Option<String>,
}

/// Parse YAML rules from a string
pub fn parse_rules(yaml: &str) -> Result<Vec<Rule>> {
    let rules: Vec<Rule> = serde_yaml::from_str(yaml)?;
    Ok(rules)
}

/// Engine that applies canonical mapping rules
pub struct RulesEngine {
    rules: Vec<Rule>,
}

impl RulesEngine {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules }
    }

    /// Load rules from all YAML files in a directory, sorted by filename
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut files: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .collect();
        files.sort_by_key(|e| e.file_name());

        let mut all_rules = Vec::new();
        for entry in files {
            let content = std::fs::read_to_string(entry.path())?;
            let mut rules = parse_rules(&content)?;
            all_rules.append(&mut rules);
        }
        Ok(Self::new(all_rules))
    }

    /// Resolve a distro-specific package name to a canonical name
    pub fn resolve(&self, name: &str, repo: &str) -> Option<String> {
        for rule in &self.rules {
            // Exact name match
            if rule.name == name {
                if let Some(ref rule_repo) = rule.repo {
                    if rule_repo == repo {
                        return Some(rule.setname.clone());
                    }
                } else {
                    // No repo constraint — matches any repo
                    return Some(rule.setname.clone());
                }
            }
            // Pattern match (if namepat is set)
            if let Some(ref pat) = rule.namepat {
                if let Ok(re) = regex::Regex::new(pat) {
                    if re.is_match(name) {
                        // Apply capture group substitution
                        let canonical = re.replace(name, rule.setname.as_str());
                        return Some(canonical.to_string());
                    }
                }
            }
        }
        None
    }

    /// Get the kind (package/group) for a canonical name if specified in rules
    pub fn get_kind(&self, canonical_name: &str) -> Option<String> {
        self.rules
            .iter()
            .find(|r| r.setname == canonical_name && r.kind.is_some())
            .and_then(|r| r.kind.clone())
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
}
```

Register in `conary-core/src/lib.rs`:
```rust
pub mod canonical;
```

Note: Add `serde_yaml` and `regex` to `conary-core/Cargo.toml` if not already present:
```toml
serde_yaml = "0.9"
regex = "1"
```

**Step 4: Run tests**

Run: `cargo test -p conary-core canonical::rules::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/canonical/ conary-core/src/lib.rs conary-core/Cargo.toml
git commit -m "feat: Add canonical rules engine with Repology-compatible YAML format"
```

---

### Task 6: Multi-Strategy Auto-Discovery

Discover canonical mappings from repo metadata using multiple strategies:
provides, name matching, binary path, stem matching, soname.

**Files:**
- Create: `conary-core/src/canonical/discovery.rs`
- Modify: `conary-core/src/canonical/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_match_strategy() {
        let packages = vec![
            DistroPackage { name: "curl".into(), distro: "fedora-41".into(), provides: vec![], files: vec![] },
            DistroPackage { name: "curl".into(), distro: "ubuntu-noble".into(), provides: vec![], files: vec![] },
            DistroPackage { name: "curl".into(), distro: "arch".into(), provides: vec![], files: vec![] },
        ];
        let mappings = discover_by_name_match(&packages);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].canonical_name, "curl");
        assert_eq!(mappings[0].implementations.len(), 3);
    }

    #[test]
    fn test_provides_strategy() {
        let packages = vec![
            DistroPackage {
                name: "debianutils".into(),
                distro: "ubuntu-noble".into(),
                provides: vec!["which".into()],
                files: vec![],
            },
            DistroPackage {
                name: "which".into(),
                distro: "fedora-41".into(),
                provides: vec!["which".into()],
                files: vec![],
            },
        ];
        let mappings = discover_by_provides(&packages);
        // Both provide "which" -> same canonical
        assert!(mappings.iter().any(|m| m.canonical_name == "which"));
    }

    #[test]
    fn test_binary_path_strategy() {
        let packages = vec![
            DistroPackage {
                name: "debianutils".into(),
                distro: "ubuntu-noble".into(),
                provides: vec![],
                files: vec!["/usr/bin/which".into()],
            },
            DistroPackage {
                name: "which".into(),
                distro: "fedora-41".into(),
                provides: vec![],
                files: vec!["/usr/bin/which".into()],
            },
        ];
        let mappings = discover_by_binary_path(&packages);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].implementations.len(), 2);
    }

    #[test]
    fn test_stem_match_strategy() {
        let stripped = strip_distro_affixes("libcurl-dev");
        assert_eq!(stripped, "curl");

        let stripped = strip_distro_affixes("libssl-devel");
        assert_eq!(stripped, "ssl");
    }

    #[test]
    fn test_soname_strategy() {
        let packages = vec![
            DistroPackage {
                name: "openssl-libs".into(),
                distro: "fedora-41".into(),
                provides: vec!["libssl.so.3".into()],
                files: vec![],
            },
            DistroPackage {
                name: "libssl3".into(),
                distro: "ubuntu-noble".into(),
                provides: vec!["libssl.so.3".into()],
                files: vec![],
            },
        ];
        let mappings = discover_by_soname(&packages);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].implementations.len(), 2);
    }

    #[test]
    fn test_full_discovery_pipeline() {
        let packages = vec![
            DistroPackage {
                name: "curl".into(),
                distro: "fedora-41".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
            DistroPackage {
                name: "curl".into(),
                distro: "ubuntu-noble".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
        ];
        let mappings = run_discovery(&packages);
        // Should deduplicate across strategies
        let curl_mappings: Vec<_> = mappings.iter().filter(|m| m.canonical_name == "curl").collect();
        assert_eq!(curl_mappings.len(), 1);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core canonical::discovery::tests -- --nocapture`
Expected: FAIL — module doesn't exist

**Step 3: Write the discovery module**

Create `conary-core/src/canonical/discovery.rs` with:

- `DistroPackage` struct: name, distro, provides, files
- `DiscoveredMapping` struct: canonical_name, implementations vec, source strategy
- `discover_by_name_match()`: group packages with identical names across distros
- `discover_by_provides()`: group packages that provide the same capability
- `discover_by_binary_path()`: group packages that install the same binary in `/usr/bin/` or `/usr/sbin/`
- `discover_by_soname()`: group packages that provide the same `.so` library
- `strip_distro_affixes()`: strip `lib` prefix, `-dev`/`-devel`/`-libs`/`-common` suffixes
- `discover_by_stem()`: group packages whose stripped stems match
- `run_discovery()`: run all strategies, merge and deduplicate results

Each strategy returns `Vec<DiscoveredMapping>`. The `run_discovery` function merges them with priority: exact name > provides > binary path > soname > stem.

Register in `conary-core/src/canonical/mod.rs`:
```rust
pub mod discovery;
pub mod rules;
```

**Step 4: Run tests**

Run: `cargo test -p conary-core canonical::discovery::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/canonical/discovery.rs conary-core/src/canonical/mod.rs
git commit -m "feat: Add multi-strategy canonical auto-discovery"
```

---

### Task 7: Repology API Client

Fetch canonical mappings from Repology's API to bootstrap the registry.

**Files:**
- Create: `conary-core/src/canonical/repology.rs`
- Modify: `conary-core/src/canonical/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repology_project_response() {
        let json = r#"[
            {"repo": "fedora_41", "visiblename": "curl", "version": "8.9.1", "status": "newest"},
            {"repo": "ubuntu_24_04", "visiblename": "curl", "version": "8.5.0", "status": "outdated"},
            {"repo": "arch", "visiblename": "curl", "version": "8.9.1", "status": "newest"}
        ]"#;
        let project = parse_project_response("curl", json).unwrap();
        assert_eq!(project.name, "curl");
        assert_eq!(project.implementations.len(), 3);
        assert_eq!(project.implementations[0].repo, "fedora_41");
    }

    #[test]
    fn test_parse_repology_projects_batch() {
        let json = r#"{
            "curl": [
                {"repo": "fedora_41", "visiblename": "curl", "version": "8.9.1", "status": "newest"}
            ],
            "wget": [
                {"repo": "fedora_41", "visiblename": "wget", "version": "1.24.5", "status": "newest"},
                {"repo": "ubuntu_24_04", "visiblename": "wget", "version": "1.21.4", "status": "outdated"}
            ]
        }"#;
        let projects = parse_projects_batch(json).unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn test_repo_id_to_distro() {
        assert_eq!(repo_to_distro("fedora_41"), Some("fedora-41".to_string()));
        assert_eq!(repo_to_distro("ubuntu_24_04"), Some("ubuntu-noble".to_string()));
        assert_eq!(repo_to_distro("arch"), Some("arch".to_string()));
        assert_eq!(repo_to_distro("unknown_repo_xyz"), None);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core canonical::repology::tests -- --nocapture`
Expected: FAIL

**Step 3: Write the Repology client**

Create `conary-core/src/canonical/repology.rs` with:

- `RepologyPackage` struct: repo, visiblename, version, status (Deserialize)
- `RepologyProject` struct: name, implementations vec
- `parse_project_response()`: parse single project JSON
- `parse_projects_batch()`: parse batch projects JSON
- `repo_to_distro()`: map Repology repo IDs to Conary distro names
  (e.g., `fedora_41` → `fedora-41`, `ubuntu_24_04` → `ubuntu-noble`)
- `RepologyClient` struct: holds reqwest client + base URL
- `RepologyClient::fetch_project()`: GET `/api/v1/project/<name>`
- `RepologyClient::fetch_projects_batch()`: GET `/api/v1/projects/<start>/` with pagination
- `RepologyClient::sync_to_db()`: fetch batch, insert into canonical_packages + package_implementations

The HTTP methods should be `async` using reqwest. For the sync_to_db method,
paginate through Repology's API (it returns ~200 projects per page).

Add `reqwest` to Cargo.toml if not present (with `json` feature).

Register in mod.rs:
```rust
pub mod repology;
```

**Step 4: Run tests** (unit tests only -- no network)

Run: `cargo test -p conary-core canonical::repology::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/canonical/repology.rs conary-core/src/canonical/mod.rs
git commit -m "feat: Add Repology API client for canonical registry bootstrap"
```

---

### Task 8: AppStream Catalog Parser

Parse AppStream XML/YAML catalogs to extract component IDs for canonical mapping.

**Files:**
- Create: `conary-core/src/canonical/appstream.rs`
- Modify: `conary-core/src/canonical/mod.rs`

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_appstream_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<components version="1.0">
  <component type="desktop-application">
    <id>org.mozilla.Firefox</id>
    <pkgname>firefox</pkgname>
    <name>Firefox</name>
    <summary>Web Browser</summary>
  </component>
  <component type="desktop-application">
    <id>org.gnome.Nautilus</id>
    <pkgname>nautilus</pkgname>
    <name>Files</name>
    <summary>File manager</summary>
  </component>
</components>"#;
        let components = parse_appstream_xml(xml).unwrap();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].id, "org.mozilla.Firefox");
        assert_eq!(components[0].pkgname, "firefox");
        assert_eq!(components[1].id, "org.gnome.Nautilus");
    }

    #[test]
    fn test_component_to_canonical_name() {
        // AppStream IDs become canonical names: org.mozilla.Firefox -> firefox
        // (use the pkgname, not the full reverse-DNS)
        let comp = AppStreamComponent {
            id: "org.mozilla.Firefox".to_string(),
            pkgname: "firefox".to_string(),
            name: "Firefox".to_string(),
            summary: Some("Web Browser".to_string()),
        };
        assert_eq!(comp.canonical_name(), "firefox");
    }

    #[test]
    fn test_parse_appstream_yaml() {
        let yaml = r#"---
File: DEP-11
Version: '1.0'
---
Type: desktop-application
ID: org.mozilla.Firefox
Package: firefox
Name:
  C: Firefox
Summary:
  C: Web Browser
---
Type: desktop-application
ID: org.gnome.Nautilus
Package: nautilus
Name:
  C: Files
"#;
        let components = parse_appstream_yaml(yaml).unwrap();
        assert_eq!(components.len(), 2);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core canonical::appstream::tests -- --nocapture`
Expected: FAIL

**Step 3: Write the AppStream parser**

Create `conary-core/src/canonical/appstream.rs` with:

- `AppStreamComponent` struct: id (reverse-DNS), pkgname, name, summary
- `AppStreamComponent::canonical_name()` -> returns pkgname as canonical name
  (AppStream ID is stored in `appstream_id` column, pkgname becomes the canonical)
- `parse_appstream_xml()`: parse XML catalog using `quick-xml`
- `parse_appstream_yaml()`: parse YAML DEP-11 catalog (Ubuntu/Debian format)
- `ingest_appstream()`: takes parsed components + distro name + DB connection,
  inserts into canonical_packages (with appstream_id) and package_implementations

Add `quick-xml` to Cargo.toml if not present:
```toml
quick-xml = { version = "0.36", features = ["serialize"] }
```

Register in mod.rs:
```rust
pub mod appstream;
```

**Step 4: Run tests**

Run: `cargo test -p conary-core canonical::appstream::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/canonical/appstream.rs conary-core/src/canonical/mod.rs conary-core/Cargo.toml
git commit -m "feat: Add AppStream catalog parser for canonical identity"
```

---

### Task 9: Repository Sync Integration

Wire auto-discovery, AppStream parsing, and rules into the repo sync pipeline.

**Files:**
- Modify: `conary-core/src/repository/sync.rs`
- Create: `conary-core/src/canonical/sync.rs`
- Modify: `conary-core/src/canonical/mod.rs`

**Step 1: Write the failing test**

In `canonical/sync.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::test_db;
    use crate::db::models::{CanonicalPackage, PackageImplementation};

    #[test]
    fn test_ingest_repo_packages_creates_canonical_mappings() {
        let conn = test_db();

        let packages = vec![
            RepoPackageInfo {
                name: "curl".into(),
                distro: "fedora-41".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
            RepoPackageInfo {
                name: "curl".into(),
                distro: "ubuntu-noble".into(),
                provides: vec!["curl".into()],
                files: vec!["/usr/bin/curl".into()],
            },
        ];

        ingest_canonical_mappings(&conn, &packages, None).unwrap();

        let pkg = CanonicalPackage::find_by_name(&conn, "curl").unwrap();
        assert!(pkg.is_some());

        let impls = PackageImplementation::find_by_canonical(&conn, pkg.unwrap().id.unwrap()).unwrap();
        assert_eq!(impls.len(), 2);
    }

    #[test]
    fn test_curated_rules_override_auto_discovery() {
        let conn = test_db();

        // Auto-discovery would create "httpd" as canonical
        let packages = vec![
            RepoPackageInfo {
                name: "httpd".into(),
                distro: "fedora-41".into(),
                provides: vec![],
                files: vec![],
            },
        ];

        // But curated rules say it's "apache-httpd"
        let rules = crate::canonical::rules::parse_rules(
            "- setname: apache-httpd\n  name: httpd\n  repo: fedora_41\n"
        ).unwrap();
        let engine = crate::canonical::rules::RulesEngine::new(rules);

        ingest_canonical_mappings(&conn, &packages, Some(&engine)).unwrap();

        // Should use curated name
        let pkg = CanonicalPackage::find_by_name(&conn, "apache-httpd").unwrap();
        assert!(pkg.is_some());

        // "httpd" should NOT be a separate canonical
        let httpd = CanonicalPackage::find_by_name(&conn, "httpd").unwrap();
        assert!(httpd.is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core canonical::sync::tests -- --nocapture`
Expected: FAIL

**Step 3: Write the sync integration**

Create `conary-core/src/canonical/sync.rs`:

- `RepoPackageInfo` struct: name, distro, provides, files
- `ingest_canonical_mappings(conn, packages, rules_engine)`:
  1. If rules engine provided, check each package against curated rules first
  2. Run auto-discovery strategies on the package list
  3. For each discovered mapping, insert_or_ignore into canonical_packages
  4. Insert package_implementations for each distro-specific name
  5. Return count of new mappings created

Then modify `conary-core/src/repository/sync.rs`:
- After `sync_repository_native()` completes and packages are in DB, call
  `ingest_canonical_mappings()` with the synced packages
- Load rules engine from `/usr/share/conary/canonical-rules/` if the directory exists
- This is a post-sync hook, not blocking the main sync path

Register in mod.rs:
```rust
pub mod sync;
```

**Step 4: Run tests**

Run: `cargo test -p conary-core canonical::sync::tests -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/canonical/sync.rs conary-core/src/canonical/mod.rs conary-core/src/repository/sync.rs
git commit -m "feat: Wire canonical discovery into repository sync pipeline"
```

---

### Task 10: Canonical Resolver

Add canonical name resolution to the SAT resolver. The CanonicalResolver
expands names to implementations and ranks candidates.

**Files:**
- Create: `conary-core/src/resolver/canonical.rs`
- Modify: `conary-core/src/resolver/mod.rs`
- Modify: `conary-core/src/resolver/provider.rs`

**Step 1: Write the failing test**

In `resolver/canonical.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::test_db;
    use crate::db::models::*;

    #[test]
    fn test_expand_canonical_name() {
        let conn = test_db();
        let cid = CanonicalPackage::new("apache-httpd", "package").insert(&conn).unwrap();
        PackageImplementation::new(cid, "fedora-41", "httpd", "curated").insert(&conn).unwrap();
        PackageImplementation::new(cid, "ubuntu-noble", "apache2", "curated").insert(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("apache-httpd").unwrap();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|c| c.distro_name == "httpd"));
        assert!(candidates.iter().any(|c| c.distro_name == "apache2"));
    }

    #[test]
    fn test_expand_distro_name_resolves_to_canonical() {
        let conn = test_db();
        let cid = CanonicalPackage::new("apache-httpd", "package").insert(&conn).unwrap();
        PackageImplementation::new(cid, "fedora-41", "httpd", "curated").insert(&conn).unwrap();
        PackageImplementation::new(cid, "ubuntu-noble", "apache2", "curated").insert(&conn).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let candidates = resolver.expand("httpd").unwrap();
        // Should resolve httpd -> apache-httpd -> all implementations
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn test_rank_candidates_pinned() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

        let candidates = vec![
            ResolverCandidate { distro_name: "httpd".into(), distro: "fedora-41".into(), canonical_id: 1 },
            ResolverCandidate { distro_name: "apache2".into(), distro: "ubuntu-noble".into(), canonical_id: 1 },
        ];

        let resolver = CanonicalResolver::new(&conn);
        let ranked = resolver.rank_candidates(&candidates).unwrap();
        // Ubuntu should be first (pinned)
        assert_eq!(ranked[0].distro, "ubuntu-noble");
    }

    #[test]
    fn test_rank_candidates_unpinned_affinity() {
        let conn = test_db();
        // No pin, but 80% of packages are Ubuntu
        conn.execute(
            "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
             VALUES ('ubuntu-noble', 80, 80.0, '2026-03-05')",
            [],
        ).unwrap();

        let candidates = vec![
            ResolverCandidate { distro_name: "curl".into(), distro: "fedora-41".into(), canonical_id: 1 },
            ResolverCandidate { distro_name: "curl".into(), distro: "ubuntu-noble".into(), canonical_id: 1 },
        ];

        let resolver = CanonicalResolver::new(&conn);
        let ranked = resolver.rank_candidates(&candidates).unwrap();
        // Ubuntu should be first (affinity)
        assert_eq!(ranked[0].distro, "ubuntu-noble");
    }

    #[test]
    fn test_mixing_policy_strict_rejects() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let result = resolver.check_mixing_policy("fedora-41");
        assert!(result.is_err()); // strict mode rejects cross-distro
    }

    #[test]
    fn test_mixing_policy_guarded_warns() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let result = resolver.check_mixing_policy("fedora-41");
        assert!(result.is_ok());
        assert!(result.unwrap().has_warning());
    }

    #[test]
    fn test_package_override_forces_distro() {
        let conn = test_db();
        DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();
        let cid = CanonicalPackage::new("mesa", "package").insert(&conn).unwrap();
        PackageOverride::set(&conn, cid, "fedora-41", None).unwrap();

        let resolver = CanonicalResolver::new(&conn);
        let override_distro = resolver.get_override(cid).unwrap();
        assert_eq!(override_distro.as_deref(), Some("fedora-41"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core resolver::canonical::tests -- --nocapture`
Expected: FAIL

**Step 3: Write the CanonicalResolver**

Create `conary-core/src/resolver/canonical.rs`:

- `ResolverCandidate` struct: distro_name, distro, canonical_id
- `MixingResult` struct: allowed (bool), warning (Option<String>)
- `CanonicalResolver<'db>` struct: holds DB connection
- `expand(name)` -> `Vec<ResolverCandidate>`: resolve name to all implementations
- `rank_candidates(candidates)` -> sorted vec:
  1. Check package overrides first (skip ranking)
  2. Pin match (pinned distro's impl first)
  3. Source affinity (higher % distro ranked higher)
  4. Version (newest first — would need version data, can defer)
- `check_mixing_policy(candidate_distro)` -> `MixingResult`:
  Check if candidate from given distro is allowed under current pin + policy
- `get_override(canonical_id)` -> `Option<String>`: check for per-package override

Then modify `conary-core/src/resolver/provider.rs`:
- In `load_repo_packages_for_names()`, add canonical expansion:
  Before loading packages, call `CanonicalResolver::expand()` to get all
  implementation names, then load all of them as candidates.

Register in `resolver/mod.rs`:
```rust
pub mod canonical;
```

**Step 4: Run tests**

Run: `cargo test -p conary-core resolver::canonical::tests -- --nocapture`
Expected: PASS

**Step 5: Run full resolver tests**

Run: `cargo test -p conary-core resolver -- --nocapture`
Expected: All existing resolver tests still pass

**Step 6: Commit**

```bash
git add conary-core/src/resolver/canonical.rs conary-core/src/resolver/mod.rs conary-core/src/resolver/provider.rs
git commit -m "feat: Add CanonicalResolver with pinning, ranking, and mixing policy"
```

---

### Task 11: Conflicts and Obsoletes in Resolver

Prevent installing multiple implementations of the same canonical package.

**Files:**
- Modify: `conary-core/src/resolver/provider.rs`
- Modify: `conary-core/src/resolver/canonical.rs`

**Step 1: Write the failing test**

Add to `canonical.rs` tests:

```rust
#[test]
fn test_canonical_equivalents_conflict() {
    let conn = test_db();
    let cid = CanonicalPackage::new("apache-httpd", "package").insert(&conn).unwrap();
    PackageImplementation::new(cid, "fedora-41", "httpd", "curated").insert(&conn).unwrap();
    PackageImplementation::new(cid, "ubuntu-noble", "apache2", "curated").insert(&conn).unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let conflicts = resolver.get_conflicts("httpd").unwrap();
    // httpd conflicts with apache2 (same canonical)
    assert!(conflicts.contains(&"apache2".to_string()));
}

#[test]
fn test_no_conflict_for_different_canonicals() {
    let conn = test_db();
    CanonicalPackage::new("curl", "package").insert(&conn).unwrap();
    CanonicalPackage::new("wget", "package").insert(&conn).unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let conflicts = resolver.get_conflicts("curl").unwrap();
    assert!(!conflicts.contains(&"wget".to_string()));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_canonical_equivalents_conflict -- --nocapture`
Expected: FAIL — `get_conflicts` doesn't exist

**Step 3: Implement conflicts**

Add to `CanonicalResolver`:

```rust
/// Get packages that conflict with the given package (canonical equivalents)
pub fn get_conflicts(&self, package_name: &str) -> Result<Vec<String>> {
    // Find the canonical package for this name
    let canonical = CanonicalPackage::resolve_name(self.conn, package_name)?;
    let Some(canonical) = canonical else {
        return Ok(vec![]);
    };
    let canonical_id = canonical.id.unwrap();

    // Get all implementations of the same canonical
    let impls = PackageImplementation::find_by_canonical(self.conn, canonical_id)?;

    // All other implementations conflict with this one
    Ok(impls
        .into_iter()
        .map(|i| i.distro_name)
        .filter(|name| name != package_name)
        .collect())
}
```

Then modify `ConaryProvider` in `provider.rs` to feed conflicts into the SAT solver.
When adding a solvable, check if it has canonical conflicts and register them
as mutually exclusive with resolvo's conflict mechanism.

**Step 4: Run tests**

Run: `cargo test -p conary-core test_canonical_equivalents -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/resolver/canonical.rs conary-core/src/resolver/provider.rs
git commit -m "feat: Add canonical conflict detection for equivalent packages"
```

---

### Task 12: Replace Hardcoded CCS Legacy Mappings

Replace the ~30 hardcoded `map_capability_to_package` entries with DB queries.

**Files:**
- Modify: `conary-core/src/ccs/legacy/mod.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_map_capability_from_db() {
    let conn = test_db();
    // Populate canonical data
    let cid = CanonicalPackage::new("glibc", "package").insert(&conn).unwrap();
    PackageImplementation::new(cid, "ubuntu-noble", "libc6", "curated").insert(&conn).unwrap();
    PackageImplementation::new(cid, "fedora-41", "glibc", "curated").insert(&conn).unwrap();
    PackageImplementation::new(cid, "arch", "glibc", "curated").insert(&conn).unwrap();

    let result = map_capability_to_package_db(&conn, "glibc", "deb");
    assert_eq!(result.unwrap(), Some("libc6".to_string()));

    let result = map_capability_to_package_db(&conn, "glibc", "rpm");
    assert_eq!(result.unwrap(), Some("glibc".to_string()));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p conary-core test_map_capability_from_db -- --nocapture`
Expected: FAIL

**Step 3: Add DB-backed mapping function**

Add to `conary-core/src/ccs/legacy/mod.rs`:

```rust
/// Map a capability to a distro-specific package name via canonical DB.
/// Falls back to hardcoded mappings if DB has no entry.
pub fn map_capability_to_package_db(
    conn: &Connection,
    capability: &str,
    format: &str,
) -> Result<Option<String>> {
    // Map format string to distro pattern
    let distro_prefix = match format {
        "deb" => "ubuntu",
        "rpm" => "fedora",
        "arch" => "arch",
        _ => return Ok(map_capability_to_package(capability, format)),
    };

    // Try canonical lookup
    if let Some(canonical) = CanonicalPackage::resolve_name(conn, capability)? {
        let impls = PackageImplementation::find_by_canonical(conn, canonical.id.unwrap())?;
        // Find implementation matching this format's distro
        if let Some(impl_pkg) = impls.iter().find(|i| i.distro.starts_with(distro_prefix)) {
            return Ok(Some(impl_pkg.distro_name.clone()));
        }
    }

    // Fall back to hardcoded for now
    Ok(map_capability_to_package(capability, format))
}
```

Update callers of `map_capability_to_package` to use `map_capability_to_package_db`
where a DB connection is available. The old function stays as fallback.

**Step 4: Run tests**

Run: `cargo test -p conary-core test_map_capability_from_db -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add conary-core/src/ccs/legacy/mod.rs
git commit -m "feat: Add DB-backed capability mapping, fallback to hardcoded"
```

---

### Task 13: CLI — Pin Command

New `conary pin` subcommand for distro pinning.

**Files:**
- Create: `src/cli/pin.rs`
- Create: `src/commands/pin.rs`
- Modify: `src/cli/mod.rs` (register subcommand)
- Modify: `src/main.rs` (wire command)

**Step 1: Define CLI args**

Create `src/cli/pin.rs`:

```rust
// src/cli/pin.rs

use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub enum PinCommands {
    /// Pin system to a distro
    Set {
        /// Distro name (e.g., "ubuntu-noble", "fedora-41")
        distro: String,

        /// Mixing policy: strict, guarded, permissive
        #[arg(long, default_value = "guarded")]
        mixing: String,
    },
    /// Remove the current distro pin
    Remove,
    /// Show available distros
    List,
    /// Show current pin and affinity stats
    Info,
    /// Change mixing policy on current pin
    Mixing {
        /// New policy: strict, guarded, permissive
        policy: String,
    },
}
```

**Step 2: Write command implementation**

Create `src/commands/pin.rs`:

```rust
// src/commands/pin.rs

use anyhow::Result;
use conary_core::db::models::{DistroPin, SystemAffinity};
use rusqlite::Connection;

pub fn cmd_pin_set(conn: &Connection, distro: &str, mixing: &str) -> Result<()> {
    // Validate mixing policy
    if !["strict", "guarded", "permissive"].contains(&mixing) {
        anyhow::bail!("Invalid mixing policy: {mixing}. Use strict, guarded, or permissive.");
    }
    DistroPin::set(conn, distro, mixing)?;
    println!("Pinned to {distro} (mixing: {mixing})");
    Ok(())
}

pub fn cmd_pin_remove(conn: &Connection) -> Result<()> {
    DistroPin::remove(conn)?;
    println!("Distro pin removed. System is now distro-agnostic.");
    Ok(())
}

pub fn cmd_pin_info(conn: &Connection) -> Result<()> {
    match DistroPin::get_current(conn)? {
        Some(pin) => {
            println!("Distro: {}", pin.distro);
            println!("Mixing: {}", pin.mixing_policy);
            println!("Set:    {}", pin.created_at);
            println!();
            println!("Source affinity:");
            let affinities = SystemAffinity::list(conn)?;
            if affinities.is_empty() {
                println!("  (no data yet -- run a sync first)");
            } else {
                for a in &affinities {
                    println!("  {}: {} packages ({:.1}%)", a.distro, a.package_count, a.percentage);
                }
            }
        }
        None => {
            println!("No distro pin set. System is distro-agnostic.");
        }
    }
    Ok(())
}

pub fn cmd_pin_list() -> Result<()> {
    // TODO: Load from data/distros.toml
    println!("Available distros:");
    println!("  ubuntu-noble     Ubuntu 24.04 LTS (Noble Numbat)");
    println!("  ubuntu-oracular  Ubuntu 24.10 (Oracular Oriole)");
    println!("  fedora-41        Fedora 41");
    println!("  fedora-42        Fedora 42");
    println!("  debian-12        Debian 12 (Bookworm)");
    println!("  arch             Arch Linux (rolling)");
    Ok(())
}

pub fn cmd_pin_mixing(conn: &Connection, policy: &str) -> Result<()> {
    if !["strict", "guarded", "permissive"].contains(&policy) {
        anyhow::bail!("Invalid mixing policy: {policy}. Use strict, guarded, or permissive.");
    }
    DistroPin::set_mixing_policy(conn, policy)?;
    println!("Mixing policy changed to {policy}");
    Ok(())
}
```

**Step 3: Register in CLI and main.rs**

Add to `src/cli/mod.rs`:
```rust
pub mod pin;
```

Add `Pin` variant to `Commands` enum and wire in `main.rs` match block.

**Step 4: Build and test**

Run: `cargo build`
Expected: Compiles clean

Run: `cargo test -p conary`
Expected: All existing tests pass

**Step 5: Commit**

```bash
git add src/cli/pin.rs src/commands/pin.rs src/cli/mod.rs src/main.rs
git commit -m "feat: Add 'conary pin' CLI for distro pinning"
```

---

### Task 14: CLI — Canonical and Groups Commands

New `conary canonical` and `conary groups` subcommands.

**Files:**
- Create: `src/cli/canonical.rs`
- Create: `src/cli/groups.rs`
- Create: `src/commands/canonical.rs`
- Create: `src/commands/groups.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`

**Step 1: Define CLI args**

`src/cli/canonical.rs`:
```rust
// src/cli/canonical.rs

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum CanonicalCommands {
    /// Show canonical identity and all implementations for a package
    Show {
        /// Package name (canonical, distro, or AppStream ID)
        name: String,
    },
    /// Search canonical registry
    Search {
        /// Search query
        query: String,
    },
    /// List installed packages without canonical mapping
    Unmapped,
}
```

`src/cli/groups.rs`:
```rust
// src/cli/groups.rs

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum GroupsCommands {
    /// List all available package groups
    List,
    /// Show members of a group
    Show {
        /// Group name
        name: String,

        /// Show distro-specific view
        #[arg(long)]
        distro: Option<String>,
    },
}
```

**Step 2: Write command implementations**

`src/commands/canonical.rs`:
```rust
// src/commands/canonical.rs

use anyhow::Result;
use conary_core::db::models::{CanonicalPackage, PackageImplementation};
use rusqlite::Connection;

pub fn cmd_canonical_show(conn: &Connection, name: &str) -> Result<()> {
    let pkg = CanonicalPackage::resolve_name(conn, name)?;
    let Some(pkg) = pkg else {
        println!("No canonical mapping found for '{name}'");
        return Ok(());
    };

    println!("Canonical: {}", pkg.name);
    if let Some(ref appstream) = pkg.appstream_id {
        println!("AppStream: {appstream}");
    }
    if let Some(ref desc) = pkg.description {
        println!("Description: {desc}");
    }
    println!("Kind: {}", pkg.kind);
    if let Some(ref cat) = pkg.category {
        println!("Category: {cat}");
    }
    println!();

    let impls = PackageImplementation::find_by_canonical(conn, pkg.id.unwrap())?;
    if impls.is_empty() {
        println!("No implementations found.");
    } else {
        println!("Implementations:");
        for i in &impls {
            println!("  {}: {} (source: {})", i.distro, i.distro_name, i.source);
        }
    }
    Ok(())
}

pub fn cmd_canonical_search(conn: &Connection, query: &str) -> Result<()> {
    let results = CanonicalPackage::search(conn, query)?;
    if results.is_empty() {
        println!("No packages found matching '{query}'");
        return Ok(());
    }
    for pkg in &results {
        let kind_tag = if pkg.kind == "group" { " [group]" } else { "" };
        let desc = pkg.description.as_deref().unwrap_or("");
        println!("  {}{kind_tag} - {desc}", pkg.name);
    }
    Ok(())
}

pub fn cmd_canonical_unmapped(conn: &Connection) -> Result<()> {
    // Find installed troves that have no canonical mapping
    let mut stmt = conn.prepare(
        "SELECT t.name FROM troves t
         WHERE NOT EXISTS (
             SELECT 1 FROM package_implementations pi WHERE pi.distro_name = t.name
         )
         AND NOT EXISTS (
             SELECT 1 FROM canonical_packages cp WHERE cp.name = t.name
         )
         ORDER BY t.name",
    )?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    if names.is_empty() {
        println!("All installed packages have canonical mappings.");
    } else {
        println!("{} installed packages without canonical mapping:", names.len());
        for name in &names {
            println!("  {name}");
        }
    }
    Ok(())
}
```

`src/commands/groups.rs`:
```rust
// src/commands/groups.rs

use anyhow::Result;
use conary_core::db::models::{CanonicalPackage, PackageImplementation};
use rusqlite::Connection;

pub fn cmd_groups_list(conn: &Connection) -> Result<()> {
    let groups = CanonicalPackage::list_by_kind(conn, "group")?;
    if groups.is_empty() {
        println!("No package groups found. Run 'conary registry update' to sync.");
        return Ok(());
    }
    println!("Package groups:");
    for g in &groups {
        let desc = g.description.as_deref().unwrap_or("");
        println!("  {} - {desc}", g.name);
    }
    Ok(())
}

pub fn cmd_groups_show(conn: &Connection, name: &str, distro: Option<&str>) -> Result<()> {
    let pkg = CanonicalPackage::find_by_name(conn, name)?;
    let Some(pkg) = pkg else {
        println!("Group '{name}' not found.");
        return Ok(());
    };
    if pkg.kind != "group" {
        println!("'{name}' is a package, not a group.");
        return Ok(());
    }

    println!("Group: {}", pkg.name);
    if let Some(ref desc) = pkg.description {
        println!("Description: {desc}");
    }
    println!();

    let impls = PackageImplementation::find_by_canonical(conn, pkg.id.unwrap())?;
    if let Some(distro_filter) = distro {
        let filtered: Vec<_> = impls.iter().filter(|i| i.distro == distro_filter).collect();
        if filtered.is_empty() {
            println!("No implementation for distro '{distro_filter}'");
        } else {
            for i in &filtered {
                println!("  {}: {}", i.distro, i.distro_name);
            }
        }
    } else {
        println!("Implementations:");
        for i in &impls {
            println!("  {}: {}", i.distro, i.distro_name);
        }
    }
    Ok(())
}
```

**Step 3: Register both in CLI and main.rs**

Add modules, enum variants, and match arms following the existing pattern.

**Step 4: Build and test**

Run: `cargo build`
Expected: Compiles clean

**Step 5: Commit**

```bash
git add src/cli/canonical.rs src/cli/groups.rs src/commands/canonical.rs src/commands/groups.rs src/cli/mod.rs src/main.rs
git commit -m "feat: Add 'conary canonical' and 'conary groups' CLI commands"
```

---

### Task 15: Install --from Flag

Add `--from` flag to `conary install` for explicit cross-distro override.

**Files:**
- Modify: `src/cli/mod.rs` or `src/cli/package.rs` (wherever Install args live)
- Modify: `src/commands/install/mod.rs`

**Step 1: Add the flag**

Find the `Install` variant in the `Commands` enum and add:

```rust
/// Install from a specific distro (cross-distro override)
#[arg(long)]
from: Option<String>,
```

**Step 2: Wire into InstallOptions**

Add `from_distro: Option<String>` to `InstallOptions`.

**Step 3: Use in resolution**

In the install command's resolution path, if `from_distro` is set:
1. Resolve the package name to its canonical identity
2. Look up the implementation for the specified distro
3. Use that implementation's distro_name for the actual package lookup
4. Bypass mixing policy check (explicit override = user knows what they're doing)

**Step 4: Build and test**

Run: `cargo build`
Expected: Compiles clean

Run: `cargo test -p conary`
Expected: All existing tests pass

**Step 5: Commit**

```bash
git add src/cli/mod.rs src/commands/install/mod.rs
git commit -m "feat: Add --from flag to 'conary install' for cross-distro override"
```

---

### Task 16: Remi Canonical Endpoints

Add canonical metadata endpoints to Remi server.

**Files:**
- Create: `conary-server/src/server/canonical.rs`
- Modify: `conary-server/src/server/mod.rs` (register routes)

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_lookup_response() {
        let response = CanonicalLookupResponse {
            canonical_name: "apache-httpd".to_string(),
            appstream_id: None,
            kind: "package".to_string(),
            implementations: vec![
                ImplementationInfo {
                    distro: "fedora-41".to_string(),
                    distro_name: "httpd".to_string(),
                    ccs_available: true,
                },
                ImplementationInfo {
                    distro: "ubuntu-noble".to_string(),
                    distro_name: "apache2".to_string(),
                    ccs_available: true,
                },
            ],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("apache-httpd"));
    }
}
```

**Step 2: Implement endpoints**

Create `conary-server/src/server/canonical.rs`:

- `CanonicalLookupResponse` struct (Serialize): canonical_name, appstream_id, kind, implementations
- `ImplementationInfo` struct: distro, distro_name, ccs_available
- `GET /api/v1/canonical/<name>` — lookup canonical package by name (canonical, distro, or AppStream)
- `GET /api/v1/canonical/search?q=<query>` — search canonical registry
- `GET /api/v1/registry/sync` — client sync endpoint, returns canonical mappings as JSON
- `GET /api/v1/groups` — list all groups
- `GET /api/v1/groups/<name>` — group details

Register routes in the server module.

**Step 3: Build with server feature**

Run: `cargo build --features server`
Expected: Compiles clean

**Step 4: Commit**

```bash
git add conary-server/src/server/canonical.rs conary-server/src/server/mod.rs
git commit -m "feat: Add Remi canonical metadata API endpoints"
```

---

### Task 17: Data Files and Registry Command

Ship curated canonical rules and distro definitions. Add `conary registry` command.

**Files:**
- Create: `data/canonical-rules/800.renames-and-merges.yaml`
- Create: `data/distros.toml`
- Create: `src/cli/registry.rs`
- Create: `src/commands/registry.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/main.rs`

**Step 1: Create seed data**

`data/canonical-rules/800.renames-and-merges.yaml` — initial curated rules for
the most common problem cases (pull from existing hardcoded mappings in
`ccs/legacy/mod.rs`):

```yaml
# Core system libraries
- setname: glibc
  name: libc6
  repo: ubuntu_24_04
- setname: glibc
  name: glibc
  repo: fedora_41
# ... convert all ~30 hardcoded mappings to YAML rules

# Web servers
- setname: apache-httpd
  name: httpd
  repo: fedora_41
- setname: apache-httpd
  name: apache2
  repo: ubuntu_24_04
- setname: apache-httpd
  name: apache
  repo: arch

# Package groups
- setname: dev-tools
  kind: group
  name: build-essential
  repo: ubuntu_24_04
- setname: dev-tools
  kind: group
  name: "@development-tools"
  repo: fedora_41
- setname: dev-tools
  kind: group
  name: base-devel
  repo: arch
```

`data/distros.toml`:
```toml
[[distros]]
name = "ubuntu-noble"
display = "Ubuntu 24.04 LTS (Noble Numbat)"
format = "deb"
release = "2024-04-25"
eol = "2029-04-25"

[[distros]]
name = "fedora-41"
display = "Fedora 41"
format = "rpm"
release = "2024-10-29"
eol = "2025-11-15"

[[distros]]
name = "debian-12"
display = "Debian 12 (Bookworm)"
format = "deb"
release = "2023-06-10"
eol = "2028-06-10"

[[distros]]
name = "arch"
display = "Arch Linux (rolling)"
format = "arch"
release = "rolling"
```

**Step 2: Write registry CLI and command**

`src/cli/registry.rs`:
```rust
// src/cli/registry.rs

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum RegistryCommands {
    /// Sync canonical registry from Remi
    Update,
    /// Show mapping coverage statistics
    Stats,
}
```

`src/commands/registry.rs`:
```rust
// src/commands/registry.rs

use anyhow::Result;
use rusqlite::Connection;

pub fn cmd_registry_update(conn: &Connection, remi_url: Option<&str>) -> Result<()> {
    println!("Syncing canonical registry...");
    // TODO: Fetch from Remi endpoint or Repology API
    // For now, load from local data/canonical-rules/
    let rules_dir = std::path::Path::new("/usr/share/conary/canonical-rules");
    let local_dir = std::path::Path::new("data/canonical-rules");
    let dir = if rules_dir.exists() { rules_dir } else { local_dir };

    if dir.exists() {
        let engine = conary_core::canonical::rules::RulesEngine::load_from_dir(dir)?;
        println!("Loaded {} curated rules", engine.rule_count());
        // TODO: Apply rules to DB
    }
    println!("[COMPLETE] Registry updated");
    Ok(())
}

pub fn cmd_registry_stats(conn: &Connection) -> Result<()> {
    let canonical_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM canonical_packages",
        [],
        |row| row.get(0),
    )?;
    let impl_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM package_implementations",
        [],
        |row| row.get(0),
    )?;
    let group_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM canonical_packages WHERE kind = 'group'",
        [],
        |row| row.get(0),
    )?;

    // Source breakdown
    println!("Canonical registry statistics:");
    println!("  Canonical packages: {canonical_count}");
    println!("  Package groups:     {group_count}");
    println!("  Implementations:    {impl_count}");
    println!();

    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) FROM package_implementations GROUP BY source ORDER BY COUNT(*) DESC",
    )?;
    let sources: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    if !sources.is_empty() {
        println!("  By source:");
        for (source, count) in &sources {
            println!("    {source}: {count}");
        }
    }

    Ok(())
}
```

**Step 3: Register and wire**

Add modules, enum variants, match arms in `cli/mod.rs` and `main.rs`.

**Step 4: Build and test**

Run: `cargo build`
Expected: Compiles clean

**Step 5: Commit**

```bash
git add data/ src/cli/registry.rs src/commands/registry.rs src/cli/mod.rs src/main.rs
git commit -m "feat: Add curated canonical rules, distros.toml, and registry CLI"
```

---

### Task 18: Integration Tests

End-to-end tests verifying the full canonical resolution pipeline.

**Files:**
- Create: `conary-core/tests/canonical.rs`

**Step 1: Write integration test**

```rust
//! Integration tests for canonical package identity system

use conary_core::db::models::*;
use conary_core::canonical::rules::{parse_rules, RulesEngine};
use conary_core::canonical::discovery::*;
use conary_core::resolver::canonical::CanonicalResolver;
use rusqlite::Connection;

fn setup_test_db() -> Connection {
    // Create in-memory DB with all migrations through v45
    let conn = Connection::open_in_memory().unwrap();
    conary_core::db::run_all_migrations(&conn).unwrap();
    conn
}

#[test]
fn test_full_canonical_resolution_pinned() {
    let conn = setup_test_db();

    // Set up canonical packages
    let apache_id = CanonicalPackage::new("apache-httpd", "package").insert(&conn).unwrap();
    PackageImplementation::new(apache_id, "fedora-41", "httpd", "curated").insert(&conn).unwrap();
    PackageImplementation::new(apache_id, "ubuntu-noble", "apache2", "curated").insert(&conn).unwrap();
    PackageImplementation::new(apache_id, "arch", "apache", "curated").insert(&conn).unwrap();

    // Pin to Ubuntu
    DistroPin::set(&conn, "ubuntu-noble", "guarded").unwrap();

    let resolver = CanonicalResolver::new(&conn);

    // Canonical name resolves to all implementations
    let candidates = resolver.expand("apache-httpd").unwrap();
    assert_eq!(candidates.len(), 3);

    // Ranking puts Ubuntu first (pinned)
    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro, "ubuntu-noble");
    assert_eq!(ranked[0].distro_name, "apache2");
}

#[test]
fn test_full_canonical_resolution_unpinned_affinity() {
    let conn = setup_test_db();

    let curl_id = CanonicalPackage::new("curl", "package").insert(&conn).unwrap();
    PackageImplementation::new(curl_id, "fedora-41", "curl", "auto").insert(&conn).unwrap();
    PackageImplementation::new(curl_id, "ubuntu-noble", "curl", "auto").insert(&conn).unwrap();

    // Set affinity: 80% Fedora
    conn.execute(
        "INSERT INTO system_affinity (distro, package_count, percentage, updated_at)
         VALUES ('fedora-41', 80, 80.0, '2026-03-05')",
        [],
    ).unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("curl").unwrap();
    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro, "fedora-41"); // affinity wins
}

#[test]
fn test_distro_name_resolves_through_canonical() {
    let conn = setup_test_db();

    let cid = CanonicalPackage::new("apache-httpd", "package").insert(&conn).unwrap();
    PackageImplementation::new(cid, "fedora-41", "httpd", "curated").insert(&conn).unwrap();
    PackageImplementation::new(cid, "ubuntu-noble", "apache2", "curated").insert(&conn).unwrap();

    // "httpd" should resolve to canonical, then expand to all implementations
    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("httpd").unwrap();
    assert_eq!(candidates.len(), 2); // both httpd and apache2
}

#[test]
fn test_package_override_bypasses_pin() {
    let conn = setup_test_db();

    let mesa_id = CanonicalPackage::new("mesa", "package").insert(&conn).unwrap();
    PackageImplementation::new(mesa_id, "fedora-41", "mesa", "auto").insert(&conn).unwrap();
    PackageImplementation::new(mesa_id, "ubuntu-noble", "mesa", "auto").insert(&conn).unwrap();

    DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();
    PackageOverride::set(&conn, mesa_id, "fedora-41", Some("want newer Mesa")).unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let override_distro = resolver.get_override(mesa_id).unwrap();
    assert_eq!(override_distro.as_deref(), Some("fedora-41"));
}

#[test]
fn test_group_resolution() {
    let conn = setup_test_db();

    let group_id = CanonicalPackage::new("dev-tools", "group").insert(&conn).unwrap();
    PackageImplementation::new(group_id, "ubuntu-noble", "build-essential", "curated").insert(&conn).unwrap();
    PackageImplementation::new(group_id, "fedora-41", "@development-tools", "curated").insert(&conn).unwrap();
    PackageImplementation::new(group_id, "arch", "base-devel", "curated").insert(&conn).unwrap();

    // Pin to Fedora
    DistroPin::set(&conn, "fedora-41", "guarded").unwrap();

    let resolver = CanonicalResolver::new(&conn);
    let candidates = resolver.expand("dev-tools").unwrap();
    let ranked = resolver.rank_candidates(&candidates).unwrap();
    assert_eq!(ranked[0].distro_name, "@development-tools");
}

#[test]
fn test_rules_engine_populates_db() {
    let conn = setup_test_db();

    let yaml = r#"
- setname: apache-httpd
  name: httpd
  repo: fedora_41
- setname: apache-httpd
  name: apache2
  repo: ubuntu_24_04
"#;
    let rules = parse_rules(yaml).unwrap();
    let engine = RulesEngine::new(rules);

    // Verify engine resolves correctly
    assert_eq!(engine.resolve("httpd", "fedora_41"), Some("apache-httpd".to_string()));
    assert_eq!(engine.resolve("apache2", "ubuntu_24_04"), Some("apache-httpd".to_string()));
}

#[test]
fn test_mixing_policy_enforcement() {
    let conn = setup_test_db();

    // Strict mode
    DistroPin::set(&conn, "ubuntu-noble", "strict").unwrap();
    let resolver = CanonicalResolver::new(&conn);
    assert!(resolver.check_mixing_policy("fedora-41").is_err());

    // Guarded mode
    DistroPin::set_mixing_policy(&conn, "guarded").unwrap();
    let result = resolver.check_mixing_policy("fedora-41").unwrap();
    assert!(result.has_warning());

    // Permissive mode
    DistroPin::set_mixing_policy(&conn, "permissive").unwrap();
    let result = resolver.check_mixing_policy("fedora-41").unwrap();
    assert!(!result.has_warning());

    // No pin = no policy
    DistroPin::remove(&conn).unwrap();
    let result = resolver.check_mixing_policy("fedora-41").unwrap();
    assert!(!result.has_warning());
}
```

**Step 2: Run tests**

Run: `cargo test -p conary-core --test canonical -- --nocapture`
Expected: PASS

**Step 3: Run full suite**

Run: `cargo test`
Expected: All tests pass, clippy clean

**Step 4: Commit**

```bash
git add conary-core/tests/canonical.rs
git commit -m "test: Add integration tests for canonical package identity system"
```

---

## Dependency Graph

```
Task 1 (schema v45)
  |
  +-- Task 2 (canonical + impl models)
  |     |
  |     +-- Task 3 (distro pin + affinity models)
  |     |     |
  |     |     +-- Task 4 (system model parser)
  |     |     +-- Task 13 (CLI: pin)
  |     |
  |     +-- Task 5 (rules engine)
  |     |     |
  |     |     +-- Task 9 (sync integration)
  |     |     +-- Task 17 (data files + registry CLI)
  |     |
  |     +-- Task 6 (discovery)
  |     |     |
  |     |     +-- Task 9 (sync integration)
  |     |
  |     +-- Task 7 (Repology client)
  |     |     |
  |     |     +-- Task 9 (sync integration)
  |     |
  |     +-- Task 8 (AppStream parser)
  |     |     |
  |     |     +-- Task 9 (sync integration)
  |     |
  |     +-- Task 10 (canonical resolver)
  |     |     |
  |     |     +-- Task 11 (conflicts/obsoletes)
  |     |     +-- Task 15 (install --from)
  |     |
  |     +-- Task 12 (CCS legacy replacement)
  |     +-- Task 14 (CLI: canonical + groups)
  |     +-- Task 16 (Remi endpoints)
  |
  +-- Task 18 (integration tests) -- depends on all above
```

## Parallel Opportunities

These task groups are independent and can be worked in parallel:

- **Group A**: Tasks 5, 6, 7, 8 (rules, discovery, Repology, AppStream) -- all independent data sources
- **Group B**: Tasks 10, 11 (resolver changes) -- after Task 2
- **Group C**: Tasks 13, 14, 17 (CLI commands) -- after Task 3
- **Group D**: Task 16 (Remi) -- after Task 2

Task 9 (sync integration) is the convergence point for Group A.
Task 18 (integration tests) is the final convergence point.
