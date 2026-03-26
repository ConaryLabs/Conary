// conary-core/src/db/migrations/v41_current.rs
//! Database migrations v41 through v57

use crate::error::Result;
use rusqlite::Connection;
use tracing::{debug, info};

/// Schema version 41: Remote collection cache for model includes
///
/// Caches collections fetched from Remi servers for remote model resolution.
/// Entries have a TTL (expires_at) so stale data is refreshed on next fetch.
pub fn migrate_v41(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 41");

    conn.execute_batch(
        "
        CREATE TABLE remote_collections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            label TEXT,
            version TEXT,
            content_hash TEXT NOT NULL,
            data_json TEXT NOT NULL,
            fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            repository_id INTEGER REFERENCES repositories(id),
            UNIQUE(name, label)
        );
        CREATE INDEX idx_remote_collections_expires
            ON remote_collections(expires_at);
        ",
    )?;

    info!("Schema version 41 applied successfully (remote collection cache)");
    Ok(())
}

/// Version 42: Add signature columns to remote_collections
///
/// Stores Ed25519 signatures and signer key IDs for remote collections,
/// enabling cryptographic verification of remote model includes.
pub fn migrate_v42(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 42");

    conn.execute_batch(
        "
        ALTER TABLE remote_collections ADD COLUMN signature BLOB;
        ALTER TABLE remote_collections ADD COLUMN signer_key_id TEXT;
        ",
    )?;

    info!("Schema version 42 applied successfully (collection signatures)");
    Ok(())
}

/// Version 43: TUF (The Update Framework) trust metadata
///
/// Adds tables for TUF supply chain trust:
/// - tuf_roots: Signed root metadata with key/threshold history
/// - tuf_keys: Known TUF keys per repository
/// - tuf_metadata: Current signed metadata for each role
/// - tuf_targets: Target (package) hashes from targets metadata
///
/// Also adds TUF-related columns to repositories table.
pub fn migrate_v43(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 43");

    conn.execute_batch(
        "
        -- TUF root metadata history
        -- Stores every root version for key rotation auditing
        CREATE TABLE tuf_roots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            version INTEGER NOT NULL,
            signed_metadata TEXT NOT NULL,
            spec_version TEXT NOT NULL DEFAULT '1.0.31',
            expires_at TEXT NOT NULL,
            thresholds_json TEXT NOT NULL,
            role_keys_json TEXT NOT NULL,
            verified_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(repository_id, version)
        );
        CREATE INDEX idx_tuf_roots_repo ON tuf_roots(repository_id, version DESC);

        -- TUF keys per repository
        -- Extracted from root metadata for efficient lookup
        CREATE TABLE tuf_keys (
            id TEXT NOT NULL,
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            key_type TEXT NOT NULL,
            public_key TEXT NOT NULL,
            roles_json TEXT NOT NULL,
            from_root_version INTEGER NOT NULL,
            PRIMARY KEY (id, repository_id)
        );
        CREATE INDEX idx_tuf_keys_repo ON tuf_keys(repository_id);

        -- Current TUF metadata per role per repository
        -- Only stores the latest verified version of each role
        CREATE TABLE tuf_metadata (
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            version INTEGER NOT NULL,
            metadata_hash TEXT NOT NULL,
            signed_metadata TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            verified_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (repository_id, role)
        );

        -- TUF targets (package hashes from targets metadata)
        CREATE TABLE tuf_targets (
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            target_path TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            length INTEGER NOT NULL,
            custom_json TEXT,
            targets_version INTEGER NOT NULL,
            PRIMARY KEY (repository_id, target_path)
        );
        CREATE INDEX idx_tuf_targets_repo ON tuf_targets(repository_id);

        -- Add TUF columns to repositories
        ALTER TABLE repositories ADD COLUMN tuf_enabled INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE repositories ADD COLUMN tuf_root_version INTEGER;
        ALTER TABLE repositories ADD COLUMN tuf_root_url TEXT;
        ",
    )?;

    info!("Schema version 43 applied successfully (TUF trust metadata)");
    Ok(())
}

/// Version 44 - Mirror health tracking and delta manifests
///
/// Adds tables for:
/// - mirror_health: Per-mirror latency, throughput, failure tracking, and composite health scores
/// - delta_manifests: Pre-computed delta information between package versions
pub fn migrate_v44(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 44");

    conn.execute_batch(
        "
        -- Mirror health tracking for ranked mirror selection
        CREATE TABLE mirror_health (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            mirror_url TEXT NOT NULL,
            latency_avg_ms INTEGER NOT NULL DEFAULT 0,
            throughput_bps INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            health_score REAL NOT NULL DEFAULT 1.0,
            disabled INTEGER NOT NULL DEFAULT 0,
            geo_hint TEXT,
            last_probed TEXT,
            last_success TEXT,
            UNIQUE(repository_id, mirror_url)
        );
        CREATE INDEX idx_mirror_health_repo ON mirror_health(repository_id);

        -- Pre-computed delta manifests between package versions
        CREATE TABLE delta_manifests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            distro TEXT NOT NULL,
            package_name TEXT NOT NULL,
            from_version TEXT NOT NULL,
            to_version TEXT NOT NULL,
            new_chunks TEXT NOT NULL,
            removed_chunks TEXT NOT NULL,
            download_size INTEGER NOT NULL DEFAULT 0,
            full_size INTEGER NOT NULL DEFAULT 0,
            computed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(distro, package_name, from_version, to_version)
        );
        ",
    )?;

    info!("Schema version 44 applied successfully (mirror health, delta manifests)");
    Ok(())
}

/// Version 45: Canonical package identity system
///
/// Adds tables for cross-distro canonical package mapping:
/// - canonical_packages: Distro-neutral package identities
/// - package_implementations: Distro-specific implementations of canonical packages
/// - distro_pin: System-level distro pinning with mixing policy
/// - package_overrides: Per-package distro source overrides
/// - system_affinity: Computed source affinity tracking
///
/// Also adds columns:
/// - provides.canonical_id: Links provides to canonical packages
/// - repositories.distro: Associates repos with a distro
/// - repository_packages.distro: Associates repo packages with a distro
pub fn migrate_v45(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 45");

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

        -- Add canonical_id to provides for linking to canonical packages
        ALTER TABLE provides ADD COLUMN canonical_id INTEGER REFERENCES canonical_packages(id);

        -- Add distro column to repositories
        ALTER TABLE repositories ADD COLUMN distro TEXT;

        -- Add distro column to repository_packages
        ALTER TABLE repository_packages ADD COLUMN distro TEXT;
        ",
    )?;

    info!("Schema version 45 applied successfully (canonical package identity)");
    Ok(())
}

/// Migration 46: Key-value settings table
/// Version 46 - User runtime settings
///
/// Creates the settings table for user-facing runtime configuration.
/// This is a general-purpose key-value store for CLI settings, distinct from
/// server_metadata and client_metadata to avoid namespace conflicts.
pub fn migrate_v46(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );",
    )?;

    info!("Schema version 46 applied successfully (settings table)");
    Ok(())
}

/// Version 47 - Admin API tokens
///
/// Creates the admin_tokens table for Remi admin API authentication.
pub fn migrate_v47(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS admin_tokens (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            scopes TEXT NOT NULL DEFAULT 'admin',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_admin_tokens_hash ON admin_tokens(token_hash);",
    )?;

    info!("Schema version 47 applied successfully (admin_tokens table)");
    Ok(())
}

/// Version 48 - Admin audit log
///
/// Creates the admin_audit_log table for tracking admin API operations.
pub fn migrate_v48(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS admin_audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL DEFAULT (datetime('now')),
            token_name TEXT,
            action TEXT NOT NULL,
            method TEXT NOT NULL,
            path TEXT NOT NULL,
            status_code INTEGER NOT NULL,
            request_body TEXT,
            response_body TEXT,
            source_ip TEXT,
            duration_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON admin_audit_log(timestamp);
        CREATE INDEX IF NOT EXISTS idx_audit_log_action ON admin_audit_log(action);",
    )?;

    info!("Schema version 48 applied successfully (admin_audit_log table)");
    Ok(())
}

/// Version 49 - Normalized repository capability tables
///
/// Adds first-class normalized tables for repo-native provides and requirements so
/// sync, replatform planning, and later SAT work can query the same substrate.
pub fn migrate_v49(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS repository_provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_package_id INTEGER NOT NULL,
            capability TEXT NOT NULL,
            version TEXT,
            kind TEXT NOT NULL DEFAULT 'package',
            raw TEXT,
            FOREIGN KEY (repository_package_id) REFERENCES repository_packages(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_repository_provides_pkg
            ON repository_provides(repository_package_id);
        CREATE INDEX IF NOT EXISTS idx_repository_provides_capability
            ON repository_provides(capability);
        CREATE INDEX IF NOT EXISTS idx_repository_provides_kind_capability
            ON repository_provides(kind, capability);

        CREATE TABLE IF NOT EXISTS repository_requirements (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_package_id INTEGER NOT NULL,
            capability TEXT NOT NULL,
            version_constraint TEXT,
            kind TEXT NOT NULL DEFAULT 'package',
            dependency_type TEXT NOT NULL DEFAULT 'runtime',
            raw TEXT,
            FOREIGN KEY (repository_package_id) REFERENCES repository_packages(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_repository_requirements_pkg
            ON repository_requirements(repository_package_id);
        CREATE INDEX IF NOT EXISTS idx_repository_requirements_capability
            ON repository_requirements(capability);
        CREATE INDEX IF NOT EXISTS idx_repository_requirements_kind_capability
            ON repository_requirements(kind, capability);
        ",
    )?;

    info!("Schema version 49 applied successfully (normalized repository capabilities)");
    Ok(())
}

/// Version 50 - Installed source identity on troves
///
/// Adds source distro and native version scheme metadata for installed/adopted troves
/// so legacy solver inputs can become scheme-aware without guessing from version strings.
pub fn migrate_v50(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        ALTER TABLE troves ADD COLUMN source_distro TEXT;
        ALTER TABLE troves ADD COLUMN version_scheme TEXT;
        ",
    )?;

    info!("Schema version 50 applied successfully (trove source identity)");
    Ok(())
}

/// Version 51 - Requirement groups, version_scheme on repository_packages and provides
///
/// Adds:
/// - `repository_requirement_groups` table so each OR-alternative group is first-class
/// - `version_scheme` column on `repository_packages` for per-package scheme awareness
/// - `version_scheme` column on `repository_provides` to record the native scheme of
///   the provide version text
pub fn migrate_v51(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS repository_requirement_groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_package_id INTEGER NOT NULL,
            kind TEXT NOT NULL DEFAULT 'depends',
            behavior TEXT NOT NULL DEFAULT 'hard',
            description TEXT,
            native_text TEXT,
            FOREIGN KEY (repository_package_id) REFERENCES repository_packages(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_repo_req_groups_pkg
            ON repository_requirement_groups(repository_package_id);

        ALTER TABLE repository_packages ADD COLUMN version_scheme TEXT;
        ALTER TABLE repository_provides ADD COLUMN version_scheme TEXT;
        ALTER TABLE repository_requirements ADD COLUMN group_id INTEGER REFERENCES repository_requirement_groups(id) ON DELETE CASCADE;
        CREATE INDEX IF NOT EXISTS idx_repo_requirements_group
            ON repository_requirements(group_id);
        ",
    )?;

    info!("Schema version 51 applied successfully (requirement groups, version_scheme columns)");
    Ok(())
}

pub fn migrate_v52(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_provides_trove_cap
            ON provides(trove_id, capability);
        CREATE INDEX IF NOT EXISTS idx_repo_req_pkg_kind
            ON repository_requirements(repository_package_id, kind);
        ",
    )?;

    info!("Schema version 52 applied successfully (composite indexes for resolver and sync)");
    Ok(())
}

/// Version 53 - Canonical cache tables and metadata stores
///
/// Creates three tables:
/// - repology_cache: Package version data from Repology (cross-distro indexing)
/// - appstream_cache: AppStream package metadata (descriptions, summaries)
/// - server_metadata: Server-side sync state (canonical map version, sync timestamps).
///   Separate from settings to avoid namespace conflicts.
/// - client_metadata: Client-side cache state (metadata versions, last-sync timestamps).
///   Separate from settings to avoid namespace conflicts.
pub fn migrate_v53(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS repology_cache (
            project_name TEXT NOT NULL,
            distro TEXT NOT NULL,
            distro_name TEXT NOT NULL,
            version TEXT,
            status TEXT,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (project_name, distro)
        );

        CREATE TABLE IF NOT EXISTS appstream_cache (
            appstream_id TEXT NOT NULL,
            pkgname TEXT NOT NULL,
            display_name TEXT,
            summary TEXT,
            distro TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (appstream_id, distro)
        );

        CREATE TABLE IF NOT EXISTS server_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        INSERT OR IGNORE INTO server_metadata (key, value)
            VALUES ('canonical_map_version', '0');

        CREATE TABLE IF NOT EXISTS client_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;

    info!("Schema version 53 applied successfully (canonical cache tables, metadata)");
    Ok(())
}

/// Version 54: Derivation index for CAS-layered bootstrap build cache.
///
/// Maps content-addressed derivation IDs to their build outputs, enabling
/// build caching: identical inputs produce the same derivation ID, so we
/// can skip the build and reuse the stored output.
pub fn migrate_v54(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS derivation_index (
            derivation_id TEXT PRIMARY KEY,
            output_hash TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            manifest_cas_hash TEXT NOT NULL,
            stage TEXT,
            build_env_hash TEXT,
            built_at TEXT NOT NULL,
            build_duration_secs INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_derivation_index_package
            ON derivation_index(package_name, package_version);

        CREATE INDEX IF NOT EXISTS idx_derivation_index_output_hash
            ON derivation_index(output_hash);
        ",
    )?;

    info!("Schema version 54 applied successfully (derivation index)");
    Ok(())
}

/// Version 55: Substituter peers, derivation cache, and seeds tables.
///
/// substituter_peers: client-side registry of known substituter endpoints with
/// health tracking (success/failure counts, last_seen).
///
/// derivation_cache: server-side index mapping derivation IDs to their cached
/// build outputs (manifest CAS hash) for build result reuse.
///
/// seeds: server-side registry of bootstrapped seed images, indexed by
/// target triple, for fast access during bootstrap pipeline stages.
pub fn migrate_v55(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS substituter_peers (
            endpoint TEXT PRIMARY KEY,
            priority INTEGER NOT NULL DEFAULT 0,
            last_seen TEXT,
            success_count INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS derivation_cache (
            derivation_id TEXT PRIMARY KEY,
            manifest_cas_hash TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_derivation_cache_package
            ON derivation_cache(package_name, package_version);

        CREATE TABLE IF NOT EXISTS seeds (
            seed_id TEXT PRIMARY KEY,
            target_triple TEXT NOT NULL,
            source TEXT NOT NULL,
            builder TEXT,
            packages_json TEXT NOT NULL DEFAULT '[]',
            verified_by_json TEXT NOT NULL DEFAULT '[]',
            image_cas_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_seeds_target
            ON seeds(target_triple, created_at DESC);
        ",
    )?;

    info!("Schema version 55 applied successfully (substituter peers, derivation cache, seeds)");
    Ok(())
}

/// Version 56: Trust levels and provenance on derivations.
///
/// Extends derivation_index with:
/// - trust_level: integer 0-4 representing trust tier (0=unverified, 4=fully-reproduced)
/// - provenance_cas_hash: CAS hash of the provenance record for this derivation
/// - reproducible: nullable boolean tracking whether the derivation has been
///   confirmed reproducible by independent rebuild
///
/// Extends derivation_cache with:
/// - provenance_cas_hash: CAS hash linking cached outputs to their provenance
pub fn migrate_v56(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        ALTER TABLE derivation_index ADD COLUMN trust_level INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE derivation_index ADD COLUMN provenance_cas_hash TEXT;
        ALTER TABLE derivation_index ADD COLUMN reproducible INTEGER;
        ALTER TABLE derivation_cache ADD COLUMN provenance_cas_hash TEXT;
        ",
    )?;

    info!("Schema version 56 applied successfully (trust levels and provenance on derivations)");
    Ok(())
}

/// Migration v57: Output equivalence table
///
/// Creates output_equivalence to store cross-seed output hash equivalences for
/// convergence verification. A row records that a given package, when built
/// under a specific seed, produced a specific output_hash -- enabling comparison
/// across seeds without rebuilding.
pub fn migrate_v57(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS output_equivalence (
            package_name TEXT NOT NULL,
            output_hash TEXT NOT NULL,
            derivation_id TEXT NOT NULL,
            seed_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (package_name, output_hash, seed_id)
        );

        CREATE INDEX IF NOT EXISTS idx_output_equivalence_hash
            ON output_equivalence(output_hash);",
    )?;

    info!("Schema version 57 applied successfully (output_equivalence table)");
    Ok(())
}

/// Version 58: Fix state_members multi-arch + remote_collections NULL label
///
/// - state_members: widen unique constraint from (state_id, trove_name) to
///   (state_id, trove_name, architecture) so multilib installs don't conflict.
/// - remote_collections: convert NULL labels to '' sentinel so the
///   UNIQUE(name, label) constraint correctly prevents duplicate inserts.
pub fn migrate_v58(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 58");

    conn.execute_batch(
        "
        -- Recreate state_members with (state_id, trove_name, architecture) unique
        CREATE TABLE state_members_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            state_id INTEGER NOT NULL REFERENCES system_states(id) ON DELETE CASCADE,
            trove_name TEXT NOT NULL,
            trove_version TEXT NOT NULL,
            architecture TEXT,
            install_reason TEXT NOT NULL DEFAULT 'explicit',
            selection_reason TEXT,
            UNIQUE(state_id, trove_name, architecture)
        );
        INSERT OR IGNORE INTO state_members_new
            (id, state_id, trove_name, trove_version, architecture, install_reason, selection_reason)
            SELECT id, state_id, trove_name, trove_version, architecture, install_reason, selection_reason
            FROM state_members;
        DROP TABLE state_members;
        ALTER TABLE state_members_new RENAME TO state_members;
        CREATE INDEX idx_state_members_state ON state_members(state_id);
        CREATE INDEX idx_state_members_name ON state_members(trove_name);

        -- Add installed_from_repository_id to troves for install provenance
        ALTER TABLE troves ADD COLUMN installed_from_repository_id INTEGER
            REFERENCES repositories(id) ON DELETE SET NULL;

        -- Convert NULL labels to '' sentinel in remote_collections.
        -- The old schema allowed duplicate (name, NULL) rows because SQLite
        -- treats NULL != NULL under UNIQUE. Additionally, the code fix may
        -- have already written '' labels before this migration runs, so we
        -- can have both (name, NULL) and (name, '') rows for the same name.
        --
        -- Strategy: for each name with any unlabeled rows (NULL or ''),
        -- keep only the one with the highest id and delete the rest.
        DELETE FROM remote_collections
        WHERE (label IS NULL OR label = '')
          AND id NOT IN (
              SELECT MAX(id) FROM remote_collections
              WHERE label IS NULL OR label = ''
              GROUP BY name
          );
        UPDATE remote_collections SET label = '' WHERE label IS NULL;
        ",
    )?;

    info!("Schema version 58 applied successfully (multi-arch state_members + label sentinel)");
    Ok(())
}

/// Version 59: Add canonical_id to repository_packages + appstream_provides table
///
/// - canonical_id: FK linking each repo package to its cross-distro canonical
///   identity. Set during sync by looking up package_implementations.
/// - appstream_provides: Cross-distro capability data from AppStream metadata
///   (libraries, binaries, python3 modules, dbus services).
pub fn migrate_v59(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 59");

    conn.execute_batch(
        "
        -- Add canonical_id to repository_packages for cross-distro identity
        ALTER TABLE repository_packages ADD COLUMN canonical_id INTEGER
            REFERENCES canonical_packages(id) ON DELETE SET NULL;
        CREATE INDEX idx_repo_packages_canonical ON repository_packages(canonical_id);

        -- Backfill from existing package_implementations data.
        -- Use COALESCE to handle repos where default_strategy_distro is NULL.
        UPDATE repository_packages SET canonical_id = (
            SELECT pi.canonical_id FROM package_implementations pi
            JOIN repositories r ON repository_packages.repository_id = r.id
            WHERE pi.distro_name = repository_packages.name
              AND pi.distro = COALESCE(r.default_strategy_distro, r.name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

    #[test]
    fn test_migrate_v45_canonical_packages() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        // Verify schema version is current
        let version: i32 = conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, crate::db::schema::SCHEMA_VERSION);

        // Insert into canonical_packages
        conn.execute(
            "INSERT INTO canonical_packages (name, kind, category) VALUES ('firefox', 'package', 'browser')",
            [],
        )
        .unwrap();

        // Insert into package_implementations
        conn.execute(
            "INSERT INTO package_implementations (canonical_id, distro, distro_name, source) VALUES (1, 'fedora', 'firefox', 'auto')",
            [],
        )
        .unwrap();

        // Insert into distro_pin
        conn.execute(
            "INSERT INTO distro_pin (distro, mixing_policy, created_at) VALUES ('fedora', 'guarded', '2026-03-05')",
            [],
        )
        .unwrap();

        // Insert into package_overrides
        conn.execute(
            "INSERT INTO package_overrides (canonical_id, from_distro, reason) VALUES (1, 'ubuntu', 'user preference')",
            [],
        )
        .unwrap();

        // Insert into system_affinity
        conn.execute(
            "INSERT INTO system_affinity (distro, package_count, percentage, updated_at) VALUES ('fedora', 42, 85.5, '2026-03-05')",
            [],
        )
        .unwrap();

        // Verify new columns on provides
        conn.execute("SELECT canonical_id FROM provides LIMIT 0", [])
            .unwrap();

        // Verify new columns on repositories
        conn.execute("SELECT distro FROM repositories LIMIT 0", [])
            .unwrap();

        // Verify new columns on repository_packages
        conn.execute("SELECT distro FROM repository_packages LIMIT 0", [])
            .unwrap();

        // Verify new columns on troves
        conn.execute(
            "SELECT source_distro, version_scheme FROM troves LIMIT 0",
            [],
        )
        .unwrap();

        // Verify v51: repository_requirement_groups table
        conn.execute("SELECT id, repository_package_id, kind, behavior, description, native_text FROM repository_requirement_groups LIMIT 0", [])
            .unwrap();

        // Verify v51: version_scheme on repository_packages
        conn.execute("SELECT version_scheme FROM repository_packages LIMIT 0", [])
            .unwrap();

        // Verify v51: version_scheme on repository_provides
        conn.execute("SELECT version_scheme FROM repository_provides LIMIT 0", [])
            .unwrap();
    }

    #[test]
    fn test_migrate_v53_cache_tables() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        // Verify repology_cache
        conn.execute(
            "INSERT INTO repology_cache (project_name, distro, distro_name, version, status, fetched_at)
             VALUES ('python', 'arch', 'python', '3.12.0', 'newest', '2026-03-19')",
            [],
        ).unwrap();

        // Verify appstream_cache
        conn.execute(
            "INSERT INTO appstream_cache (appstream_id, pkgname, display_name, summary, distro, fetched_at)
             VALUES ('org.mozilla.firefox', 'firefox', 'Firefox', 'Web Browser', 'fedora', '2026-03-19')",
            [],
        ).unwrap();

        // Verify server_metadata seeded
        let version: String = conn
            .query_row(
                "SELECT value FROM server_metadata WHERE key = 'canonical_map_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "0");

        // Verify client_metadata
        conn.execute(
            "INSERT INTO client_metadata (key, value) VALUES ('etag', 'test')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn test_migrate_v59_canonical_id_and_appstream_provides() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        // Verify canonical_id column exists on repository_packages
        conn.execute("SELECT canonical_id FROM repository_packages LIMIT 0", [])
            .unwrap();

        // Verify appstream_provides table exists with FK enforcement
        conn.execute(
            "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
             VALUES (1, 'library', 'libssl.so.3')",
            [],
        )
        .expect_err("should fail FK -- no canonical_packages row");

        // Insert a real canonical package, then appstream_provides
        conn.execute(
            "INSERT INTO canonical_packages (name, kind) VALUES ('openssl', 'package')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO appstream_provides (canonical_id, provide_type, capability)
             VALUES (1, 'library', 'libssl.so.3')",
            [],
        )
        .unwrap();

        // Verify schema version
        let version: i32 = conn
            .query_row(
                "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, crate::db::schema::SCHEMA_VERSION);
    }
}
