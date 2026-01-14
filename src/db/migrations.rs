// src/db/migrations.rs
//! Database migration implementations
//!
//! This module contains the individual migration functions for evolving
//! the Conary database schema. Each migration function handles a specific
//! version upgrade.

use crate::error::Result;
use rusqlite::Connection;
use tracing::{debug, info};

/// Initial schema - Version 1
///
/// Creates all core tables for Conary:
/// - troves: Package/component/collection metadata
/// - changesets: Transactional operation history
/// - files: File-level tracking with hashes
/// - flavors: Build-time variations
/// - provenance: Supply chain tracking
/// - dependencies: Trove relationships
pub fn migrate_v1(conn: &Connection) -> Result<()> {
    debug!("Creating schema version 1");

    conn.execute_batch(
        "
        -- Troves: The core unit (package, component, or collection)
        CREATE TABLE troves (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            type TEXT NOT NULL CHECK(type IN ('package', 'component', 'collection')),
            architecture TEXT,
            description TEXT,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            installed_by_changeset_id INTEGER,
            UNIQUE(name, version, architecture),
            FOREIGN KEY (installed_by_changeset_id) REFERENCES changesets(id)
        );

        CREATE INDEX idx_troves_name ON troves(name);
        CREATE INDEX idx_troves_type ON troves(type);

        -- Changesets: Atomic transactional operations
        CREATE TABLE changesets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            description TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('pending', 'applied', 'rolled_back')),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            applied_at TEXT,
            rolled_back_at TEXT
        );

        CREATE INDEX idx_changesets_status ON changesets(status);
        CREATE INDEX idx_changesets_created_at ON changesets(created_at);

        -- Files: File-level tracking with content hashing
        CREATE TABLE files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            sha256_hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            permissions INTEGER NOT NULL,
            owner TEXT,
            group_name TEXT,
            trove_id INTEGER NOT NULL,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_files_path ON files(path);
        CREATE INDEX idx_files_trove_id ON files(trove_id);
        CREATE INDEX idx_files_sha256 ON files(sha256_hash);

        -- Flavors: Build-time variations (arch, features, toolchain, etc.)
        CREATE TABLE flavors (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            UNIQUE(trove_id, key),
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_flavors_trove_id ON flavors(trove_id);
        CREATE INDEX idx_flavors_key ON flavors(key);

        -- Provenance: Supply chain tracking
        CREATE TABLE provenance (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL UNIQUE,
            source_url TEXT,
            source_branch TEXT,
            source_commit TEXT,
            build_host TEXT,
            build_time TEXT,
            builder TEXT,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_provenance_trove_id ON provenance(trove_id);

        -- Dependencies: Relationships between troves
        CREATE TABLE dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL,
            depends_on_name TEXT NOT NULL,
            depends_on_version TEXT,
            dependency_type TEXT NOT NULL CHECK(dependency_type IN ('runtime', 'build', 'optional')),
            version_constraint TEXT,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_dependencies_trove_id ON dependencies(trove_id);
        CREATE INDEX idx_dependencies_depends_on ON dependencies(depends_on_name);
        ",
    )?;

    info!("Schema version 1 created successfully");
    Ok(())
}

/// Schema Version 2: Add rollback tracking to changesets
///
/// Adds reversed_by_changeset_id to track which changeset reversed another
pub fn migrate_v2(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 2");

    conn.execute_batch(
        "
        ALTER TABLE changesets ADD COLUMN reversed_by_changeset_id INTEGER
            REFERENCES changesets(id) ON DELETE SET NULL;
        ",
    )?;

    info!("Schema version 2 applied successfully");
    Ok(())
}

/// Schema Version 3: Add content-addressable storage tracking
///
/// Adds tables for tracking file contents and file history:
/// - file_contents: Maps SHA-256 hashes to stored content locations
/// - file_history: Tracks file states per changeset for rollback support
pub fn migrate_v3(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 3");

    conn.execute_batch(
        "
        -- File contents stored in CAS (content-addressable storage)
        CREATE TABLE file_contents (
            sha256_hash TEXT PRIMARY KEY,
            content_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            stored_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_file_contents_stored_at ON file_contents(stored_at);

        -- File history for rollback support
        -- Tracks file states at each changeset
        CREATE TABLE file_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            changeset_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            sha256_hash TEXT,
            action TEXT NOT NULL CHECK(action IN ('add', 'modify', 'delete')),
            previous_hash TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (changeset_id) REFERENCES changesets(id) ON DELETE CASCADE,
            FOREIGN KEY (sha256_hash) REFERENCES file_contents(sha256_hash),
            FOREIGN KEY (previous_hash) REFERENCES file_contents(sha256_hash)
        );

        CREATE INDEX idx_file_history_changeset ON file_history(changeset_id);
        CREATE INDEX idx_file_history_path ON file_history(path);
        ",
    )?;

    info!("Schema version 3 applied successfully");
    Ok(())
}

/// Schema Version 4: Add repository management support
///
/// Adds tables for remote repository management:
/// - repositories: Repository configuration and metadata
/// - repository_packages: Package metadata index from repositories
pub fn migrate_v4(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 4");

    conn.execute_batch(
        "
        -- Repositories: Remote package sources
        CREATE TABLE repositories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            url TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 0,
            gpg_check INTEGER NOT NULL DEFAULT 1,
            gpg_key_url TEXT,
            metadata_expire INTEGER NOT NULL DEFAULT 3600,
            last_sync TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_repositories_name ON repositories(name);
        CREATE INDEX idx_repositories_enabled ON repositories(enabled);
        CREATE INDEX idx_repositories_priority ON repositories(priority);

        -- Repository packages: Available packages from repositories
        CREATE TABLE repository_packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            architecture TEXT,
            description TEXT,
            checksum TEXT NOT NULL,
            size INTEGER NOT NULL,
            download_url TEXT NOT NULL,
            dependencies TEXT,
            metadata TEXT,
            synced_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (repository_id) REFERENCES repositories(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_repo_packages_name ON repository_packages(name);
        CREATE INDEX idx_repo_packages_repo ON repository_packages(repository_id);
        CREATE INDEX idx_repo_packages_checksum ON repository_packages(checksum);
        CREATE UNIQUE INDEX idx_repo_packages_unique ON repository_packages(repository_id, name, version, architecture);
        ",
    )?;

    info!("Schema version 4 applied successfully");
    Ok(())
}

/// Schema Version 5: Add delta update support
///
/// Adds tables for tracking available deltas and bandwidth metrics:
/// - package_deltas: Available delta files with metadata
/// - delta_stats: Bandwidth savings metrics per changeset
pub fn migrate_v5(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 5");

    conn.execute_batch(
        "
        -- Package deltas: Available delta files for updates
        CREATE TABLE package_deltas (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            package_name TEXT NOT NULL,
            from_version TEXT NOT NULL,
            to_version TEXT NOT NULL,
            from_hash TEXT NOT NULL,
            to_hash TEXT NOT NULL,
            delta_url TEXT NOT NULL,
            delta_size INTEGER NOT NULL,
            delta_checksum TEXT NOT NULL,
            full_size INTEGER NOT NULL,
            compression_ratio REAL NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (from_hash) REFERENCES file_contents(sha256_hash),
            FOREIGN KEY (to_hash) REFERENCES file_contents(sha256_hash)
        );

        CREATE INDEX idx_package_deltas_package ON package_deltas(package_name);
        CREATE INDEX idx_package_deltas_from_hash ON package_deltas(from_hash);
        CREATE INDEX idx_package_deltas_to_hash ON package_deltas(to_hash);
        CREATE UNIQUE INDEX idx_package_deltas_transition ON package_deltas(package_name, from_version, to_version);

        -- Delta statistics: Bandwidth metrics per changeset
        CREATE TABLE delta_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            changeset_id INTEGER NOT NULL,
            total_bytes_saved INTEGER NOT NULL,
            deltas_applied INTEGER NOT NULL,
            full_downloads INTEGER NOT NULL,
            delta_failures INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (changeset_id) REFERENCES changesets(id) ON DELETE CASCADE
        );

        CREATE INDEX idx_delta_stats_changeset ON delta_stats(changeset_id);
        ",
    )?;

    info!("Schema version 5 applied successfully");
    Ok(())
}

/// Schema Version 6: Add install source tracking for package adoption
///
/// Adds install_source column to troves table to track how packages were installed:
/// - 'file': Installed from local package file
/// - 'repository': Installed from Conary repository
/// - 'adopted-track': Adopted from system, metadata only
/// - 'adopted-full': Adopted from system with full CAS storage
pub fn migrate_v6(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 6");

    conn.execute_batch(
        "
        -- Add install source tracking to troves
        ALTER TABLE troves ADD COLUMN install_source TEXT DEFAULT 'file';
        ",
    )?;

    info!("Schema version 6 applied successfully");
    Ok(())
}

/// Schema Version 7: Add metadata storage for rollback of removals
///
/// Adds metadata column to changesets table to store trove information
/// before deletion, enabling rollback of remove operations.
pub fn migrate_v7(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 7");

    conn.execute_batch(
        "
        -- Add metadata column to store trove info for removal rollback
        ALTER TABLE changesets ADD COLUMN metadata TEXT;
        ",
    )?;

    info!("Schema version 7 applied successfully");
    Ok(())
}

/// Schema Version 8: Add provides table for capability tracking
///
/// Creates a provides table to track what capabilities each package offers.
/// This enables self-contained dependency resolution without querying the
/// host package manager.
///
/// Capabilities include:
/// - Package names (e.g., "perl-Text-CharWidth")
/// - Virtual provides (e.g., "perl(Text::CharWidth)")
/// - Library sonames (e.g., "libc.so.6")
/// - File paths (e.g., "/usr/bin/perl")
pub fn migrate_v8(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 8");

    conn.execute_batch(
        "
        -- Capabilities/provides that packages offer
        CREATE TABLE provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            capability TEXT NOT NULL,
            version TEXT,
            UNIQUE(trove_id, capability)
        );

        -- Index for fast capability lookups during dependency resolution
        CREATE INDEX idx_provides_capability ON provides(capability);
        ",
    )?;

    info!("Schema version 8 applied successfully");
    Ok(())
}

/// Schema Version 9: Add scriptlets table for package install/remove hooks
///
/// Creates a scriptlets table to store package scriptlets (install/remove hooks).
/// This enables execution of scriptlets during installation and removal,
/// and storage for later removal operations.
///
/// Scriptlet phases include:
/// - pre-install, post-install
/// - pre-remove, post-remove
/// - pre-upgrade, post-upgrade (Arch-specific)
pub fn migrate_v9(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 9");

    conn.execute_batch(
        "
        -- Scriptlets: Package install/remove hooks
        CREATE TABLE scriptlets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            phase TEXT NOT NULL,
            interpreter TEXT NOT NULL,
            content TEXT NOT NULL,
            flags TEXT,
            package_format TEXT NOT NULL DEFAULT 'rpm',
            UNIQUE(trove_id, phase)
        );

        -- Index for fast lookup of scriptlets by trove
        CREATE INDEX idx_scriptlets_trove ON scriptlets(trove_id);
        ",
    )?;

    info!("Schema version 9 applied successfully");
    Ok(())
}

/// Schema Version 10: Add strict GPG signature mode
///
/// Adds gpg_strict column to repositories table. When enabled,
/// packages MUST have valid GPG signatures - missing signatures
/// are treated as failures rather than warnings.
pub fn migrate_v10(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 10");

    // Add gpg_strict column - default false for backwards compatibility
    conn.execute(
        "ALTER TABLE repositories ADD COLUMN gpg_strict INTEGER NOT NULL DEFAULT 0",
        [],
    )?;

    info!("Schema version 10 applied successfully");
    Ok(())
}

/// Schema Version 11: Add component model support
///
/// Adds tables for first-class component support:
/// - components: Independently installable units within packages
/// - component_dependencies: Dependencies between components
/// - component_provides: Capabilities provided by components
///
/// Also adds component_id column to files table to link files to components.
///
/// Components are split from packages at install time based on file paths:
/// - :runtime - Executables, assets, helpers (default bucket)
/// - :lib - Shared libraries (.so files in lib directories)
/// - :devel - Headers, static libs, pkg-config
/// - :doc - Documentation, man pages
/// - :config - Configuration files (/etc/*)
pub fn migrate_v11(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 11");

    conn.execute_batch(
        "
        -- Components: Independently installable units within packages
        -- A package like 'openssl' may have components :runtime, :lib, :devel, :doc, :config
        CREATE TABLE components (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            parent_trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            description TEXT,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            is_installed INTEGER NOT NULL DEFAULT 1,
            UNIQUE(parent_trove_id, name)
        );

        CREATE INDEX idx_components_parent ON components(parent_trove_id);
        CREATE INDEX idx_components_name ON components(name);
        CREATE INDEX idx_components_installed ON components(is_installed);

        -- Add component_id to files table
        -- NULL component_id indicates legacy (pre-component) installation
        ALTER TABLE files ADD COLUMN component_id INTEGER REFERENCES components(id) ON DELETE SET NULL;

        CREATE INDEX idx_files_component ON files(component_id);

        -- Component dependencies: Dependencies between components
        -- Can reference components in same package (depends_on_package = NULL) or other packages
        CREATE TABLE component_dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            component_id INTEGER NOT NULL REFERENCES components(id) ON DELETE CASCADE,
            depends_on_component TEXT NOT NULL,
            depends_on_package TEXT,
            dependency_type TEXT NOT NULL CHECK(dependency_type IN ('runtime', 'build', 'optional')),
            version_constraint TEXT,
            UNIQUE(component_id, depends_on_component, depends_on_package)
        );

        CREATE INDEX idx_component_deps_component ON component_dependencies(component_id);
        CREATE INDEX idx_component_deps_target ON component_dependencies(depends_on_package, depends_on_component);

        -- Component provides: Capabilities provided by components
        -- Similar to package-level provides but at component granularity
        CREATE TABLE component_provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            component_id INTEGER NOT NULL REFERENCES components(id) ON DELETE CASCADE,
            capability TEXT NOT NULL,
            version TEXT,
            UNIQUE(component_id, capability)
        );

        CREATE INDEX idx_component_provides_component ON component_provides(component_id);
        CREATE INDEX idx_component_provides_capability ON component_provides(capability);
        ",
    )?;

    info!("Schema version 11 applied successfully");
    Ok(())
}

/// Schema Version 12: Add install_reason for autoremove support
///
/// Adds install_reason column to troves table to track why a package was installed:
/// - 'explicit': User explicitly requested this package
/// - 'dependency': Installed automatically as a dependency
///
/// This enables the autoremove command to find orphaned dependencies.
pub fn migrate_v12(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 12");

    // Add install_reason column - default 'explicit' for backwards compatibility
    // (all existing packages are assumed to be explicitly installed)
    conn.execute(
        "ALTER TABLE troves ADD COLUMN install_reason TEXT NOT NULL DEFAULT 'explicit'",
        [],
    )?;

    // Create index for efficient orphan detection queries
    conn.execute(
        "CREATE INDEX idx_troves_install_reason ON troves(install_reason)",
        [],
    )?;

    info!("Schema version 12 applied successfully");
    Ok(())
}

/// Schema Version 13: Add collections/groups support
///
/// Creates tables for managing collections (meta-packages that group other packages):
/// - collection_members: Links collections to their member packages
///
/// Collections allow users to:
/// - Create named groups of packages (e.g., "development-tools", "server-base")
/// - Install/remove all packages in a group with a single command
/// - Define system profiles or roles
pub fn migrate_v13(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 13");

    conn.execute_batch(
        "
        -- Collection members: Links collections to their member packages
        CREATE TABLE collection_members (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            collection_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            member_name TEXT NOT NULL,
            member_version TEXT,
            is_optional INTEGER NOT NULL DEFAULT 0,
            UNIQUE(collection_id, member_name)
        );

        CREATE INDEX idx_collection_members_collection ON collection_members(collection_id);
        CREATE INDEX idx_collection_members_member ON collection_members(member_name);
        ",
    )?;

    info!("Schema version 13 applied successfully");
    Ok(())
}

/// Schema Version 14: Add flavor specification to troves
///
/// Adds flavor_spec column to troves table to store Conary-style flavor
/// specifications. Flavors enable build-time variations like:
/// - `[ssl, !debug]` - Required ssl, must not have debug
/// - `[~vmware, ~!xen]` - Prefers vmware, prefers not xen
/// - `[is: x86_64]` - Architecture requirement
///
/// The flavor_spec is stored as a canonicalized string for consistent
/// matching and comparison.
pub fn migrate_v14(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 14");

    // Add flavor_spec column - NULL for packages without flavor requirements
    conn.execute(
        "ALTER TABLE troves ADD COLUMN flavor_spec TEXT",
        [],
    )?;

    // Index for efficient flavor-based queries and matching
    conn.execute(
        "CREATE INDEX idx_troves_flavor ON troves(flavor_spec)",
        [],
    )?;

    info!("Schema version 14 applied successfully");
    Ok(())
}

/// Schema Version 15: Add package pinning support
///
/// Adds a `pinned` column to the troves table to support preventing
/// packages from being modified during updates.
///
/// Pinned packages:
/// - Are skipped during `conary update` operations
/// - Cannot be removed without first unpinning
/// - Can have multiple versions installed (for kernels, etc.)
pub fn migrate_v15(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 15");

    // Add pinned column - default false (0) for existing packages
    conn.execute(
        "ALTER TABLE troves ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
        [],
    )?;

    // Index for efficient pinned package queries
    conn.execute(
        "CREATE INDEX idx_troves_pinned ON troves(pinned) WHERE pinned = 1",
        [],
    )?;

    info!("Schema version 15 applied successfully");
    Ok(())
}

/// Schema Version 16: Add selection reason tracking
///
/// Adds a `selection_reason` column to track human-readable reasons
/// for why a package was installed:
///
/// - "Explicitly installed by user"
/// - "Required by nginx"
/// - "Installed via @server collection"
///
/// This enables better tracking of the dependency chain and
/// collection attribution for installed packages.
pub fn migrate_v16(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 16");

    // Add selection_reason column - NULL for existing packages (will show as "Unknown")
    conn.execute(
        "ALTER TABLE troves ADD COLUMN selection_reason TEXT",
        [],
    )?;

    // Update existing packages with default reasons based on install_reason
    conn.execute(
        "UPDATE troves SET selection_reason = 'Explicitly installed' WHERE install_reason = 'explicit' AND selection_reason IS NULL",
        [],
    )?;
    conn.execute(
        "UPDATE troves SET selection_reason = 'Installed as dependency' WHERE install_reason = 'dependency' AND selection_reason IS NULL",
        [],
    )?;

    info!("Schema version 16 applied successfully");
    Ok(())
}

/// Schema Version 17: Add trigger system for post-installation actions
///
/// Creates tables for the trigger system which provides:
/// - Path-based triggers that run when files matching patterns are installed/removed
/// - DAG-ordered execution (triggers can depend on other triggers)
/// - Built-in triggers for common system actions (ldconfig, update-desktop-database, etc.)
///
/// Tables:
/// - triggers: Defines trigger handlers with path patterns
/// - trigger_dependencies: DAG ordering between triggers
/// - changeset_triggers: Tracks which triggers were activated per changeset
pub fn migrate_v17(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 17");

    conn.execute_batch(
        "
        -- Triggers: Path-based handlers for post-installation actions
        CREATE TABLE triggers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            description TEXT,
            pattern TEXT NOT NULL,
            handler TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 50,
            enabled INTEGER NOT NULL DEFAULT 1,
            builtin INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX idx_triggers_name ON triggers(name);
        CREATE INDEX idx_triggers_enabled ON triggers(enabled) WHERE enabled = 1;
        CREATE INDEX idx_triggers_builtin ON triggers(builtin);

        -- Trigger dependencies: DAG ordering between triggers
        CREATE TABLE trigger_dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trigger_id INTEGER NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
            depends_on TEXT NOT NULL,
            UNIQUE(trigger_id, depends_on)
        );

        CREATE INDEX idx_trigger_deps_trigger ON trigger_dependencies(trigger_id);
        CREATE INDEX idx_trigger_deps_depends ON trigger_dependencies(depends_on);

        -- Changeset triggers: Track which triggers were activated per changeset
        CREATE TABLE changeset_triggers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            changeset_id INTEGER NOT NULL REFERENCES changesets(id) ON DELETE CASCADE,
            trigger_id INTEGER NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
            status TEXT NOT NULL DEFAULT 'pending',
            matched_files INTEGER NOT NULL DEFAULT 0,
            started_at TEXT,
            completed_at TEXT,
            output TEXT,
            UNIQUE(changeset_id, trigger_id)
        );

        CREATE INDEX idx_changeset_triggers_changeset ON changeset_triggers(changeset_id);
        CREATE INDEX idx_changeset_triggers_status ON changeset_triggers(status);
        ",
    )?;

    // Insert built-in system triggers
    conn.execute_batch(
        "
        -- ldconfig: Update shared library cache
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('ldconfig', 'Update shared library cache', '/usr/lib/*.so*,/usr/lib64/*.so*,/lib/*.so*,/lib64/*.so*', '/sbin/ldconfig', 10, 1);

        -- update-mime-database: Update MIME type database
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('update-mime-database', 'Update MIME type database', '/usr/share/mime/*', 'update-mime-database /usr/share/mime', 30, 1);

        -- update-desktop-database: Update desktop entry database
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('update-desktop-database', 'Update desktop entry database', '/usr/share/applications/*.desktop', 'update-desktop-database /usr/share/applications', 30, 1);

        -- gtk-update-icon-cache: Update GTK icon cache
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('gtk-update-icon-cache', 'Update GTK icon cache', '/usr/share/icons/*', 'gtk-update-icon-cache -f /usr/share/icons/hicolor', 40, 1);

        -- glib-compile-schemas: Compile GSettings schemas
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('glib-compile-schemas', 'Compile GSettings schemas', '/usr/share/glib-2.0/schemas/*.xml,/usr/share/glib-2.0/schemas/*.gschema.override', 'glib-compile-schemas /usr/share/glib-2.0/schemas', 30, 1);

        -- systemd-tmpfiles: Create tmpfiles.d entries
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('systemd-tmpfiles', 'Create tmpfiles entries', '/usr/lib/tmpfiles.d/*.conf', 'systemd-tmpfiles --create', 20, 1);

        -- systemd-sysusers: Create system users
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('systemd-sysusers', 'Create system users', '/usr/lib/sysusers.d/*.conf', 'systemd-sysusers', 15, 1);

        -- systemctl-daemon-reload: Reload systemd units
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('systemctl-daemon-reload', 'Reload systemd daemon', '/usr/lib/systemd/system/*,/usr/lib/systemd/user/*', 'systemctl daemon-reload', 50, 1);

        -- fc-cache: Update font cache
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('fc-cache', 'Update font cache', '/usr/share/fonts/*', 'fc-cache -s', 40, 1);

        -- depmod: Update kernel module dependencies
        INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
        VALUES ('depmod', 'Update kernel module dependencies', '/lib/modules/*/modules.*,/usr/lib/modules/*/*.ko*', 'depmod -a', 20, 1);
        ",
    )?;

    // Add trigger dependencies (systemd-sysusers before systemd-tmpfiles)
    conn.execute(
        "INSERT INTO trigger_dependencies (trigger_id, depends_on)
         SELECT t.id, 'systemd-sysusers'
         FROM triggers t
         WHERE t.name = 'systemd-tmpfiles'",
        [],
    )?;

    info!("Schema version 17 applied successfully (trigger system with {} built-in triggers)", 10);
    Ok(())
}

/// Schema Version 18: Add system state snapshots
///
/// Creates tables for full system state tracking:
/// - system_states: Stores numbered snapshots of system state
/// - state_members: Packages in each state snapshot
///
/// State snapshots provide cleaner rollback semantics than per-changeset rollback:
/// - Each state captures the complete set of installed packages
/// - States are numbered sequentially (1, 2, 3...)
/// - The "active" state marks the current system configuration
/// - Rollback to any previous state computes the minimal operations needed
/// - State pruning removes old states to save space
pub fn migrate_v18(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 18");

    conn.execute_batch(
        "
        -- System states: Numbered snapshots of complete system state
        CREATE TABLE system_states (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            state_number INTEGER NOT NULL UNIQUE,
            summary TEXT NOT NULL,
            description TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            is_active INTEGER NOT NULL DEFAULT 0,
            package_count INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX idx_system_states_number ON system_states(state_number);
        CREATE INDEX idx_system_states_active ON system_states(is_active) WHERE is_active = 1;
        CREATE INDEX idx_system_states_created ON system_states(created_at);

        -- State members: Packages in each state snapshot
        CREATE TABLE state_members (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            state_id INTEGER NOT NULL REFERENCES system_states(id) ON DELETE CASCADE,
            trove_name TEXT NOT NULL,
            trove_version TEXT NOT NULL,
            architecture TEXT,
            install_reason TEXT NOT NULL DEFAULT 'explicit',
            selection_reason TEXT,
            UNIQUE(state_id, trove_name)
        );

        CREATE INDEX idx_state_members_state ON state_members(state_id);
        CREATE INDEX idx_state_members_name ON state_members(trove_name);
        ",
    )?;

    // Create initial state (state 0) representing the pre-Conary system
    // This is only created if there are already packages installed
    let package_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM troves WHERE type = 'package'",
        [],
        |row| row.get(0),
    )?;

    if package_count > 0 {
        // Create state 0 with current packages
        conn.execute(
            "INSERT INTO system_states (state_number, summary, description, is_active, package_count)
             VALUES (0, 'Initial system state', 'State snapshot created during migration to capture existing packages', 1, ?1)",
            [package_count],
        )?;

        // Populate state 0 with current packages
        conn.execute(
            "INSERT INTO state_members (state_id, trove_name, trove_version, architecture, install_reason, selection_reason)
             SELECT (SELECT id FROM system_states WHERE state_number = 0),
                    name, version, architecture, install_reason, selection_reason
             FROM troves WHERE type = 'package'",
            [],
        )?;
    }

    info!("Schema version 18 applied successfully (system state snapshots)");
    Ok(())
}

/// Schema Version 19: Add typed dependencies
///
/// Adds explicit dependency kind tracking for type-safe dependency resolution.
/// Each dependency now has a `kind` field indicating its type:
/// - package: Standard package dependency
/// - soname: Shared library (libfoo.so.1)
/// - python: Python module
/// - perl: Perl module
/// - ruby: Ruby gem
/// - java: Java package
/// - pkgconfig: pkg-config module
/// - cmake: CMake package
/// - binary: Executable binary
/// - file: Specific file path
/// - interpreter: ELF interpreter
/// - abi: ABI compatibility
/// - kmod: Kernel module
///
/// Also adds kind to provides table for typed provider matching.
pub fn migrate_v19(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 19");

    conn.execute_batch(
        "
        -- Add kind column to dependencies table
        ALTER TABLE dependencies ADD COLUMN kind TEXT DEFAULT 'package';

        -- Create index for kind-based lookups
        CREATE INDEX idx_dependencies_kind ON dependencies(kind);
        CREATE INDEX idx_dependencies_kind_name ON dependencies(kind, depends_on_name);

        -- Add kind column to provides table
        ALTER TABLE provides ADD COLUMN kind TEXT DEFAULT 'package';

        -- Create index for typed provider lookups
        CREATE INDEX idx_provides_kind ON provides(kind);
        CREATE INDEX idx_provides_kind_capability ON provides(kind, capability);
        ",
    )?;

    // Migrate existing dependencies by parsing kind from depends_on_name
    // Pattern: kind(name) -> kind, name
    // Example: python(requests) -> kind='python', depends_on_name='requests'
    let dep_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dependencies WHERE depends_on_name LIKE '%(%'",
        [],
        |row| row.get(0),
    )?;

    if dep_count > 0 {
        info!("Migrating {} typed dependencies...", dep_count);

        // Update python dependencies
        conn.execute(
            "UPDATE dependencies SET kind = 'python',
             depends_on_name = SUBSTR(depends_on_name, 8, LENGTH(depends_on_name) - 8)
             WHERE depends_on_name LIKE 'python(%)'",
            [],
        )?;

        // Update perl dependencies
        conn.execute(
            "UPDATE dependencies SET kind = 'perl',
             depends_on_name = SUBSTR(depends_on_name, 6, LENGTH(depends_on_name) - 6)
             WHERE depends_on_name LIKE 'perl(%)'",
            [],
        )?;

        // Update ruby dependencies
        conn.execute(
            "UPDATE dependencies SET kind = 'ruby',
             depends_on_name = SUBSTR(depends_on_name, 6, LENGTH(depends_on_name) - 6)
             WHERE depends_on_name LIKE 'ruby(%)'",
            [],
        )?;

        // Update soname dependencies
        conn.execute(
            "UPDATE dependencies SET kind = 'soname',
             depends_on_name = SUBSTR(depends_on_name, 8, LENGTH(depends_on_name) - 8)
             WHERE depends_on_name LIKE 'soname(%)'",
            [],
        )?;

        // Update pkgconfig dependencies
        conn.execute(
            "UPDATE dependencies SET kind = 'pkgconfig',
             depends_on_name = SUBSTR(depends_on_name, 11, LENGTH(depends_on_name) - 11)
             WHERE depends_on_name LIKE 'pkgconfig(%)'",
            [],
        )?;

        // Update file dependencies
        conn.execute(
            "UPDATE dependencies SET kind = 'file',
             depends_on_name = SUBSTR(depends_on_name, 6, LENGTH(depends_on_name) - 6)
             WHERE depends_on_name LIKE 'file(%)'",
            [],
        )?;
    }

    // Migrate existing provides by parsing kind from capability
    let provides_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM provides WHERE capability LIKE '%(%'",
        [],
        |row| row.get(0),
    )?;

    if provides_count > 0 {
        info!("Migrating {} typed provides...", provides_count);

        // Update python provides
        conn.execute(
            "UPDATE provides SET kind = 'python',
             capability = SUBSTR(capability, 8, LENGTH(capability) - 8)
             WHERE capability LIKE 'python(%)'",
            [],
        )?;

        // Update perl provides
        conn.execute(
            "UPDATE provides SET kind = 'perl',
             capability = SUBSTR(capability, 6, LENGTH(capability) - 6)
             WHERE capability LIKE 'perl(%)'",
            [],
        )?;

        // Update ruby provides
        conn.execute(
            "UPDATE provides SET kind = 'ruby',
             capability = SUBSTR(capability, 6, LENGTH(capability) - 6)
             WHERE capability LIKE 'ruby(%)'",
            [],
        )?;

        // Update soname provides
        conn.execute(
            "UPDATE provides SET kind = 'soname',
             capability = SUBSTR(capability, 8, LENGTH(capability) - 8)
             WHERE capability LIKE 'soname(%)'",
            [],
        )?;

        // Update java provides
        conn.execute(
            "UPDATE provides SET kind = 'java',
             capability = SUBSTR(capability, 6, LENGTH(capability) - 6)
             WHERE capability LIKE 'java(%)'",
            [],
        )?;
    }

    info!("Schema version 19 applied successfully (typed dependencies)");
    Ok(())
}

/// Version 20: Label system for package provenance tracking
///
/// Labels use the format `repository@namespace:tag` to identify where packages came from.
/// This enables:
/// - Tracking package origin (which repository/branch)
/// - Label-based dependency resolution
/// - Branch-aware updates and rollbacks
///
/// Creates:
/// - labels: Label definitions with repository, namespace, tag
/// - label_path: Ordered list of labels for resolution priority
/// - trove label column: Track which label a package came from
pub fn migrate_v20(conn: &Connection) -> Result<()> {
    debug!("Migrating to schema version 20");

    conn.execute_batch(
        "
        -- Labels table: defines available labels
        -- Format: repository@namespace:tag
        CREATE TABLE IF NOT EXISTS labels (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository TEXT NOT NULL,
            namespace TEXT NOT NULL,
            tag TEXT NOT NULL,
            description TEXT,
            parent_label_id INTEGER REFERENCES labels(id),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(repository, namespace, tag)
        );

        -- Index for label lookups
        CREATE INDEX idx_labels_full ON labels(repository, namespace, tag);
        CREATE INDEX idx_labels_repo ON labels(repository);

        -- Label path table: defines search order for package resolution
        -- Lower priority number = higher priority (searched first)
        CREATE TABLE IF NOT EXISTS label_path (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            label_id INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
            priority INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1,
            UNIQUE(label_id)
        );

        -- Index for priority-based lookups
        CREATE INDEX idx_label_path_priority ON label_path(priority) WHERE enabled = 1;

        -- Add label column to troves for provenance tracking
        ALTER TABLE troves ADD COLUMN label_id INTEGER REFERENCES labels(id);

        -- Index for label-based trove lookups
        CREATE INDEX idx_troves_label ON troves(label_id);
        ",
    )?;

    info!("Schema version 20 applied successfully (label system)");
    Ok(())
}

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
