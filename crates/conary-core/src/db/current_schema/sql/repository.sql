-- Current repository, federation, derivation, and TUF schema.
-- Part of the current pre-alpha schema epoch.

CREATE TABLE repository_source_policies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_identity TEXT NOT NULL,
            scope_kind TEXT NOT NULL CHECK(scope_kind IN ('repository', 'group')),
            scope_identity TEXT NOT NULL,
            ecosystem TEXT NOT NULL CHECK(ecosystem IN ('rpm', 'deb', 'alpm', 'eopkg')),
            version_scheme TEXT NOT NULL CHECK(version_scheme IN ('rpm', 'debian', 'arch', 'eopkg')),
            stream_kind TEXT NOT NULL CHECK(stream_kind IN ('release', 'channel', 'rolling')),
            stream_identity TEXT NOT NULL,
            update_mode TEXT NOT NULL CHECK(update_mode IN ('follow', 'pin')),
            CHECK(length(source_identity) BETWEEN 1 AND 255 AND trim(source_identity) = source_identity),
            CHECK(length(scope_identity) BETWEEN 1 AND 255 AND trim(scope_identity) = scope_identity),
            CHECK(length(stream_identity) BETWEEN 1 AND 255 AND trim(stream_identity) = stream_identity),
            CHECK(
                (ecosystem = 'rpm' AND version_scheme = 'rpm')
                OR (ecosystem = 'deb' AND version_scheme = 'debian')
                OR (ecosystem = 'alpm' AND version_scheme = 'arch')
                OR (ecosystem = 'eopkg' AND version_scheme = 'eopkg')
            ),
            UNIQUE(source_identity, scope_kind, scope_identity)
        );
CREATE TABLE repositories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            url TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            priority INTEGER NOT NULL DEFAULT 0,
            trust_policy_json TEXT,
            metadata_expire INTEGER NOT NULL DEFAULT 3600,
            last_sync TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        , content_url TEXT,
            default_strategy TEXT
                CHECK(default_strategy IS NULL OR default_strategy IN ('binary', 'remi', 'static')),
            default_strategy_endpoint TEXT,
            source_profile TEXT,
            tuf_enabled INTEGER NOT NULL DEFAULT 0, tuf_root_version INTEGER, tuf_root_url TEXT,
            security_advisory_support TEXT NOT NULL DEFAULT 'unknown'
            CHECK(security_advisory_support IN ('unknown', 'unsupported', 'supported')),
            package_format TEXT NOT NULL DEFAULT 'unspecified'
                CHECK(package_format IN ('arch', 'deb', 'rpm', 'eopkg', 'json', 'unspecified')),
            parser_config_json TEXT,
            managed_by TEXT NOT NULL DEFAULT 'operator'
                CHECK(managed_by IN ('operator', 'remi-config', 'native-projection', 'package-projection')),
            source_policy_id INTEGER REFERENCES repository_source_policies(id) ON DELETE RESTRICT,
            repository_identity TEXT,
            stream_binding_sha256 TEXT,
            authenticated_snapshot_sha256 TEXT,
            CHECK(
                (package_format = 'unspecified' AND parser_config_json IS NULL)
                OR (package_format != 'unspecified' AND parser_config_json IS NOT NULL)
            ),
            CHECK(
                (package_format IN ('arch', 'deb', 'rpm', 'eopkg')
                    AND source_policy_id IS NOT NULL
                    AND repository_identity IS NOT NULL
                    AND stream_binding_sha256 IS NOT NULL)
                OR (package_format NOT IN ('arch', 'deb', 'rpm', 'eopkg')
                    AND source_policy_id IS NULL
                    AND repository_identity IS NULL
                    AND stream_binding_sha256 IS NULL
                    AND authenticated_snapshot_sha256 IS NULL)
            ),
            CHECK(repository_identity IS NULL OR (length(repository_identity) BETWEEN 1 AND 255 AND trim(repository_identity) = repository_identity)),
            CHECK(stream_binding_sha256 IS NULL OR (length(stream_binding_sha256) = 64 AND stream_binding_sha256 NOT GLOB '*[^0-9a-f]*')),
            CHECK(authenticated_snapshot_sha256 IS NULL OR (length(authenticated_snapshot_sha256) = 64 AND authenticated_snapshot_sha256 NOT GLOB '*[^0-9a-f]*')),
            UNIQUE(source_policy_id, repository_identity)
        );
CREATE INDEX idx_repositories_name ON repositories(name);
CREATE INDEX idx_repositories_enabled ON repositories(enabled);
CREATE INDEX idx_repositories_priority ON repositories(priority);
CREATE TRIGGER repositories_validate_repository_scope_insert
BEFORE INSERT ON repositories
WHEN NEW.source_policy_id IS NOT NULL
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM repository_source_policies policy
        WHERE policy.id = NEW.source_policy_id
          AND policy.scope_kind = 'repository'
          AND policy.scope_identity != NEW.repository_identity
    ) THEN RAISE(ABORT, 'repository-scope source policy identity mismatch') END;
END;
CREATE TRIGGER repositories_validate_repository_scope_update
BEFORE UPDATE OF source_policy_id, repository_identity ON repositories
WHEN NEW.source_policy_id IS NOT NULL
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM repository_source_policies policy
        WHERE policy.id = NEW.source_policy_id
          AND policy.scope_kind = 'repository'
          AND policy.scope_identity != NEW.repository_identity
    ) THEN RAISE(ABORT, 'repository-scope source policy identity mismatch') END;
END;
CREATE TABLE repository_sync_runs (
            run_id TEXT PRIMARY KEY,
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            scope_kind TEXT NOT NULL CHECK(scope_kind = 'repository'),
            scope_identity TEXT NOT NULL,
            owner_instance_uuid TEXT NOT NULL,
            fencing_epoch INTEGER NOT NULL CHECK(fencing_epoch > 0),
            staging_repository_id INTEGER NOT NULL,
            input_revision TEXT CHECK(input_revision IS NULL OR json_valid(input_revision)),
            candidate_revision TEXT CHECK(candidate_revision IS NULL OR json_valid(candidate_revision)),
            state TEXT NOT NULL CHECK(state IN (
                'created',
                'fetching_roots',
                'fetching_objects',
                'authenticated',
                'ingesting',
                'validating',
                'ready_to_publish',
                'published',
                'failed',
                'abandoned'
            )),
            started_at INTEGER NOT NULL,
            heartbeat_at INTEGER NOT NULL,
            lease_expires_at INTEGER NOT NULL,
            finished_at INTEGER,
            failure_stage TEXT CHECK(failure_stage IS NULL OR failure_stage IN (
                'created',
                'fetching_roots',
                'fetching_objects',
                'authenticated',
                'ingesting',
                'validating',
                'ready_to_publish',
                'publishing'
            )),
            failure_category TEXT CHECK(failure_category IS NULL OR failure_category IN (
                'transport',
                'wire_contract',
                'database',
                'fenced',
                'internal'
            )),
            failure_evidence TEXT,
            CHECK(length(run_id) = 36),
            CHECK(length(owner_instance_uuid) = 36),
            CHECK(scope_identity = CAST(repository_id AS TEXT)),
            CHECK(heartbeat_at >= started_at),
            CHECK(lease_expires_at >= heartbeat_at),
            CHECK(
                (state = 'published'
                    AND finished_at IS NOT NULL
                    AND failure_stage IS NULL
                    AND failure_category IS NULL
                    AND failure_evidence IS NULL)
                OR (state IN ('failed', 'abandoned')
                    AND finished_at IS NOT NULL
                    AND failure_stage IS NOT NULL
                    AND failure_category IS NOT NULL
                    AND failure_evidence IS NOT NULL)
                OR (state NOT IN ('published', 'failed', 'abandoned')
                    AND finished_at IS NULL
                    AND failure_stage IS NULL
                    AND failure_category IS NULL
                    AND failure_evidence IS NULL)
            ),
            UNIQUE(repository_id, fencing_epoch)
        );
CREATE INDEX idx_repository_sync_runs_repository
            ON repository_sync_runs(repository_id, fencing_epoch DESC);
CREATE INDEX idx_repository_sync_runs_state
            ON repository_sync_runs(state);
CREATE TABLE repository_sync_scopes (
            repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,
            fencing_epoch INTEGER NOT NULL CHECK(fencing_epoch > 0),
            current_run_id TEXT NOT NULL,
            active_revision TEXT CHECK(active_revision IS NULL OR json_valid(active_revision)),
            UNIQUE(current_run_id),
            FOREIGN KEY(current_run_id) REFERENCES repository_sync_runs(run_id) ON DELETE RESTRICT
        );
CREATE TABLE repository_source_pins (
            repository_id INTEGER PRIMARY KEY REFERENCES repositories(id) ON DELETE CASCADE,
            snapshot_sha256 TEXT NOT NULL,
            CHECK(length(snapshot_sha256) = 64 AND snapshot_sha256 NOT GLOB '*[^0-9a-f]*')
        );
CREATE TRIGGER repository_source_pins_require_pin_policy
BEFORE INSERT ON repository_source_pins
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM repositories repository
        JOIN repository_source_policies policy ON policy.id = repository.source_policy_id
        WHERE repository.id = NEW.repository_id
          AND policy.update_mode = 'pin'
    ) THEN RAISE(ABORT, 'repository source pin requires pin update policy') END;
END;
CREATE TABLE native_repository_takeovers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            selected_root TEXT NOT NULL UNIQUE,
            preview_sha256 TEXT NOT NULL,
            preview_json TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            CHECK(length(selected_root) > 0),
            CHECK(length(preview_sha256) = 64 AND preview_sha256 NOT GLOB '*[^0-9a-f]*')
        );
CREATE TABLE native_repository_takeover_members (
            takeover_id INTEGER NOT NULL REFERENCES native_repository_takeovers(id) ON DELETE CASCADE,
            repository_id INTEGER NOT NULL UNIQUE REFERENCES repositories(id) ON DELETE RESTRICT,
            PRIMARY KEY(takeover_id, repository_id)
        );
CREATE TRIGGER native_repository_takeover_members_require_owned_repository
BEFORE INSERT ON native_repository_takeover_members
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM repositories
        WHERE id = NEW.repository_id AND managed_by = 'native-projection'
    ) THEN RAISE(ABORT, 'native takeover member requires native-projection ownership') END;
END;
CREATE TRIGGER native_repository_takeover_members_preserve_owned_repository
BEFORE UPDATE OF managed_by ON repositories
WHEN EXISTS (
    SELECT 1 FROM native_repository_takeover_members
    WHERE repository_id = OLD.id
)
BEGIN
    SELECT CASE WHEN NEW.managed_by != 'native-projection'
        THEN RAISE(ABORT, 'native takeover repository ownership is immutable') END;
END;
CREATE TRIGGER native_repository_takeover_members_preserve_repository_authority
BEFORE UPDATE ON repositories
WHEN EXISTS (
    SELECT 1 FROM native_repository_takeover_members
    WHERE repository_id = OLD.id
)
BEGIN
    SELECT CASE WHEN
        NEW.name IS NOT OLD.name
        OR NEW.url IS NOT OLD.url
        OR NEW.content_url IS NOT OLD.content_url
        OR NEW.enabled IS NOT OLD.enabled
        OR NEW.priority IS NOT OLD.priority
        OR NEW.trust_policy_json IS NOT OLD.trust_policy_json
        OR NEW.metadata_expire IS NOT OLD.metadata_expire
        OR NEW.default_strategy IS NOT OLD.default_strategy
        OR NEW.default_strategy_endpoint IS NOT OLD.default_strategy_endpoint
        OR NEW.source_profile IS NOT OLD.source_profile
        OR NEW.tuf_enabled IS NOT OLD.tuf_enabled
        OR NEW.tuf_root_version IS NOT OLD.tuf_root_version
        OR NEW.tuf_root_url IS NOT OLD.tuf_root_url
        OR NEW.security_advisory_support IS NOT OLD.security_advisory_support
        OR NEW.package_format IS NOT OLD.package_format
        OR NEW.parser_config_json IS NOT OLD.parser_config_json
        OR NEW.managed_by IS NOT OLD.managed_by
        OR NEW.source_policy_id IS NOT OLD.source_policy_id
        OR NEW.repository_identity IS NOT OLD.repository_identity
        OR NEW.stream_binding_sha256 IS NOT OLD.stream_binding_sha256
        THEN RAISE(ABORT, 'native takeover repository authority changes require projection enrollment') END;
END;
CREATE TABLE native_repository_projections (
            takeover_id INTEGER NOT NULL REFERENCES native_repository_takeovers(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            content BLOB NOT NULL,
            sha256 TEXT NOT NULL,
            mode INTEGER NOT NULL CHECK(mode BETWEEN 0 AND 4095),
            prior_existed INTEGER NOT NULL CHECK(prior_existed IN (0, 1)),
            prior_content BLOB,
            prior_mode INTEGER,
            CHECK(length(path) > 1 AND substr(path, 1, 1) = '/'),
            CHECK(length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
            CHECK((prior_existed = 1 AND prior_content IS NOT NULL AND prior_mode IS NOT NULL)
                OR (prior_existed = 0 AND prior_content IS NULL AND prior_mode IS NULL)),
            PRIMARY KEY(takeover_id, path)
        );
CREATE TABLE package_repository_enrollments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER REFERENCES troves(id) ON DELETE CASCADE,
            owner_kind TEXT NOT NULL CHECK(owner_kind IN ('package', 'retained')),
            source_package TEXT NOT NULL,
            source_version TEXT NOT NULL,
            intent_id TEXT NOT NULL,
            intent_sha256 TEXT NOT NULL,
            intent_json TEXT NOT NULL,
            last_owner TEXT NOT NULL CHECK(last_owner IN ('remove-when-unowned', 'retain')),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            CHECK(length(source_package) BETWEEN 1 AND 255),
            CHECK(length(source_version) BETWEEN 1 AND 255),
            CHECK(length(intent_id) BETWEEN 1 AND 255 AND trim(intent_id) = intent_id),
            CHECK(length(intent_sha256) = 64 AND intent_sha256 NOT GLOB '*[^0-9a-f]*'),
            CHECK((owner_kind = 'package' AND trove_id IS NOT NULL)
                OR (owner_kind = 'retained' AND trove_id IS NULL)),
            UNIQUE(trove_id, intent_id)
        );
CREATE INDEX idx_package_repository_enrollments_trove
ON package_repository_enrollments(trove_id);
CREATE TABLE package_repository_enrollment_members (
            enrollment_id INTEGER NOT NULL REFERENCES package_repository_enrollments(id) ON DELETE CASCADE,
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE RESTRICT,
            repository_name TEXT NOT NULL,
            definition_sha256 TEXT NOT NULL,
            CHECK(length(repository_name) BETWEEN 1 AND 255),
            CHECK(length(definition_sha256) = 64 AND definition_sha256 NOT GLOB '*[^0-9a-f]*'),
            PRIMARY KEY(enrollment_id, repository_id),
            UNIQUE(enrollment_id, repository_name)
        );
CREATE INDEX idx_package_repository_enrollment_members_repository
ON package_repository_enrollment_members(repository_id);
CREATE TABLE package_repository_projection_owners (
            enrollment_id INTEGER NOT NULL REFERENCES package_repository_enrollments(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            mode INTEGER NOT NULL CHECK(mode BETWEEN 0 AND 4095),
            role TEXT NOT NULL CHECK(role IN ('declaration', 'trust-root')),
            CHECK(length(path) > 1 AND substr(path, 1, 1) = '/'),
            CHECK(length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
            PRIMARY KEY(enrollment_id, path)
        );
CREATE INDEX idx_package_repository_projection_owners_path
ON package_repository_projection_owners(path);
CREATE TRIGGER package_repository_members_require_owned_repository
BEFORE INSERT ON package_repository_enrollment_members
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM repositories
        WHERE id = NEW.repository_id AND managed_by = 'package-projection'
    ) THEN RAISE(ABORT, 'package enrollment member requires package-projection ownership') END;
END;
CREATE TRIGGER package_repository_members_preserve_owned_repository
BEFORE UPDATE OF managed_by ON repositories
WHEN EXISTS (
    SELECT 1 FROM package_repository_enrollment_members
    WHERE repository_id = OLD.id
)
BEGIN
    SELECT CASE WHEN NEW.managed_by != 'package-projection'
        THEN RAISE(ABORT, 'package enrollment repository ownership is immutable') END;
END;
CREATE TRIGGER package_repository_members_preserve_repository_authority
BEFORE UPDATE ON repositories
WHEN EXISTS (
    SELECT 1 FROM package_repository_enrollment_members
    WHERE repository_id = OLD.id
)
BEGIN
    SELECT CASE WHEN
        NEW.name IS NOT OLD.name
        OR NEW.url IS NOT OLD.url
        OR NEW.content_url IS NOT OLD.content_url
        OR NEW.enabled IS NOT OLD.enabled
        OR NEW.priority IS NOT OLD.priority
        OR NEW.trust_policy_json IS NOT OLD.trust_policy_json
        OR NEW.metadata_expire IS NOT OLD.metadata_expire
        OR NEW.default_strategy IS NOT OLD.default_strategy
        OR NEW.default_strategy_endpoint IS NOT OLD.default_strategy_endpoint
        OR NEW.source_profile IS NOT OLD.source_profile
        OR NEW.tuf_enabled IS NOT OLD.tuf_enabled
        OR NEW.tuf_root_version IS NOT OLD.tuf_root_version
        OR NEW.tuf_root_url IS NOT OLD.tuf_root_url
        OR NEW.security_advisory_support IS NOT OLD.security_advisory_support
        OR NEW.package_format IS NOT OLD.package_format
        OR NEW.parser_config_json IS NOT OLD.parser_config_json
        OR NEW.managed_by IS NOT OLD.managed_by
        OR NEW.source_policy_id IS NOT OLD.source_policy_id
        OR NEW.repository_identity IS NOT OLD.repository_identity
        OR NEW.stream_binding_sha256 IS NOT OLD.stream_binding_sha256
        THEN RAISE(ABORT, 'package enrollment repository authority changes require package transaction') END;
END;
CREATE TABLE repository_packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            architecture TEXT CHECK(architecture IS NULL OR length(architecture) > 0),
            debian_multi_arch TEXT
                CHECK(debian_multi_arch IS NULL OR debian_multi_arch IN ('no', 'same', 'allowed', 'foreign')),
            description TEXT,
            checksum TEXT NOT NULL,
            size INTEGER NOT NULL,
            download_url TEXT NOT NULL,
            metadata TEXT,
            synced_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, is_security_update INTEGER NOT NULL DEFAULT 0, severity TEXT, cve_ids TEXT, advisory_id TEXT, advisory_url TEXT,
            source_profile TEXT,
            version_scheme TEXT NOT NULL
                CHECK(version_scheme IN ('conary', 'rpm', 'debian', 'arch', 'eopkg')), canonical_id INTEGER
            REFERENCES canonical_packages(id) ON DELETE SET NULL, package_release TEXT NOT NULL DEFAULT '',
            CHECK(
                (version_scheme = 'debian' AND debian_multi_arch IS NOT NULL)
                OR (version_scheme != 'debian' AND debian_multi_arch IS NULL)
            ),
            FOREIGN KEY (repository_id) REFERENCES repositories(id) ON DELETE CASCADE
        );
CREATE INDEX idx_repo_packages_name ON repository_packages(name);
CREATE INDEX idx_repo_packages_repo ON repository_packages(repository_id);
CREATE INDEX idx_repo_packages_checksum ON repository_packages(checksum);
CREATE UNIQUE INDEX idx_repo_packages_exact_identity
ON repository_packages(
    repository_id,
    name,
    version,
    package_release,
    COALESCE(architecture, '')
);
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
CREATE INDEX idx_repo_packages_security ON repository_packages(is_security_update) WHERE is_security_update = 1;
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
            status TEXT NOT NULL DEFAULT 'pending'
                CHECK(status IN ('pending', 'built', 'stale', 'error')),
            -- Built trove ID (when status = built)
            built_trove_id INTEGER REFERENCES troves(id) ON DELETE SET NULL,
            -- Model file this came from (NULL if created via CLI)
            model_source TEXT,
            -- Error message if status = error
            error_message TEXT,
            last_built_version TEXT,
            last_built_parent_version TEXT,
            build_artifact_hash TEXT,
            build_artifact_path TEXT,
            build_artifact_size INTEGER,
            CHECK (
                (version_policy = 'inherit' AND version_suffix IS NULL AND specific_version IS NULL)
                OR (version_policy = 'suffix' AND version_suffix IS NOT NULL AND specific_version IS NULL)
                OR (version_policy = 'specific' AND version_suffix IS NULL AND specific_version IS NOT NULL)
            )
        );
CREATE INDEX idx_derived_packages_parent ON derived_packages(parent_name);
CREATE INDEX idx_derived_packages_status ON derived_packages(status);
CREATE INDEX idx_derived_packages_built ON derived_packages(built_trove_id);
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
CREATE INDEX idx_chunk_access_lru ON chunk_access(last_accessed ASC);
CREATE INDEX idx_chunk_access_size ON chunk_access(size_bytes DESC);
CREATE INDEX idx_chunk_access_count ON chunk_access(access_count DESC);
CREATE INDEX idx_chunk_access_protected ON chunk_access(protected) WHERE protected = 1;
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
            -- Primary strategy for indexing: remi, delegate, repository_package
            primary_strategy TEXT NOT NULL
                CHECK(primary_strategy IN (
                    'remi', 'delegate', 'repository_package'
                )),

            -- Caching policy
            -- TTL in seconds (NULL = don't cache, use repository default)
            cache_ttl INTEGER,
            -- Higher priority = cached longer, lower priority for eviction
            cache_priority INTEGER NOT NULL DEFAULT 0,

            UNIQUE(repository_id, name, version)
        );
CREATE INDEX idx_resolution_strategy ON package_resolution(repository_id, primary_strategy);
CREATE INDEX idx_resolution_name ON package_resolution(repository_id, name);
CREATE INDEX idx_resolution_cache_priority ON package_resolution(cache_priority DESC);
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
CREATE TABLE download_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_profile TEXT NOT NULL
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch', 'solus')),
            package_name TEXT NOT NULL,
            package_version TEXT,
            downloaded_at TEXT NOT NULL DEFAULT (datetime('now')),
            client_ip_hash TEXT,
            user_agent TEXT
        );
CREATE INDEX idx_download_stats_package
            ON download_stats(source_profile, package_name);
CREATE INDEX idx_download_stats_time
            ON download_stats(downloaded_at);
CREATE TABLE download_counts (
            source_profile TEXT NOT NULL
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch', 'solus')),
            package_name TEXT NOT NULL,
            total_count INTEGER NOT NULL DEFAULT 0,
            count_30d INTEGER NOT NULL DEFAULT 0,
            count_7d INTEGER NOT NULL DEFAULT 0,
            last_updated TEXT,
            PRIMARY KEY (source_profile, package_name)
        );
CREATE INDEX idx_download_counts_popular
            ON download_counts(source_profile, total_count DESC);
CREATE TABLE remote_collections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            label TEXT NOT NULL DEFAULT '',
            version TEXT,
            content_hash TEXT NOT NULL,
            data_json TEXT NOT NULL,
            fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            repository_id INTEGER REFERENCES repositories(id), signature BLOB, signer_key_id TEXT,
            UNIQUE(name, label)
        );
CREATE INDEX idx_remote_collections_expires
            ON remote_collections(expires_at);
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
CREATE TABLE delta_manifests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_profile TEXT NOT NULL
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch', 'solus')),
            package_name TEXT NOT NULL,
            from_version TEXT NOT NULL,
            to_version TEXT NOT NULL,
            new_chunks TEXT NOT NULL,
            removed_chunks TEXT NOT NULL,
            download_size INTEGER NOT NULL DEFAULT 0,
            full_size INTEGER NOT NULL DEFAULT 0,
            computed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(source_profile, package_name, from_version, to_version)
        );
CREATE TABLE canonical_packages (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            appstream_id TEXT UNIQUE,
            description TEXT,
            kind TEXT NOT NULL DEFAULT 'package',
            category TEXT
        );
CREATE INDEX idx_canonical_packages_name
            ON canonical_packages(name);
CREATE TABLE package_implementations (
            id INTEGER PRIMARY KEY,
            canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id) ON DELETE CASCADE,
            distro TEXT NOT NULL
                CHECK(distro IN ('fedora-44', 'ubuntu-26.04', 'arch', 'solus')),
            distro_name TEXT NOT NULL,
            source TEXT NOT NULL
                CHECK(source IN ('contract', 'remi')),
            UNIQUE(canonical_id, distro)
        );
CREATE INDEX idx_pkg_impl_distro
            ON package_implementations(distro, distro_name);
CREATE INDEX idx_pkg_impl_canonical
            ON package_implementations(canonical_id);
CREATE TABLE repository_provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_package_id INTEGER NOT NULL,
            capability TEXT NOT NULL,
            version TEXT,
            version_relation TEXT
                CHECK(version_relation IN ('lt', 'le', 'eq', 'ge', 'gt')),
            kind TEXT NOT NULL DEFAULT 'package',
            raw TEXT,
            version_scheme TEXT NOT NULL
                CHECK(version_scheme IN ('conary', 'rpm', 'debian', 'arch', 'eopkg')),
            architecture_qualifier_kind TEXT NOT NULL
                CHECK(architecture_qualifier_kind IN ('implicit', 'any', 'exact')),
            architecture TEXT,
            provenance TEXT NOT NULL CHECK(json_valid(provenance)),
            CHECK(
                (version IS NULL AND version_relation IS NULL)
                OR
                (version IS NOT NULL AND version_relation IS NOT NULL)
            ),
            CHECK(
                (architecture_qualifier_kind = 'exact'
                    AND architecture IS NOT NULL
                    AND architecture <> '')
                OR
                (architecture_qualifier_kind IN ('implicit', 'any')
                    AND architecture IS NULL)
            ),
            FOREIGN KEY (repository_package_id) REFERENCES repository_packages(id) ON DELETE CASCADE
        );
CREATE INDEX idx_repository_provides_pkg
            ON repository_provides(repository_package_id);
-- Capability is the only column any provider lookup seeks on. A
-- (kind, capability) composite cannot serve a capability-only seek, and the one
-- statement that also filters kind seeks this index and filters the rows one
-- capability holds, so the composite is not carried.
CREATE INDEX idx_repository_provides_capability
            ON repository_provides(capability);
CREATE INDEX idx_repository_provides_raw
            ON repository_provides(raw)
            WHERE raw IS NOT NULL AND raw != '';
CREATE TABLE repository_requirement_groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_package_id INTEGER NOT NULL,
            kind TEXT NOT NULL DEFAULT 'depends',
            behavior TEXT NOT NULL DEFAULT 'hard',
            description TEXT,
            native_text TEXT,
            expression_json TEXT NOT NULL,
            FOREIGN KEY (repository_package_id) REFERENCES repository_packages(id) ON DELETE CASCADE
        );
CREATE INDEX idx_repo_req_groups_pkg
            ON repository_requirement_groups(repository_package_id);
CREATE TABLE repository_requirements (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_package_id INTEGER NOT NULL,
            group_id INTEGER NOT NULL REFERENCES repository_requirement_groups(id) ON DELETE CASCADE,
            capability TEXT NOT NULL,
            version_constraint TEXT,
            kind TEXT NOT NULL DEFAULT 'package',
            dependency_type TEXT NOT NULL DEFAULT 'runtime',
            raw TEXT,
            FOREIGN KEY (repository_package_id) REFERENCES repository_packages(id) ON DELETE CASCADE
        );
CREATE INDEX idx_repository_requirements_pkg
            ON repository_requirements(repository_package_id);
CREATE INDEX idx_repository_requirements_capability
            ON repository_requirements(capability);
CREATE INDEX idx_repository_requirements_kind_capability
            ON repository_requirements(kind, capability);
CREATE INDEX idx_repo_requirements_group
            ON repository_requirements(group_id);
CREATE INDEX idx_repo_req_pkg_kind
            ON repository_requirements(repository_package_id, kind);
CREATE TABLE remi_sparse_projection_revisions (
            source_profile TEXT PRIMARY KEY,
            sequence INTEGER NOT NULL CHECK(sequence >= 0)
        );
CREATE TRIGGER remi_sparse_revision_repositories_insert
AFTER INSERT ON repositories
WHEN NEW.enabled = 1 AND NEW.source_profile IS NOT NULL
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    VALUES (NEW.source_profile, 1)
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_repositories_delete
AFTER DELETE ON repositories
WHEN OLD.enabled = 1 AND OLD.source_profile IS NOT NULL
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    VALUES (OLD.source_profile, 1)
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_repositories_update_old
AFTER UPDATE OF enabled, source_profile ON repositories
WHEN OLD.enabled = 1 AND OLD.source_profile IS NOT NULL
     AND (OLD.enabled IS NOT NEW.enabled OR OLD.source_profile IS NOT NEW.source_profile)
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    VALUES (OLD.source_profile, 1)
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_repositories_update_new
AFTER UPDATE OF enabled, source_profile ON repositories
WHEN NEW.enabled = 1 AND NEW.source_profile IS NOT NULL
     AND (OLD.enabled IS NOT NEW.enabled OR OLD.source_profile IS NOT NEW.source_profile)
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    VALUES (NEW.source_profile, 1)
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_packages_insert
AFTER INSERT ON repository_packages
WHEN NEW.size >= 1
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repositories repository
    WHERE repository.id = NEW.repository_id
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_packages_delete
AFTER DELETE ON repository_packages
WHEN OLD.size >= 1
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repositories repository
    WHERE repository.id = OLD.repository_id
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_packages_update_old
AFTER UPDATE OF repository_id, name, version, package_release, architecture, size, metadata
ON repository_packages
WHEN OLD.size >= 1 AND (
    OLD.repository_id IS NOT NEW.repository_id
    OR OLD.name IS NOT NEW.name
    OR OLD.version IS NOT NEW.version
    OR OLD.package_release IS NOT NEW.package_release
    OR OLD.architecture IS NOT NEW.architecture
    OR OLD.size IS NOT NEW.size
    OR OLD.metadata IS NOT NEW.metadata
)
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repositories repository
    WHERE repository.id = OLD.repository_id
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_packages_update_new
AFTER UPDATE OF repository_id, name, version, package_release, architecture, size, metadata
ON repository_packages
WHEN NEW.size >= 1 AND (
    OLD.repository_id IS NOT NEW.repository_id
    OR OLD.name IS NOT NEW.name
    OR OLD.version IS NOT NEW.version
    OR OLD.package_release IS NOT NEW.package_release
    OR OLD.architecture IS NOT NEW.architecture
    OR OLD.size IS NOT NEW.size
    OR OLD.metadata IS NOT NEW.metadata
)
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repositories repository
    WHERE repository.id = NEW.repository_id
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_provides_insert
AFTER INSERT ON repository_provides
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id = NEW.repository_package_id
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_provides_delete
AFTER DELETE ON repository_provides
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id = OLD.repository_package_id
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_provides_update
AFTER UPDATE OF repository_package_id, capability, version, version_relation, kind, raw,
                version_scheme, architecture_qualifier_kind, architecture
ON repository_provides
WHEN OLD.repository_package_id IS NOT NEW.repository_package_id
     OR OLD.capability IS NOT NEW.capability
     OR OLD.version IS NOT NEW.version
     OR OLD.version_relation IS NOT NEW.version_relation
     OR OLD.kind IS NOT NEW.kind
     OR OLD.raw IS NOT NEW.raw
     OR OLD.version_scheme IS NOT NEW.version_scheme
     OR OLD.architecture_qualifier_kind IS NOT NEW.architecture_qualifier_kind
     OR OLD.architecture IS NOT NEW.architecture
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT DISTINCT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id IN (OLD.repository_package_id, NEW.repository_package_id)
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_requirement_groups_insert
AFTER INSERT ON repository_requirement_groups
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id = NEW.repository_package_id
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_requirement_groups_delete
AFTER DELETE ON repository_requirement_groups
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id = OLD.repository_package_id
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_requirement_groups_update
AFTER UPDATE OF repository_package_id, kind, behavior, description, native_text, expression_json
ON repository_requirement_groups
WHEN OLD.repository_package_id IS NOT NEW.repository_package_id
     OR OLD.kind IS NOT NEW.kind
     OR OLD.behavior IS NOT NEW.behavior
     OR OLD.description IS NOT NEW.description
     OR OLD.native_text IS NOT NEW.native_text
     OR OLD.expression_json IS NOT NEW.expression_json
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT DISTINCT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id IN (OLD.repository_package_id, NEW.repository_package_id)
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_requirements_insert
AFTER INSERT ON repository_requirements
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id = NEW.repository_package_id
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_requirements_delete
AFTER DELETE ON repository_requirements
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id = OLD.repository_package_id
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TRIGGER remi_sparse_revision_requirements_update
AFTER UPDATE OF repository_package_id, group_id, capability, version_constraint, kind,
                dependency_type, raw
ON repository_requirements
WHEN OLD.repository_package_id IS NOT NEW.repository_package_id
     OR OLD.group_id IS NOT NEW.group_id
     OR OLD.capability IS NOT NEW.capability
     OR OLD.version_constraint IS NOT NEW.version_constraint
     OR OLD.kind IS NOT NEW.kind
     OR OLD.dependency_type IS NOT NEW.dependency_type
     OR OLD.raw IS NOT NEW.raw
BEGIN
    INSERT INTO remi_sparse_projection_revisions(source_profile, sequence)
    SELECT DISTINCT repository.source_profile, 1
    FROM repository_packages package
    JOIN repositories repository ON repository.id = package.repository_id
    WHERE package.id IN (OLD.repository_package_id, NEW.repository_package_id)
      AND package.size >= 1
      AND repository.enabled = 1
      AND repository.source_profile IS NOT NULL
    ON CONFLICT(source_profile) DO UPDATE SET sequence = sequence + 1;
END;
CREATE TABLE repology_cache (
            project_name TEXT NOT NULL,
            distro TEXT NOT NULL,
            distro_name TEXT NOT NULL,
            version TEXT,
            status TEXT,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (project_name, distro)
        );
CREATE TABLE appstream_cache (
            appstream_id TEXT NOT NULL,
            pkgname TEXT NOT NULL,
            display_name TEXT,
            summary TEXT,
            distro TEXT NOT NULL,
            fetched_at TEXT NOT NULL,
            PRIMARY KEY (appstream_id, distro)
        );
CREATE TABLE derivation_index (
            derivation_id TEXT PRIMARY KEY,
            output_hash TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            manifest_cas_hash TEXT NOT NULL,
            stage TEXT,
            build_env_hash TEXT,
            built_at TEXT NOT NULL,
            build_duration_secs INTEGER NOT NULL
        , trust_level INTEGER NOT NULL DEFAULT 0, provenance_cas_hash TEXT, reproducible INTEGER);
CREATE INDEX idx_derivation_index_package
            ON derivation_index(package_name, package_version);
CREATE INDEX idx_derivation_index_output_hash
            ON derivation_index(output_hash);
CREATE TABLE substituter_peers (
            endpoint TEXT PRIMARY KEY,
            priority INTEGER NOT NULL DEFAULT 0,
            last_seen TEXT,
            success_count INTEGER NOT NULL DEFAULT 0,
            failure_count INTEGER NOT NULL DEFAULT 0
        );
CREATE TABLE derivation_cache (
            derivation_id TEXT PRIMARY KEY,
            manifest_cas_hash TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        , provenance_cas_hash TEXT);
CREATE INDEX idx_derivation_cache_package
            ON derivation_cache(package_name, package_version);
CREATE TABLE output_equivalence (
            package_name TEXT NOT NULL,
            output_hash TEXT NOT NULL,
            derivation_id TEXT NOT NULL,
            seed_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (package_name, output_hash, seed_id)
        );
CREATE INDEX idx_output_equivalence_hash
            ON output_equivalence(output_hash);
CREATE INDEX idx_repo_packages_canonical ON repository_packages(canonical_id);
CREATE TABLE appstream_provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            canonical_id INTEGER NOT NULL REFERENCES canonical_packages(id) ON DELETE CASCADE,
            provide_type TEXT NOT NULL,
            capability TEXT NOT NULL,
            UNIQUE(canonical_id, provide_type, capability)
        );
CREATE INDEX idx_appstream_provides_cap ON appstream_provides(capability);
CREATE INDEX idx_derived_packages_artifact_hash
            ON derived_packages(build_artifact_hash)
            WHERE build_artifact_hash IS NOT NULL;
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
CREATE UNIQUE INDEX idx_repo_packages_unique
            ON repository_packages(repository_id, name, version, package_release, architecture);
CREATE TABLE native_package_publications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository_id INTEGER NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
            repository_package_id INTEGER NOT NULL REFERENCES repository_packages(id) ON DELETE CASCADE,
            source_profile TEXT NOT NULL
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch', 'solus')),
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            package_release TEXT NOT NULL,
            architecture TEXT NOT NULL,
            package_kind TEXT NOT NULL,
            authority_format_version INTEGER NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('public', 'superseded', 'rolled_back')),
            content_hash TEXT NOT NULL,
            transport_json TEXT NOT NULL
                CHECK(json_valid(transport_json) AND json_type(transport_json) = 'object'),
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
            ON native_package_publications(source_profile, name, version, package_release, architecture)
            WHERE status = 'public';
CREATE INDEX idx_native_publications_repo_package
            ON native_package_publications(repository_package_id);
CREATE INDEX idx_native_publications_chunk_hash
            ON native_package_publications(content_hash);
