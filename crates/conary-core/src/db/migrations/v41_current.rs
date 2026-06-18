// conary-core/src/db/migrations/v41_current.rs
//! Database migrations v41 through current

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

/// Schema version 60: Add symlink_target column to files table
///
/// Tracks symlink targets so that generation building can include
/// package symlinks in EROFS images (soname links, alternatives, etc.)
/// instead of silently dropping them.
pub fn migrate_v60(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 60");

    conn.execute_batch(
        "
        ALTER TABLE files ADD COLUMN symlink_target TEXT;
        CREATE INDEX idx_files_symlink ON files(id) WHERE symlink_target IS NOT NULL;
        ",
    )?;

    info!("Schema version 60 applied (files.symlink_target)");
    Ok(())
}

/// Version 61: Add state_cas_hashes for GC-safe CAS liveness tracking
///
/// The GC liveness query previously joined files -> troves -> state_members,
/// which broke on package upgrade: trove deletion cascade-deletes file rows,
/// so hashes needed by older surviving generations were lost.
///
/// This table snapshots CAS hashes at state creation time, decoupling GC
/// liveness from the mutable troves/files tables.
pub fn migrate_v61(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 61");
    conn.execute_batch(
        "
        CREATE TABLE state_cas_hashes (
            state_id INTEGER NOT NULL REFERENCES system_states(id) ON DELETE CASCADE,
            sha256_hash TEXT NOT NULL,
            UNIQUE(state_id, sha256_hash)
        );
        CREATE INDEX idx_state_cas_hashes_state ON state_cas_hashes(state_id);
        ",
    )?;

    // Backfill: snapshot hashes from surviving states whose troves still exist.
    // Hashes for states whose troves were already cascade-deleted are irrecoverable;
    // those states are already GC-unsafe and should be pruned.
    conn.execute_batch(
        "
        INSERT OR IGNORE INTO state_cas_hashes (state_id, sha256_hash)
        SELECT sm.state_id, f.sha256_hash
        FROM state_members sm
        JOIN troves t ON t.name = sm.trove_name AND t.version = sm.trove_version
        JOIN files f ON f.trove_id = t.id
        WHERE f.sha256_hash IS NOT NULL
          AND f.sha256_hash != ''
          AND NOT f.sha256_hash LIKE 'adopted-%';
        ",
    )?;

    info!("Schema version 61 applied successfully (state_cas_hashes for GC safety)");
    Ok(())
}

/// Version 62: Add group_id to dependencies for OR-group support
///
/// When a Debian package has `A | B` alternatives, only `A` was recorded
/// because the dependencies table had no way to express OR groups.
/// The nullable `group_id` column links dependency rows that belong to the
/// same OR group: rows sharing the same (trove_id, group_id) are
/// alternatives.  Existing rows get NULL (backward compatible -- they
/// represent simple single-clause dependencies).
pub fn migrate_v62(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 62");

    conn.execute_batch(
        "
        ALTER TABLE dependencies ADD COLUMN group_id INTEGER;
        CREATE INDEX idx_dependencies_group ON dependencies(trove_id, group_id);
        ",
    )?;

    info!("Schema version 62 applied successfully (dependencies.group_id for OR groups)");
    Ok(())
}

/// Version 63: Allow degraded post-install hook status on changesets
///
/// CCS installs now record committed-but-incomplete post-install hook/script
/// failures as `post_hooks_failed` instead of mutating the changeset
/// description. This rebuilds the `changesets` table to extend the status
/// CHECK constraint while preserving data and indexes.
pub fn migrate_v63(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 63");

    conn.execute_batch(
        "
        CREATE TABLE changesets_v63 (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            description TEXT NOT NULL,
            status TEXT NOT NULL CHECK(
                status IN ('pending', 'applied', 'post_hooks_failed', 'rolled_back')
            ),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            applied_at TEXT,
            rolled_back_at TEXT,
            reversed_by_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            tx_uuid TEXT,
            metadata TEXT
        );

        INSERT INTO changesets_v63 (
            id,
            description,
            status,
            created_at,
            applied_at,
            rolled_back_at,
            reversed_by_changeset_id,
            tx_uuid,
            metadata
        )
        SELECT
            id,
            description,
            status,
            created_at,
            applied_at,
            rolled_back_at,
            reversed_by_changeset_id,
            tx_uuid,
            metadata
        FROM changesets;

        DROP INDEX IF EXISTS idx_changesets_status;
        DROP INDEX IF EXISTS idx_changesets_created_at;
        DROP INDEX IF EXISTS idx_changesets_tx_uuid;
        DROP TABLE changesets;
        ALTER TABLE changesets_v63 RENAME TO changesets;

        CREATE INDEX idx_changesets_status ON changesets(status);
        CREATE INDEX idx_changesets_created_at ON changesets(created_at);
        CREATE UNIQUE INDEX idx_changesets_tx_uuid
            ON changesets(tx_uuid) WHERE tx_uuid IS NOT NULL;
        ",
    )?;

    info!("Schema version 63 applied successfully (changesets.post_hooks_failed)");
    Ok(())
}

/// Version 64: Store each generation's /etc merge base in the database
pub fn migrate_v64(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 64");

    conn.execute_batch(
        "
        ALTER TABLE system_states ADD COLUMN base_generation INTEGER;
        ",
    )?;

    info!("Schema version 64 applied successfully (system_states.base_generation)");
    Ok(())
}

/// Version 65: Persist derived build artifact metadata
pub fn migrate_v65(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 65");

    conn.execute_batch(
        "
        ALTER TABLE derived_packages ADD COLUMN last_built_version TEXT;
        ALTER TABLE derived_packages ADD COLUMN last_built_parent_version TEXT;
        ALTER TABLE derived_packages ADD COLUMN build_artifact_hash TEXT;
        ALTER TABLE derived_packages ADD COLUMN build_artifact_path TEXT;
        ALTER TABLE derived_packages ADD COLUMN build_artifact_size INTEGER;
        CREATE INDEX idx_derived_packages_artifact_hash
            ON derived_packages(build_artifact_hash)
            WHERE build_artifact_hash IS NOT NULL;
        ",
    )?;

    info!("Schema version 65 applied successfully (derived build artifact metadata)");
    Ok(())
}

/// Version 66: Automation apply history
pub fn migrate_v66(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 66");

    conn.execute_batch(
        "
        CREATE TABLE automation_history (
            id INTEGER PRIMARY KEY,
            action_id TEXT NOT NULL,
            category TEXT NOT NULL,
            packages TEXT,
            status TEXT NOT NULL,
            error_message TEXT,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX idx_automation_history_applied_at
            ON automation_history(applied_at DESC);
        CREATE INDEX idx_automation_history_category
            ON automation_history(category);
        CREATE INDEX idx_automation_history_status
            ON automation_history(status);
        ",
    )?;

    info!("Schema version 66 applied successfully (automation history)");
    Ok(())
}

/// Version 67: Architecture-aware server-side conversion cache
pub fn migrate_v67(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 67");

    conn.execute_batch(
        "
        ALTER TABLE converted_packages ADD COLUMN package_architecture TEXT;

        CREATE INDEX idx_converted_packages_identity_arch
            ON converted_packages(distro, package_name, package_version, package_architecture);
        ",
    )?;

    info!("Schema version 67 applied successfully (architecture-aware conversions)");
    Ok(())
}

/// Version 68: Repository security-advisory metadata support
pub fn migrate_v68(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 68");

    conn.execute_batch(
        "
        ALTER TABLE repositories
            ADD COLUMN security_advisory_support TEXT NOT NULL DEFAULT 'unknown'
            CHECK(security_advisory_support IN ('unknown', 'unsupported', 'supported'));
        ",
    )?;

    info!("Schema version 68 applied successfully (repository security advisory support)");
    Ok(())
}

/// Version 69: Generation publication debt ledger
pub fn migrate_v69(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 69");

    conn.execute_batch(
        "
        CREATE TABLE generation_publications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trigger_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            published_through_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            tx_uuid TEXT,
            db_path TEXT NOT NULL,
            runtime_root TEXT NOT NULL,
            phase TEXT NOT NULL CHECK(phase IN (
                'pending_build',
                'building',
                'artifact_ready',
                'current_published',
                'active_marked'
            )),
            status TEXT NOT NULL CHECK(status IN (
                'pending',
                'running',
                'failed',
                'complete',
                'abandoned'
            )),
            state_number INTEGER,
            generation_number INTEGER,
            summary TEXT NOT NULL,
            last_error TEXT,
            retry_count INTEGER NOT NULL DEFAULT 0,
            recoverable INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            completed_at TEXT,
            CHECK (
                state_number IS NULL
                OR generation_number IS NULL
                OR state_number = generation_number
            )
        );

        CREATE INDEX idx_generation_publications_status
            ON generation_publications(status);
        CREATE INDEX idx_generation_publications_trigger_changeset
            ON generation_publications(trigger_changeset_id);
        CREATE INDEX idx_generation_publications_generation
            ON generation_publications(generation_number);
        CREATE INDEX idx_generation_publications_recoverable
            ON generation_publications(recoverable, status);
        ",
    )?;

    info!("Schema version 69 applied successfully (generation publication debt ledger)");
    Ok(())
}

/// Version 70: Passive legacy scriptlet bundle metadata for converted packages
pub fn migrate_v70(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 70");

    conn.execute_batch(
        "
        ALTER TABLE converted_packages ADD COLUMN scriptlet_fidelity TEXT NOT NULL DEFAULT 'unknown';
        ALTER TABLE converted_packages ADD COLUMN target_compatibility TEXT NOT NULL DEFAULT 'unknown';
        ALTER TABLE converted_packages ADD COLUMN publication_status TEXT NOT NULL DEFAULT 'public';
        ALTER TABLE converted_packages ADD COLUMN evidence_digest TEXT;
        ALTER TABLE converted_packages ADD COLUMN curation_evidence_digest TEXT;
        ALTER TABLE converted_packages ADD COLUMN blocked_reason_codes_json TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE converted_packages ADD COLUMN scriptlet_summary_json TEXT NOT NULL DEFAULT '{}';
        ALTER TABLE converted_packages ADD COLUMN review_artifact_path TEXT;

        CREATE INDEX idx_converted_packages_scriptlet_fidelity
            ON converted_packages(scriptlet_fidelity);
        CREATE INDEX idx_converted_packages_publication_status
            ON converted_packages(publication_status);
        ",
    )?;

    info!("Schema version 70 applied successfully (passive scriptlet metadata)");
    Ok(())
}

/// Version 71: Installed legacy scriptlet bundle state for safe replay
pub fn migrate_v71(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 71");

    conn.execute_batch(
        "
        CREATE TABLE installed_legacy_scriptlet_bundles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL UNIQUE REFERENCES troves(id) ON DELETE CASCADE,
            source_format TEXT NOT NULL,
            source_family TEXT NOT NULL,
            source_distro TEXT,
            source_release TEXT,
            source_arch TEXT,
            source_package TEXT NOT NULL,
            source_version TEXT NOT NULL,
            target_id TEXT NOT NULL,
            target_compatibility TEXT NOT NULL,
            foreign_replay_policy TEXT NOT NULL,
            scriptlet_fidelity TEXT NOT NULL,
            publication_status TEXT NOT NULL,
            evidence_digest TEXT,
            replay_policy TEXT NOT NULL,
            replay_enabled INTEGER NOT NULL DEFAULT 0,
            bundle_toml TEXT NOT NULL,
            installed_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_installed_legacy_scriptlet_bundles_trove
            ON installed_legacy_scriptlet_bundles(trove_id);

        CREATE INDEX idx_installed_legacy_scriptlet_bundles_evidence
            ON installed_legacy_scriptlet_bundles(evidence_digest);
        ",
    )?;

    info!("Schema version 71 applied successfully (installed legacy scriptlet bundles)");
    Ok(())
}

/// Version 72: Repository package signing keys
pub fn migrate_v72(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 72");

    conn.execute_batch(
        "
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
        ",
    )?;

    info!("Schema version 72 applied successfully (repository package signing keys)");
    Ok(())
}

/// Version 73: Try session persistence
pub fn migrate_v73(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 73");

    conn.execute_batch(
        "
        CREATE TABLE try_sessions (
            id TEXT PRIMARY KEY,
            package_path TEXT NOT NULL,
            package_name TEXT,
            package_version TEXT,
            previous_generation_id INTEGER,
            try_generation_id INTEGER,
            launcher_pid INTEGER,
            launcher_boot_id TEXT,
            status TEXT NOT NULL CHECK (status IN ('active', 'orphaned', 'kept', 'rolled_back')),
            mode TEXT NOT NULL CHECK (mode IN ('namespace', 'activated')),
            open_slot INTEGER NOT NULL DEFAULT 1 CHECK (open_slot = 1),
            work_dir TEXT NOT NULL,
            last_error TEXT,
            started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            completed_at TEXT
        );

        CREATE UNIQUE INDEX idx_try_sessions_single_open
            ON try_sessions(open_slot)
            WHERE status IN ('active', 'orphaned');
        ",
    )?;

    info!("Schema version 73 applied successfully (try session persistence)");
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::migrate;

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

        conn.execute(
            "SELECT package_release FROM repository_packages LIMIT 0",
            [],
        )
        .unwrap();
        conn.execute(
            "SELECT package_release, architecture, status FROM native_package_publications LIMIT 0",
            [],
        )
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
             (repository_id, name, version, package_release, checksum, size, download_url)
             VALUES (?1, 'hello', '1.0.0', '1', 'sha256:hello', 42, '/v1/fedora/packages/hello/download')",
            [repo_id],
        )
        .unwrap();
        let repo_pkg_id = conn.last_insert_rowid();

        let insert_native = || {
            conn.execute(
                "INSERT INTO native_package_publications (
                    repository_id, repository_package_id, distro, name, version, package_release,
                    architecture, package_kind, authority_format_version, status, content_hash,
                    chunk_hashes_json, total_size, package_path, target_path, trust_status
                ) VALUES (?1, ?2, 'test-distro', 'hello', '1.0.0', '1', 'noarch',
                          'package', 2, 'public', 'sha256:hello', '[\"sha256:hello\"]',
                          42, '/tmp/hello.ccs', 'packages/test-distro/hello.ccs',
                          'verified')",
                [repo_id, repo_pkg_id],
            )
        };

        insert_native().unwrap();
        let duplicate = insert_native().unwrap_err();
        assert!(matches!(
            duplicate,
            rusqlite::Error::SqliteFailure(error, _)
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
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
                repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                architecture TEXT,
                checksum TEXT NOT NULL,
                size INTEGER NOT NULL,
                download_url TEXT NOT NULL
            );
            CREATE UNIQUE INDEX idx_repo_packages_unique
                ON repository_packages(repository_id, name, version, architecture);
            INSERT INTO repositories (id, name, url)
                VALUES (1, 'legacy', 'https://legacy.example.test');
            INSERT INTO repository_packages
                (repository_id, name, version, architecture, checksum, size, download_url)
                VALUES
                (1, 'legacy-null-arch', '1.0.0', NULL, 'sha256:first', 1, '/first'),
                (1, 'legacy-null-arch', '1.0.0', NULL, 'sha256:second', 1, '/second');
            ",
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

    #[test]
    fn test_migrate_v63_preserves_changesets_and_allows_post_hooks_failed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE changesets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                description TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('pending', 'applied', 'rolled_back')),
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                applied_at TEXT,
                rolled_back_at TEXT,
                reversed_by_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
                tx_uuid TEXT,
                metadata TEXT
            );
            CREATE INDEX idx_changesets_status ON changesets(status);
            CREATE INDEX idx_changesets_created_at ON changesets(created_at);
            CREATE UNIQUE INDEX idx_changesets_tx_uuid
                ON changesets(tx_uuid) WHERE tx_uuid IS NOT NULL;
            ",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO changesets (description, status, applied_at, tx_uuid)
             VALUES (?1, 'applied', CURRENT_TIMESTAMP, ?2)",
            ("pre-v63 changeset", "tx-123"),
        )
        .unwrap();

        migrate_v63(&conn).unwrap();

        conn.execute(
            "INSERT INTO changesets (description, status, applied_at)
             VALUES (?1, 'post_hooks_failed', CURRENT_TIMESTAMP)",
            ["degraded install"],
        )
        .unwrap();

        let preserved: (String, String) = conn
            .query_row(
                "SELECT description, tx_uuid FROM changesets WHERE tx_uuid = 'tx-123'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(preserved.0, "pre-v63 changeset");
        assert_eq!(preserved.1, "tx-123".to_string());
    }

    #[test]
    fn test_migrate_v64_adds_base_generation_to_system_states() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE system_states (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                state_number INTEGER NOT NULL UNIQUE,
                summary TEXT NOT NULL,
                description TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                changeset_id INTEGER,
                is_active INTEGER NOT NULL DEFAULT 0,
                package_count INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .unwrap();

        migrate_v64(&conn).unwrap();

        conn.execute(
            "INSERT INTO system_states (state_number, summary, is_active, package_count, base_generation)
             VALUES (1, 'baseline', 1, 0, 0)",
            [],
        )
        .unwrap();

        let base_generation: i64 = conn
            .query_row(
                "SELECT base_generation FROM system_states WHERE state_number = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(base_generation, 0);
    }

    #[test]
    fn test_migrate_v66_adds_automation_history_table() {
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

        conn.execute(
            "INSERT INTO automation_history (action_id, category, packages, status)
             VALUES (?1, ?2, ?3, ?4)",
            ("updates:openssl", "updates", "[\"openssl\"]", "applied"),
        )
        .unwrap();

        let row: (String, String, String, String) = conn
            .query_row(
                "SELECT action_id, category, packages, status FROM automation_history",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_eq!(row.0, "updates:openssl");
        assert_eq!(row.1, "updates");
        assert_eq!(row.2, "[\"openssl\"]");
        assert_eq!(row.3, "applied");
    }

    #[test]
    fn test_migrate_v68_adds_repository_security_advisory_support() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO repositories (name, url) VALUES ('security-test', 'https://example.test')",
            [],
        )
        .unwrap();

        let support: String = conn
            .query_row(
                "SELECT security_advisory_support FROM repositories WHERE name = 'security-test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(support, "unknown");
    }

    #[test]
    fn test_migrate_v69_adds_generation_publications_table() {
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

        conn.execute(
            "INSERT INTO changesets (description, status)
             VALUES ('Install fixture', 'applied')",
            [],
        )
        .unwrap();
        let changeset_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO generation_publications (
                trigger_changeset_id, db_path, runtime_root, phase, status, summary
             ) VALUES (?1, ?2, ?3, 'pending_build', 'pending', ?4)",
            (
                changeset_id,
                "/tmp/conary.db",
                "/tmp/conary",
                "Install fixture",
            ),
        )
        .unwrap();

        let phase: String = conn
            .query_row("SELECT phase FROM generation_publications", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(phase, "pending_build");
    }

    #[test]
    fn test_generation_publications_reject_unknown_phase() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let err = conn
            .execute(
                "INSERT INTO generation_publications (
                    db_path, runtime_root, phase, status, summary
                 ) VALUES (?1, ?2, 'current_renamed', 'pending', ?3)",
                ("/tmp/conary.db", "/tmp/conary", "bad phase"),
            )
            .unwrap_err();
        assert!(err.to_string().contains("CHECK"));
    }

    #[test]
    fn test_generation_publications_reject_mismatched_state_and_generation() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let err = conn
            .execute(
                "INSERT INTO generation_publications (
                    db_path, runtime_root, phase, status, state_number, generation_number, summary
                 ) VALUES (?1, ?2, 'artifact_ready', 'running', 1, 2, ?3)",
                ("/tmp/conary.db", "/tmp/conary", "bad state/generation"),
            )
            .unwrap_err();
        assert!(err.to_string().contains("CHECK"));
    }
}
