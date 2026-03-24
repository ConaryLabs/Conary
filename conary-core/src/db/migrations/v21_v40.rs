// conary-core/src/db/migrations/v21_v40.rs
//! Database migrations v21 through v40

use crate::error::Result;
use rusqlite::Connection;
use tracing::{debug, info};

/// Version 21: Configuration file management
///
/// Tracks configuration files with special handling for upgrades:
/// - Preserves user modifications during package updates
/// - Backs up configs before modification
/// - Enables config diff between installed and package versions
///
/// Creates:
/// - config_files: Track config file status and modifications
/// - config_backups: Store backup copies of configs before changes
pub fn migrate_v21(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 21");

    conn.execute_batch(
        "
        -- Config files table: tracks configuration file status
        -- A config file is any file that:
        -- 1. Is in /etc/ (automatically classified as :config)
        -- 2. Was marked as %config in the package (RPM) or listed in conffiles (DEB)
        CREATE TABLE IF NOT EXISTS config_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            -- Hash of the file as shipped by the package
            original_hash TEXT NOT NULL,
            -- Current hash on filesystem (NULL if not checked)
            current_hash TEXT,
            -- If true, preserve user's version on upgrade (like RPM %config(noreplace))
            noreplace INTEGER NOT NULL DEFAULT 0,
            -- Status: pristine (unchanged), modified (user changed), missing (deleted)
            status TEXT NOT NULL DEFAULT 'pristine',
            -- When the modification was detected
            modified_at TEXT,
            -- Package source that declared this as config (rpm, deb, arch, auto)
            source TEXT DEFAULT 'auto',
            UNIQUE(path)
        );

        CREATE INDEX idx_config_files_path ON config_files(path);
        CREATE INDEX idx_config_files_trove ON config_files(trove_id);
        CREATE INDEX idx_config_files_status ON config_files(status);

        -- Config backups table: stores backup copies before changes
        CREATE TABLE IF NOT EXISTS config_backups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            config_file_id INTEGER NOT NULL REFERENCES config_files(id) ON DELETE CASCADE,
            -- Hash of the backed-up content (stored in CAS)
            backup_hash TEXT NOT NULL,
            -- Reason for backup: upgrade, restore, manual
            reason TEXT NOT NULL,
            -- Changeset that triggered this backup (NULL for manual)
            changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_config_backups_file ON config_backups(config_file_id);
        CREATE INDEX idx_config_backups_changeset ON config_backups(changeset_id);
        ",
    )?;

    // Populate config_files from existing files in /etc/
    // Any file under /etc is automatically considered a config file
    let config_count = conn.execute(
        "INSERT INTO config_files (file_id, path, trove_id, original_hash, current_hash, status, source)
         SELECT f.id, f.path, f.trove_id, f.sha256_hash, f.sha256_hash, 'pristine', 'auto'
         FROM files f
         WHERE f.path LIKE '/etc/%'
         AND NOT EXISTS (SELECT 1 FROM config_files cf WHERE cf.path = f.path)",
        [],
    )?;

    if config_count > 0 {
        info!("Migrated {} existing config files from /etc/", config_count);
    }

    info!("Schema version 21 applied successfully (configuration management)");
    Ok(())
}

/// Version 22: Update improvements - security metadata
///
/// Adds security update tracking to repository packages for critical update filtering.
pub fn migrate_v22(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 22");

    conn.execute_batch(
        "
        -- Add security update columns to repository_packages
        ALTER TABLE repository_packages ADD COLUMN is_security_update INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE repository_packages ADD COLUMN severity TEXT;
        ALTER TABLE repository_packages ADD COLUMN cve_ids TEXT;
        ALTER TABLE repository_packages ADD COLUMN advisory_id TEXT;
        ALTER TABLE repository_packages ADD COLUMN advisory_url TEXT;

        -- Index for filtering security updates
        CREATE INDEX idx_repo_packages_security ON repository_packages(is_security_update) WHERE is_security_update = 1;
        ",
    )?;

    info!("Schema version 22 applied successfully (update improvements)");
    Ok(())
}

/// Version 23: Transaction engine support
///
/// Adds tx_uuid column to changesets for crash recovery correlation.
/// The transaction engine uses UUIDs to correlate journal records with
/// database changesets, enabling recovery after crashes.
pub fn migrate_v23(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 23");

    conn.execute_batch(
        "
        -- Add transaction UUID for crash recovery correlation
        -- NULL for legacy changesets created before transaction engine
        ALTER TABLE changesets ADD COLUMN tx_uuid TEXT;

        -- Index for fast lookup during recovery
        CREATE UNIQUE INDEX idx_changesets_tx_uuid ON changesets(tx_uuid) WHERE tx_uuid IS NOT NULL;
        ",
    )?;

    info!("Schema version 23 applied successfully (transaction engine support)");
    Ok(())
}

/// Version 24: Reference mirrors for split metadata/content sources
///
/// Adds content_url column to repositories table to support "reference mirrors":
/// - url: Metadata source (trusted, signed repository metadata)
/// - content_url: Content source (local cache, peer CDN, cheaper bandwidth)
///
/// This enables scenarios like:
/// - Central metadata server with local content mirrors
/// - Peer-to-peer content distribution while maintaining trusted metadata
/// - Different content sources for different network zones
pub fn migrate_v24(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 24");

    conn.execute_batch(
        "
        -- Add content_url for reference mirrors
        -- NULL means use the same URL for both metadata and content
        ALTER TABLE repositories ADD COLUMN content_url TEXT;
        ",
    )?;

    info!("Schema version 24 applied successfully (reference mirrors)");
    Ok(())
}

/// Version 25: Derived packages
///
/// Derived packages allow creating custom versions of existing packages without
/// rebuilding from source. This enables enterprise customization such as:
/// - Custom configuration files (e.g., corporate nginx.conf)
/// - Security patches applied before upstream releases
/// - Monitoring/logging instrumentation
/// - Branding modifications
///
/// Creates:
/// - derived_packages: Track derived package definitions and build status
/// - derived_patches: Ordered list of patches to apply
/// - derived_overrides: File overrides (replace or remove specific files)
pub fn migrate_v25(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 25");

    conn.execute_batch(
        "
        -- Derived packages: Custom packages based on existing ones
        CREATE TABLE derived_packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            -- Name of the derived package (must be unique)
            name TEXT NOT NULL UNIQUE,
            -- Parent trove reference (may be NULL if parent not installed)
            parent_trove_id INTEGER REFERENCES troves(id) ON DELETE SET NULL,
            -- Parent package name (always stored for resolution)
            parent_name TEXT NOT NULL,
            -- Parent version constraint (NULL = track latest)
            parent_version TEXT,
            -- Version policy: inherit (same as parent), suffix (+custom), specific
            version_policy TEXT NOT NULL DEFAULT 'inherit',
            -- Version suffix for suffix policy (e.g., '+custom1')
            version_suffix TEXT,
            -- Specific version for specific policy
            specific_version TEXT,
            description TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            -- Status: pending (not built), built, stale (parent updated), error
            status TEXT NOT NULL DEFAULT 'pending',
            -- Built trove ID (when status = built)
            built_trove_id INTEGER REFERENCES troves(id) ON DELETE SET NULL,
            -- Model file this came from (NULL if created via CLI)
            model_source TEXT,
            -- Error message if status = error
            error_message TEXT
        );

        CREATE INDEX idx_derived_packages_parent ON derived_packages(parent_name);
        CREATE INDEX idx_derived_packages_status ON derived_packages(status);
        CREATE INDEX idx_derived_packages_built ON derived_packages(built_trove_id);

        -- Derived package patches: ordered patch files to apply
        CREATE TABLE derived_patches (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            derived_id INTEGER NOT NULL REFERENCES derived_packages(id) ON DELETE CASCADE,
            -- Order of patch application (1, 2, 3...)
            patch_order INTEGER NOT NULL,
            -- Human-readable patch name
            patch_name TEXT NOT NULL,
            -- Patch content hash (stored in CAS)
            patch_hash TEXT NOT NULL,
            -- Strip level for patch application (default -p1)
            strip_level INTEGER NOT NULL DEFAULT 1,
            UNIQUE(derived_id, patch_order)
        );

        CREATE INDEX idx_derived_patches_derived ON derived_patches(derived_id);

        -- Derived package file overrides: replace or remove specific files
        CREATE TABLE derived_overrides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            derived_id INTEGER NOT NULL REFERENCES derived_packages(id) ON DELETE CASCADE,
            -- Target path in the package to override
            target_path TEXT NOT NULL,
            -- Source content hash (stored in CAS); NULL means remove the file
            source_hash TEXT,
            -- Original source path (for reference in model file)
            source_path TEXT,
            -- Permissions override (NULL = inherit from parent)
            permissions INTEGER,
            UNIQUE(derived_id, target_path)
        );

        CREATE INDEX idx_derived_overrides_derived ON derived_overrides(derived_id);
        CREATE INDEX idx_derived_overrides_path ON derived_overrides(target_path);
        ",
    )?;

    info!("Schema version 25 applied successfully (derived packages)");
    Ok(())
}

/// Version 26: Converted packages tracking
///
/// Tracks packages converted from legacy formats (RPM/DEB/Arch) to CCS format.
/// This enables:
/// - Skip re-conversion of same package artifact (checksum-based dedup)
/// - Track conversion fidelity for debugging and user warnings
/// - Store detected hooks extracted from scriptlets
/// - Re-convert when conversion algorithm is upgraded
///
/// Creates:
/// - converted_packages: Track conversion metadata and fidelity
pub fn migrate_v26(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 26");

    conn.execute_batch(
        "
        -- Converted packages: Track packages converted from legacy formats to CCS
        CREATE TABLE converted_packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            -- Reference to the converted trove (CCS package that was installed)
            trove_id INTEGER REFERENCES troves(id) ON DELETE CASCADE,
            -- Original package format (rpm, deb, arch)
            original_format TEXT NOT NULL,
            -- Checksum of original package file (skip if already converted)
            original_checksum TEXT NOT NULL,
            -- Conversion algorithm version (re-convert if upgraded)
            conversion_version INTEGER NOT NULL DEFAULT 1,
            -- Fidelity level achieved (full, high, partial, low)
            conversion_fidelity TEXT NOT NULL,
            -- JSON of extracted hooks and fidelity details
            detected_hooks TEXT,
            -- When the conversion occurred
            converted_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            -- Unique on checksum to prevent duplicate conversions
            UNIQUE(original_checksum)
        );

        CREATE INDEX idx_converted_packages_trove ON converted_packages(trove_id);
        CREATE INDEX idx_converted_packages_format ON converted_packages(original_format);
        CREATE INDEX idx_converted_packages_checksum ON converted_packages(original_checksum);
        CREATE INDEX idx_converted_packages_fidelity ON converted_packages(conversion_fidelity);
        ",
    )?;

    info!("Schema version 26 applied successfully (converted packages)");
    Ok(())
}

/// Version 27: Chunk access tracking for LRU cache management
///
/// Tracks chunk access patterns for smarter cache eviction and analytics.
/// This complements the filesystem-based LRU tracking with persistent metadata.
///
/// Creates:
/// - chunk_access: Track chunk access patterns and popularity
pub fn migrate_v27(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 27");

    conn.execute_batch(
        "
        -- Chunk access tracking for LRU cache management
        CREATE TABLE chunk_access (
            -- SHA-256 hash of the chunk (primary key)
            hash TEXT PRIMARY KEY NOT NULL,
            -- Size of the chunk in bytes
            size_bytes INTEGER NOT NULL,
            -- Number of times this chunk has been accessed
            access_count INTEGER NOT NULL DEFAULT 1,
            -- When this chunk was first stored
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            -- When this chunk was last accessed
            last_accessed TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            -- Which packages reference this chunk (JSON array of package names)
            referenced_by TEXT,
            -- Whether this chunk is protected from eviction
            protected INTEGER NOT NULL DEFAULT 0
        );

        -- Index for LRU queries (oldest accessed first)
        CREATE INDEX idx_chunk_access_lru ON chunk_access(last_accessed ASC);
        -- Index for finding large chunks
        CREATE INDEX idx_chunk_access_size ON chunk_access(size_bytes DESC);
        -- Index for finding popular chunks
        CREATE INDEX idx_chunk_access_count ON chunk_access(access_count DESC);
        -- Index for protected chunks
        CREATE INDEX idx_chunk_access_protected ON chunk_access(protected) WHERE protected = 1;
        ",
    )?;

    info!("Schema version 27 applied successfully (chunk access tracking)");
    Ok(())
}

/// Version 28: Package redirects
///
/// Redirects allow package names to be aliased or superseded by other packages.
/// This enables clean handling of:
/// - Package renames (old-name → new-name)
/// - Package obsoletes (deprecated-pkg → replacement-pkg)
/// - Package merges (pkg-a, pkg-b → combined-pkg)
/// - Package splits (monolith-pkg → pkg-core, pkg-extras)
///
/// Creates:
/// - redirects: Store redirect mappings from source to target packages
pub fn migrate_v28(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 28");

    conn.execute_batch(
        "
        -- Package redirects: Alias or supersede package names
        CREATE TABLE redirects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            -- Source package name (the name being redirected FROM)
            source_name TEXT NOT NULL,
            -- Source version constraint (NULL = all versions redirect)
            source_version TEXT,
            -- Target package name (the name being redirected TO)
            target_name TEXT NOT NULL,
            -- Target version constraint (NULL = use latest)
            target_version TEXT,
            -- Type of redirect: rename, obsolete, merge, split
            redirect_type TEXT NOT NULL CHECK(redirect_type IN ('rename', 'obsolete', 'merge', 'split')),
            -- Optional user-facing message explaining the redirect
            message TEXT,
            -- When the redirect was created
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            -- Unique constraint: one redirect per source name/version combo
            UNIQUE(source_name, source_version)
        );

        CREATE INDEX idx_redirects_source ON redirects(source_name);
        CREATE INDEX idx_redirects_target ON redirects(target_name);
        CREATE INDEX idx_redirects_type ON redirects(redirect_type);
        ",
    )?;

    info!("Schema version 28 applied successfully (package redirects)");
    Ok(())
}

/// Version 29: Package resolution routing table
///
/// Transforms repositories from package storage into routing layers that direct
/// resolution to the appropriate source per-package. This enables:
/// - Per-package routing (binary cache, Remi conversion, recipe build, delegation)
/// - Unified resolution across different package sources
/// - Tiered caching policies (popular packages cached longer)
/// - Federation support for label-based delegation
///
/// Creates:
/// - package_resolution: Routing table with per-package resolution strategies
pub fn migrate_v29(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 29");

    conn.execute_batch(
        "
        -- Package resolution routing table: per-package resolution strategies
        -- When a package is requested, this table determines how to obtain it
        CREATE TABLE package_resolution (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            -- Which repository this routing entry belongs to
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            -- Package name to match
            name TEXT NOT NULL,
            -- Version constraint (NULL = any version matches)
            version TEXT,

            -- Resolution strategies as JSON array of ResolutionStrategy
            -- Tried in order until one succeeds
            strategies TEXT NOT NULL,
            -- Primary strategy for indexing: 'binary', 'remi', 'recipe', 'delegate', 'legacy'
            primary_strategy TEXT NOT NULL,

            -- Caching policy
            -- TTL in seconds (NULL = don't cache, use repository default)
            cache_ttl INTEGER,
            -- Higher priority = cached longer, lower priority for eviction
            cache_priority INTEGER NOT NULL DEFAULT 0,

            UNIQUE(repository_id, name, version)
        );

        -- Index for fast strategy-based filtering (e.g., find all Remi packages)
        CREATE INDEX idx_resolution_strategy ON package_resolution(repository_id, primary_strategy);
        -- Index for package name lookup within a repository
        CREATE INDEX idx_resolution_name ON package_resolution(repository_id, name);
        -- Index for cache priority (for eviction decisions)
        CREATE INDEX idx_resolution_cache_priority ON package_resolution(cache_priority DESC);
        ",
    )?;

    info!("Schema version 29 applied successfully (package resolution routing)");
    Ok(())
}

/// Version 30: Label federation support
///
/// Adds columns to the labels table to support:
/// - Linking labels to repositories (for resolution)
/// - Delegation chains (label A delegates to label B)
///
/// This enables the federation feature where packages can be resolved
/// through label chains, e.g., `local@devel:main` delegates to `fedora@f41:stable`
pub fn migrate_v30(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 30");

    conn.execute_batch(
        "
        -- Add repository link: which repository to resolve packages from for this label
        ALTER TABLE labels ADD COLUMN repository_id INTEGER REFERENCES repositories(id);

        -- Add delegation target: when resolving through this label, delegate to another
        ALTER TABLE labels ADD COLUMN delegate_to_label_id INTEGER REFERENCES labels(id);

        -- Index for finding labels by repository
        CREATE INDEX idx_labels_repository ON labels(repository_id) WHERE repository_id IS NOT NULL;

        -- Index for finding delegation targets
        CREATE INDEX idx_labels_delegate ON labels(delegate_to_label_id) WHERE delegate_to_label_id IS NOT NULL;
        ",
    )?;

    info!("Schema version 30 applied successfully (label federation)");
    Ok(())
}

/// Version 31: Repository default resolution strategy
///
/// Adds columns to repositories table for default resolution strategy.
/// When no per-package routing entry exists in `package_resolution`,
/// the resolver uses the repository's default strategy.
///
/// This enables seamless Remi integration: add a repo with `--default-strategy=remi`
/// and all packages from that repo automatically use Remi for conversion.
pub fn migrate_v31(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 31");

    conn.execute_batch(
        "
        -- Default resolution strategy for packages without explicit routing entries
        -- Values: 'binary', 'remi', 'recipe', 'delegate', 'legacy', NULL
        -- NULL means no default (use per-package routing or legacy fallback)
        ALTER TABLE repositories ADD COLUMN default_strategy TEXT;

        -- For 'remi' strategy: the Remi server endpoint URL
        ALTER TABLE repositories ADD COLUMN default_strategy_endpoint TEXT;

        -- For 'remi' strategy: the distribution name (fedora, arch, debian, etc.)
        ALTER TABLE repositories ADD COLUMN default_strategy_distro TEXT;
        ",
    )?;

    info!("Schema version 31 applied successfully (repository default strategy)");
    Ok(())
}

/// Version 32: Package DNA / Full Provenance Tracking
///
/// Extends the provenance system to support complete package lineage (Package DNA):
/// - Source layer: upstream URL, hash, git commit, patches
/// - Build layer: recipe hash, build deps with their DNA hashes, environment
/// - Signature layer: builder and reviewer signatures, transparency logs
/// - Content layer: merkle root, component hashes
///
/// The `dna_hash` is a unique identifier computed from all layers, enabling:
/// - Full lineage queries ("what went into this binary?")
/// - Reproducibility verification ("do independent builds match?")
/// - Trust chains ("who vouches for this package?")
pub fn migrate_v32(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 32");

    conn.execute_batch(
        "
        -- Extend provenance table with full DNA tracking
        -- Source layer
        ALTER TABLE provenance ADD COLUMN upstream_url TEXT;
        ALTER TABLE provenance ADD COLUMN upstream_hash TEXT;
        ALTER TABLE provenance ADD COLUMN git_repo TEXT;
        ALTER TABLE provenance ADD COLUMN git_tag TEXT;
        ALTER TABLE provenance ADD COLUMN fetch_timestamp TEXT;
        ALTER TABLE provenance ADD COLUMN patches_json TEXT;

        -- Build layer
        ALTER TABLE provenance ADD COLUMN recipe_hash TEXT;
        ALTER TABLE provenance ADD COLUMN build_deps_json TEXT;
        ALTER TABLE provenance ADD COLUMN host_arch TEXT;
        ALTER TABLE provenance ADD COLUMN host_kernel TEXT;
        ALTER TABLE provenance ADD COLUMN host_distro TEXT;
        ALTER TABLE provenance ADD COLUMN build_start TEXT;
        ALTER TABLE provenance ADD COLUMN build_end TEXT;
        ALTER TABLE provenance ADD COLUMN build_log_hash TEXT;
        ALTER TABLE provenance ADD COLUMN isolation_level TEXT;
        ALTER TABLE provenance ADD COLUMN reproducibility_json TEXT;

        -- Signature layer
        ALTER TABLE provenance ADD COLUMN signatures_json TEXT;
        ALTER TABLE provenance ADD COLUMN rekor_log_index INTEGER;
        ALTER TABLE provenance ADD COLUMN sbom_spdx_hash TEXT;
        ALTER TABLE provenance ADD COLUMN sbom_cyclonedx_hash TEXT;

        -- Content layer
        ALTER TABLE provenance ADD COLUMN merkle_root TEXT;
        ALTER TABLE provenance ADD COLUMN component_hashes_json TEXT;
        ALTER TABLE provenance ADD COLUMN chunk_manifest_json TEXT;
        ALTER TABLE provenance ADD COLUMN total_size INTEGER;
        ALTER TABLE provenance ADD COLUMN file_count INTEGER;

        -- DNA hash - unique identifier for complete provenance chain
        -- Note: UNIQUE constraint enforced via unique index instead of column constraint
        -- (SQLite doesn't support UNIQUE on ALTER TABLE ADD COLUMN)
        ALTER TABLE provenance ADD COLUMN dna_hash TEXT;

        -- Index for DNA hash lookups (unique index enforces uniqueness)
        CREATE UNIQUE INDEX idx_provenance_dna ON provenance(dna_hash) WHERE dna_hash IS NOT NULL;

        -- Index for finding packages built with specific dependency DNA
        -- (useful for 'what else uses this vulnerable dependency' queries)
        CREATE INDEX idx_provenance_deps ON provenance(build_deps_json) WHERE build_deps_json IS NOT NULL;

        -- Index for Rekor log lookups
        CREATE INDEX idx_provenance_rekor ON provenance(rekor_log_index) WHERE rekor_log_index IS NOT NULL;

        -- Table for tracking provenance verification events
        CREATE TABLE provenance_verifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provenance_id INTEGER NOT NULL REFERENCES provenance(id) ON DELETE CASCADE,
            verified_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            verifier_id TEXT NOT NULL,
            matches_expected BOOLEAN NOT NULL,
            details TEXT
        );

        CREATE INDEX idx_prov_verify_prov ON provenance_verifications(provenance_id);
        CREATE INDEX idx_prov_verify_verifier ON provenance_verifications(verifier_id);
        ",
    )?;

    info!("Schema version 32 applied successfully (Package DNA / Full Provenance)");
    Ok(())
}

/// Version 33: Capability Declarations
///
/// Adds tables for tracking package capability declarations:
/// - What network access does a package need?
/// - What filesystem paths does it access?
/// - What syscalls does it use?
///
/// This enables:
/// - Documentation of package security requirements
/// - Audit mode to compare declared vs observed behavior
/// - Future enforcement via landlock/seccomp
///
/// Creates:
/// - capabilities: Store capability declarations as JSON
/// - capability_audits: Track audit results
pub fn migrate_v33(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 33");

    conn.execute_batch(
        "
        -- Capability declarations for packages
        -- Stores JSON-encoded CapabilityDeclaration
        CREATE TABLE capabilities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            -- Reference to the trove (one declaration per package)
            trove_id INTEGER UNIQUE REFERENCES troves(id) ON DELETE CASCADE,
            -- JSON-encoded CapabilityDeclaration
            declaration_json TEXT NOT NULL,
            -- Version of the declaration schema
            declaration_version INTEGER DEFAULT 1,
            -- When the declaration was stored
            declared_at TEXT DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_capabilities_trove ON capabilities(trove_id);

        -- Capability audit results
        -- Tracks results of comparing declared vs observed capabilities
        CREATE TABLE capability_audits (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            -- Reference to the trove being audited
            trove_id INTEGER REFERENCES troves(id) ON DELETE CASCADE,
            -- Audit status: compliant, over_privileged, under_utilized
            status TEXT NOT NULL,
            -- JSON array of violations/observations
            violations_json TEXT,
            -- When the audit was performed
            audited_at TEXT DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_capability_audits_trove ON capability_audits(trove_id);
        CREATE INDEX idx_capability_audits_status ON capability_audits(status);
        ",
    )?;

    info!("Schema version 33 applied successfully (capability declarations)");
    Ok(())
}

/// Version 34: Federation peer and stats tracking
///
/// Adds tables for CAS federation:
/// - federation_peers: Known federation peers with latency/success tracking
/// - federation_stats: Daily statistics for bandwidth savings
///
/// This enables:
/// - Cross-machine CAS deduplication
/// - Peer selection based on performance
/// - Bandwidth savings monitoring
pub fn migrate_v34(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 34");

    conn.execute_batch(
        "
        -- Known federation peers
        CREATE TABLE federation_peers (
            id TEXT PRIMARY KEY NOT NULL,          -- Peer ID (SHA-256 of endpoint)
            endpoint TEXT NOT NULL UNIQUE,         -- HTTP(S) URL
            node_name TEXT,                        -- Human-friendly name
            tier TEXT NOT NULL DEFAULT 'leaf',     -- 'region_hub', 'cell_hub', 'leaf'
            first_seen TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_seen TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            latency_ms INTEGER NOT NULL DEFAULT 0,
            success_count INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            is_enabled INTEGER NOT NULL DEFAULT 1
        );

        CREATE INDEX idx_federation_peers_tier ON federation_peers(tier);
        CREATE INDEX idx_federation_peers_latency ON federation_peers(latency_ms);
        CREATE INDEX idx_federation_peers_enabled ON federation_peers(is_enabled) WHERE is_enabled = 1;

        -- Daily federation statistics
        CREATE TABLE federation_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            date TEXT NOT NULL UNIQUE,             -- YYYY-MM-DD
            bytes_from_peers INTEGER NOT NULL DEFAULT 0,
            bytes_from_upstream INTEGER NOT NULL DEFAULT 0,
            chunks_from_peers INTEGER NOT NULL DEFAULT 0,
            chunks_from_upstream INTEGER NOT NULL DEFAULT 0,
            requests_coalesced INTEGER NOT NULL DEFAULT 0,
            circuit_breaker_trips INTEGER NOT NULL DEFAULT 0,
            peer_count INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX idx_federation_stats_date ON federation_stats(date DESC);
        ",
    )?;

    info!("Schema version 34 applied successfully (federation peers and stats)");
    Ok(())
}

/// Version 35: Daemon job persistence
///
/// Adds the daemon_jobs table for persisting job state across daemon restarts.
/// Jobs are identified by UUID and have an optional idempotency key for deduplication.
pub fn migrate_v35(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 35");

    conn.execute_batch(
        "
        -- Daemon jobs table
        -- Persists job state across daemon restarts
        CREATE TABLE daemon_jobs (
            id TEXT PRIMARY KEY NOT NULL,           -- UUID job identifier
            idempotency_key TEXT UNIQUE,            -- Client-provided key for deduplication
            kind TEXT NOT NULL,                     -- install/remove/update/dry_run/rollback/verify/gc
            spec_json TEXT NOT NULL,                -- Serialized operation specification
            status TEXT NOT NULL DEFAULT 'queued',  -- queued/running/completed/failed/cancelled
            result_json TEXT,                       -- Serialized result (if completed)
            error_json TEXT,                        -- RFC7807 error (if failed)
            requested_by_uid INTEGER,               -- UID of requesting user
            client_info TEXT,                       -- Peer creds, socket path
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            started_at TEXT,
            completed_at TEXT
        );

        CREATE INDEX idx_daemon_jobs_status ON daemon_jobs(status);
        CREATE INDEX idx_daemon_jobs_created ON daemon_jobs(created_at DESC);
        CREATE INDEX idx_daemon_jobs_idempotency ON daemon_jobs(idempotency_key) WHERE idempotency_key IS NOT NULL;
        ",
    )?;

    info!("Schema version 35 applied successfully (daemon jobs)");
    Ok(())
}

/// Version 36: Enhancement framework for converted packages
///
/// Extends the converted_packages table to support retroactive enhancement:
/// - enhancement_version: Track which enhancement version has been applied
/// - inferred_caps_json: Store raw inference results for audit trail
/// - extracted_provenance_json: Store extracted provenance before DB insertion
/// - enhancement_status: Track enhancement progress (pending/in_progress/complete/failed)
///
/// Also creates subpackage_relationships table to track RPM/DEB subpackage
/// relationships (e.g., nginx-devel is a subpackage of nginx).
///
/// This enables:
/// - Retroactive enhancement of already-installed converted packages
/// - Re-enhancement when inference algorithms improve
/// - Audit trail of what was inferred vs declared
/// - Future component merging for subpackages
pub fn migrate_v36(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 36");

    conn.execute_batch(
        "
        -- Extend converted_packages table for enhancement tracking
        -- Enhancement version: which version of the enhancement algorithm was applied
        -- 0 = no enhancement applied yet
        ALTER TABLE converted_packages ADD COLUMN enhancement_version INTEGER DEFAULT 0;

        -- Store raw inference results for audit trail
        -- This preserves what was inferred even if the capability declaration changes
        ALTER TABLE converted_packages ADD COLUMN inferred_caps_json TEXT;

        -- Store extracted provenance before it's written to provenance table
        -- Useful for debugging and understanding conversion fidelity
        ALTER TABLE converted_packages ADD COLUMN extracted_provenance_json TEXT;

        -- Enhancement status tracking
        -- pending: needs enhancement
        -- in_progress: enhancement running
        -- complete: enhancement finished successfully
        -- failed: enhancement failed (check error_message)
        -- skipped: enhancement skipped (e.g., no binaries to analyze)
        ALTER TABLE converted_packages ADD COLUMN enhancement_status TEXT DEFAULT 'pending';

        -- Error message if enhancement failed
        ALTER TABLE converted_packages ADD COLUMN enhancement_error TEXT;

        -- When enhancement was last attempted
        ALTER TABLE converted_packages ADD COLUMN enhancement_attempted_at TEXT;

        -- Index for finding packages needing enhancement
        CREATE INDEX idx_converted_enhancement_status ON converted_packages(enhancement_status);
        CREATE INDEX idx_converted_enhancement_version ON converted_packages(enhancement_version);

        -- Subpackage relationships table
        -- Tracks relationships between base packages and their subpackages
        -- e.g., nginx-devel (subpackage) -> nginx (base)
        CREATE TABLE subpackage_relationships (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            -- Base package name (without suffix like -devel, -doc)
            base_package TEXT NOT NULL,
            -- Full subpackage name (e.g., nginx-devel)
            subpackage_name TEXT NOT NULL,
            -- Component type this subpackage represents
            -- Common types: devel, doc, debuginfo, libs, common, data, lang
            component_type TEXT NOT NULL,
            -- Source format where this relationship was detected
            source_format TEXT NOT NULL,
            -- When this relationship was recorded
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            -- Ensure no duplicate relationships
            UNIQUE(base_package, subpackage_name)
        );

        -- Index for finding all subpackages of a base package
        CREATE INDEX idx_subpackage_base ON subpackage_relationships(base_package);
        -- Index for reverse lookup (find base from subpackage name)
        CREATE INDEX idx_subpackage_name ON subpackage_relationships(subpackage_name);
        -- Index for filtering by component type
        CREATE INDEX idx_subpackage_component ON subpackage_relationships(component_type);
        ",
    )?;

    info!("Schema version 36 applied successfully (enhancement framework)");
    Ok(())
}

pub fn migrate_v37(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 37");

    conn.execute_batch(
        "
        -- Add enhancement priority for lazy enhancement scheduling
        -- Higher priority packages are processed first
        -- 0=low, 1=normal (default), 2=high, 3=critical
        ALTER TABLE converted_packages ADD COLUMN enhancement_priority INTEGER DEFAULT 1;

        -- Index for efficient priority-ordered processing
        CREATE INDEX idx_converted_enhancement_priority
            ON converted_packages(enhancement_status, enhancement_priority DESC);
        ",
    )?;

    info!("Schema version 37 applied successfully (enhancement priority)");
    Ok(())
}

/// Migration 38: Add server-side conversion tracking columns
///
/// For Remi server to cache converted packages across restarts, we need to store:
/// - Package identity (name, version, distro)
/// - Chunk manifest (JSON array of hashes)
/// - CCS file location
pub fn migrate_v38(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 38");

    conn.execute_batch(
        "
        -- Server-side conversion tracking columns (nullable for client-side records)
        -- Package identity
        ALTER TABLE converted_packages ADD COLUMN package_name TEXT;
        ALTER TABLE converted_packages ADD COLUMN package_version TEXT;
        ALTER TABLE converted_packages ADD COLUMN distro TEXT;

        -- Chunk manifest and CCS file info
        ALTER TABLE converted_packages ADD COLUMN chunk_hashes_json TEXT;
        ALTER TABLE converted_packages ADD COLUMN total_size INTEGER;
        ALTER TABLE converted_packages ADD COLUMN content_hash TEXT;
        ALTER TABLE converted_packages ADD COLUMN ccs_path TEXT;

        -- Index for server-side lookups by package identity
        CREATE INDEX idx_converted_packages_identity
            ON converted_packages(distro, package_name, package_version);
        ",
    )?;

    info!("Schema version 38 applied successfully (server-side conversion tracking)");
    Ok(())
}

/// Migration 39: Add orphan_since column for orphan cleanup grace period
///
/// Tracks when a package became orphaned (no longer required by any explicit package).
/// This enables grace period policies: "don't remove orphans until they've been
/// orphaned for N days", preventing accidental removal of recently-orphaned packages.
///
/// - NULL: Package is not orphaned (either explicit or still required)
/// - Timestamp: When the package became orphaned
///
/// The column is cleared when a package becomes required again.
pub fn migrate_v39(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 39");

    conn.execute_batch(
        "
        -- Add orphan_since column for tracking when packages became orphaned
        -- NULL means not orphaned, timestamp means when it became orphaned
        ALTER TABLE troves ADD COLUMN orphan_since TEXT;

        -- Index for efficient orphan queries with grace period filtering
        CREATE INDEX idx_troves_orphan_since ON troves(orphan_since)
            WHERE orphan_since IS NOT NULL;
        ",
    )?;

    info!("Schema version 39 applied successfully (orphan tracking)");
    Ok(())
}

/// Migration 40: Download statistics for package index
///
/// Adds tables for tracking package download counts per-distro,
/// used by the public Remi package index for popularity rankings.
pub fn migrate_v40(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 40");

    conn.execute_batch(
        "
        -- Individual download events (write-heavy, periodically aggregated)
        CREATE TABLE download_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            distro TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT,
            downloaded_at TEXT NOT NULL DEFAULT (datetime('now')),
            client_ip_hash TEXT,
            user_agent TEXT
        );
        CREATE INDEX idx_download_stats_package
            ON download_stats(distro, package_name);
        CREATE INDEX idx_download_stats_time
            ON download_stats(downloaded_at);

        -- Aggregated download counts (read-heavy, periodically refreshed)
        CREATE TABLE download_counts (
            distro TEXT NOT NULL,
            package_name TEXT NOT NULL,
            total_count INTEGER NOT NULL DEFAULT 0,
            count_30d INTEGER NOT NULL DEFAULT 0,
            count_7d INTEGER NOT NULL DEFAULT 0,
            last_updated TEXT,
            PRIMARY KEY (distro, package_name)
        );
        CREATE INDEX idx_download_counts_popular
            ON download_counts(distro, total_count DESC);
        ",
    )?;

    info!("Schema version 40 applied successfully (download statistics)");
    Ok(())
}

