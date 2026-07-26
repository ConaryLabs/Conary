-- Current repository, federation, derivation, and TUF schema.
-- Part of the current pre-alpha schema epoch.

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
            source_profile TEXT
                CHECK(source_profile IS NULL OR source_profile IN (
                    'fedora-44', 'ubuntu-26.04', 'arch'
                )),
            tuf_enabled INTEGER NOT NULL DEFAULT 0, tuf_root_version INTEGER, tuf_root_url TEXT,
            security_advisory_support TEXT NOT NULL DEFAULT 'unknown'
            CHECK(security_advisory_support IN ('unknown', 'unsupported', 'supported')),
            package_format TEXT NOT NULL DEFAULT 'unspecified'
                CHECK(package_format IN ('arch', 'deb', 'rpm', 'json', 'unspecified')),
            parser_config_json TEXT,
            managed_by TEXT NOT NULL DEFAULT 'operator'
                CHECK(managed_by IN ('operator', 'remi-config')),
            CHECK(
                (package_format = 'unspecified' AND parser_config_json IS NULL)
                OR (package_format != 'unspecified' AND parser_config_json IS NOT NULL)
            ));
CREATE INDEX idx_repositories_name ON repositories(name);
CREATE INDEX idx_repositories_enabled ON repositories(enabled);
CREATE INDEX idx_repositories_priority ON repositories(priority);
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
            source_profile TEXT CHECK(source_profile IS NULL OR source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch')),
            version_scheme TEXT NOT NULL
                CHECK(version_scheme IN ('conary', 'rpm', 'debian', 'arch')), canonical_id INTEGER
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
            status TEXT NOT NULL DEFAULT 'pending',
            -- Built trove ID (when status = built)
            built_trove_id INTEGER REFERENCES troves(id) ON DELETE SET NULL,
            -- Model file this came from (NULL if created via CLI)
            model_source TEXT,
            -- Error message if status = error
            error_message TEXT
        , last_built_version TEXT, last_built_parent_version TEXT, build_artifact_hash TEXT, build_artifact_path TEXT, build_artifact_size INTEGER);
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
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch')),
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
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch')),
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
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch')),
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
                CHECK(distro IN ('fedora-44', 'ubuntu-26.04', 'arch')),
            distro_name TEXT NOT NULL,
            source TEXT NOT NULL
                CHECK(source IN ('contract', 'remi')),
            UNIQUE(canonical_id, distro)
        );
CREATE INDEX idx_pkg_impl_distro
            ON package_implementations(distro, distro_name);
CREATE INDEX idx_pkg_impl_canonical
            ON package_implementations(canonical_id);
CREATE TABLE distro_pin (
            id INTEGER PRIMARY KEY,
            distro TEXT NOT NULL
                CHECK(distro IN ('fedora-44', 'ubuntu-26.04', 'arch')),
            mixing_policy TEXT NOT NULL DEFAULT 'guarded'
                CHECK(mixing_policy IN ('strict', 'guarded', 'permissive')),
            created_at TEXT NOT NULL
        );
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
                CHECK(version_scheme IN ('conary', 'rpm', 'debian', 'arch')),
            architecture_qualifier_kind TEXT NOT NULL
                CHECK(architecture_qualifier_kind IN ('implicit', 'any', 'exact')),
            architecture TEXT,
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
CREATE INDEX idx_repository_provides_capability
            ON repository_provides(capability);
CREATE INDEX idx_repository_provides_kind_capability
            ON repository_provides(kind, capability);
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
                CHECK(source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch')),
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
            ON native_package_publications(source_profile, name, version, package_release, architecture)
            WHERE status = 'public';
CREATE INDEX idx_native_publications_repo_package
            ON native_package_publications(repository_package_id);
CREATE INDEX idx_native_publications_chunk_hash
            ON native_package_publications(content_hash);
