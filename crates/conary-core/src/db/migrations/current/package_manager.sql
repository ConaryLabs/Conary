-- Current local package-manager state schema.
-- Part of the single current pre-alpha schema epoch.

CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE schema_identity (
    epoch TEXT PRIMARY KEY,
    revision INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE troves (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            type TEXT NOT NULL CHECK(type IN ('package', 'component', 'collection')),
            architecture TEXT,
            description TEXT,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            installed_by_changeset_id INTEGER, install_source TEXT DEFAULT 'file', install_reason TEXT NOT NULL DEFAULT 'explicit', flavor_spec TEXT, pinned INTEGER NOT NULL DEFAULT 0, selection_reason TEXT, label_id INTEGER REFERENCES labels(id), orphan_since TEXT, source_distro TEXT, version_scheme TEXT, installed_from_repository_id INTEGER
            REFERENCES repositories(id) ON DELETE SET NULL,
            UNIQUE(name, version, architecture),
            FOREIGN KEY (installed_by_changeset_id) REFERENCES changesets(id)
        );
CREATE INDEX idx_troves_name ON troves(name);
CREATE INDEX idx_troves_type ON troves(type);
CREATE TABLE files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            sha256_hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            permissions INTEGER NOT NULL,
            owner TEXT,
            group_name TEXT,
            trove_id INTEGER NOT NULL,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, component_id INTEGER REFERENCES components(id) ON DELETE SET NULL, symlink_target TEXT,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );
CREATE INDEX idx_files_path ON files(path);
CREATE INDEX idx_files_trove_id ON files(trove_id);
CREATE INDEX idx_files_sha256 ON files(sha256_hash);
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
CREATE TABLE provenance (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL UNIQUE,
            source_url TEXT,
            source_branch TEXT,
            source_commit TEXT,
            build_host TEXT,
            build_time TEXT,
            builder TEXT, upstream_url TEXT, upstream_hash TEXT, git_repo TEXT, git_tag TEXT, fetch_timestamp TEXT, patches_json TEXT, recipe_hash TEXT, build_deps_json TEXT, host_arch TEXT, host_kernel TEXT, host_distro TEXT, build_start TEXT, build_end TEXT, build_log_hash TEXT, isolation_level TEXT, reproducibility_json TEXT, signatures_json TEXT, rekor_log_index INTEGER, sbom_spdx_hash TEXT, sbom_cyclonedx_hash TEXT, merkle_root TEXT, component_hashes_json TEXT, chunk_manifest_json TEXT, total_size INTEGER, file_count INTEGER, dna_hash TEXT,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );
CREATE INDEX idx_provenance_trove_id ON provenance(trove_id);
CREATE TABLE dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL,
            depends_on_name TEXT NOT NULL,
            depends_on_version TEXT,
            dependency_type TEXT NOT NULL CHECK(dependency_type IN ('runtime', 'build', 'optional')),
            version_constraint TEXT, kind TEXT DEFAULT 'package', group_id INTEGER,
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE
        );
CREATE INDEX idx_dependencies_trove_id ON dependencies(trove_id);
CREATE INDEX idx_dependencies_depends_on ON dependencies(depends_on_name);
CREATE TABLE file_contents (
            sha256_hash TEXT PRIMARY KEY,
            content_path TEXT NOT NULL,
            size INTEGER NOT NULL,
            stored_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
CREATE INDEX idx_file_contents_stored_at ON file_contents(stored_at);
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
CREATE TABLE provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            capability TEXT NOT NULL,
            version TEXT, kind TEXT DEFAULT 'package', canonical_id INTEGER REFERENCES canonical_packages(id),
            UNIQUE(trove_id, capability)
        );
CREATE INDEX idx_provides_capability ON provides(capability);
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
CREATE INDEX idx_scriptlets_trove ON scriptlets(trove_id);
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
CREATE INDEX idx_files_component ON files(component_id);
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
CREATE TABLE component_provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            component_id INTEGER NOT NULL REFERENCES components(id) ON DELETE CASCADE,
            capability TEXT NOT NULL,
            version TEXT,
            UNIQUE(component_id, capability)
        );
CREATE INDEX idx_component_provides_component ON component_provides(component_id);
CREATE INDEX idx_component_provides_capability ON component_provides(capability);
CREATE INDEX idx_troves_install_reason ON troves(install_reason);
CREATE INDEX idx_troves_flavor ON troves(flavor_spec);
CREATE INDEX idx_troves_pinned ON troves(pinned) WHERE pinned = 1;
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
CREATE TABLE trigger_dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trigger_id INTEGER NOT NULL REFERENCES triggers(id) ON DELETE CASCADE,
            depends_on TEXT NOT NULL,
            UNIQUE(trigger_id, depends_on)
        );
CREATE INDEX idx_trigger_deps_trigger ON trigger_dependencies(trigger_id);
CREATE INDEX idx_trigger_deps_depends ON trigger_dependencies(depends_on);
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
INSERT INTO triggers (name, description, pattern, handler, priority, builtin)
VALUES
    ('ldconfig', 'Update shared library cache', '/usr/lib/*.so*,/usr/lib64/*.so*,/lib/*.so*,/lib64/*.so*', '/sbin/ldconfig', 10, 1),
    ('update-mime-database', 'Update MIME type database', '/usr/share/mime/*', 'update-mime-database /usr/share/mime', 30, 1),
    ('update-desktop-database', 'Update desktop entry database', '/usr/share/applications/*.desktop', 'update-desktop-database /usr/share/applications', 30, 1),
    ('gtk-update-icon-cache', 'Update GTK icon cache', '/usr/share/icons/*', 'gtk-update-icon-cache -f /usr/share/icons/hicolor', 40, 1),
    ('glib-compile-schemas', 'Compile GSettings schemas', '/usr/share/glib-2.0/schemas/*.xml,/usr/share/glib-2.0/schemas/*.gschema.override', 'glib-compile-schemas /usr/share/glib-2.0/schemas', 30, 1),
    ('systemd-tmpfiles', 'Create tmpfiles entries', '/usr/lib/tmpfiles.d/*.conf', 'systemd-tmpfiles --create', 20, 1),
    ('systemd-sysusers', 'Create system users', '/usr/lib/sysusers.d/*.conf', 'systemd-sysusers', 15, 1),
    ('systemctl-daemon-reload', 'Reload systemd daemon', '/usr/lib/systemd/system/*,/usr/lib/systemd/user/*', 'systemctl daemon-reload', 50, 1),
    ('fc-cache', 'Update font cache', '/usr/share/fonts/*', 'fc-cache -s', 40, 1),
    ('depmod', 'Update kernel module dependencies', '/lib/modules/*/modules.*,/usr/lib/modules/*/*.ko*', 'depmod -a', 20, 1);
INSERT INTO trigger_dependencies (trigger_id, depends_on)
SELECT id, 'systemd-sysusers'
FROM triggers
WHERE name = 'systemd-tmpfiles';
CREATE TABLE system_states (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            state_number INTEGER NOT NULL UNIQUE,
            summary TEXT NOT NULL,
            description TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            is_active INTEGER NOT NULL DEFAULT 0,
            package_count INTEGER NOT NULL DEFAULT 0
        , base_generation INTEGER);
CREATE INDEX idx_system_states_number ON system_states(state_number);
CREATE INDEX idx_system_states_active ON system_states(is_active) WHERE is_active = 1;
CREATE INDEX idx_system_states_created ON system_states(created_at);
CREATE INDEX idx_dependencies_kind ON dependencies(kind);
CREATE INDEX idx_dependencies_kind_name ON dependencies(kind, depends_on_name);
CREATE INDEX idx_provides_kind ON provides(kind);
CREATE INDEX idx_provides_kind_capability ON provides(kind, capability);
CREATE TABLE labels (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repository TEXT NOT NULL,
            namespace TEXT NOT NULL,
            tag TEXT NOT NULL,
            description TEXT,
            parent_label_id INTEGER REFERENCES labels(id),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, repository_id INTEGER REFERENCES repositories(id), delegate_to_label_id INTEGER REFERENCES labels(id),
            UNIQUE(repository, namespace, tag)
        );
CREATE INDEX idx_labels_full ON labels(repository, namespace, tag);
CREATE INDEX idx_labels_repo ON labels(repository);
CREATE TABLE label_path (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            label_id INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
            priority INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1,
            UNIQUE(label_id)
        );
CREATE INDEX idx_label_path_priority ON label_path(priority) WHERE enabled = 1;
CREATE INDEX idx_troves_label ON troves(label_id);
CREATE TABLE config_files (
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
CREATE TABLE config_backups (
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
CREATE INDEX idx_labels_repository ON labels(repository_id) WHERE repository_id IS NOT NULL;
CREATE INDEX idx_labels_delegate ON labels(delegate_to_label_id) WHERE delegate_to_label_id IS NOT NULL;
CREATE UNIQUE INDEX idx_provenance_dna ON provenance(dna_hash) WHERE dna_hash IS NOT NULL;
CREATE INDEX idx_provenance_deps ON provenance(build_deps_json) WHERE build_deps_json IS NOT NULL;
CREATE INDEX idx_provenance_rekor ON provenance(rekor_log_index) WHERE rekor_log_index IS NOT NULL;
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
CREATE INDEX idx_subpackage_base ON subpackage_relationships(base_package);
CREATE INDEX idx_subpackage_name ON subpackage_relationships(subpackage_name);
CREATE INDEX idx_subpackage_component ON subpackage_relationships(component_type);
CREATE INDEX idx_troves_orphan_since ON troves(orphan_since)
            WHERE orphan_since IS NOT NULL;
CREATE TABLE system_affinity (
            distro TEXT PRIMARY KEY,
            package_count INTEGER NOT NULL DEFAULT 0,
            percentage REAL NOT NULL DEFAULT 0.0,
            updated_at TEXT NOT NULL
        );
CREATE TABLE settings (
            key   TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
CREATE INDEX idx_provides_trove_cap
            ON provides(trove_id, capability);
CREATE TABLE "state_members" (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            state_id INTEGER NOT NULL REFERENCES system_states(id) ON DELETE CASCADE,
            trove_name TEXT NOT NULL,
            trove_version TEXT NOT NULL,
            architecture TEXT,
            install_reason TEXT NOT NULL DEFAULT 'explicit',
            selection_reason TEXT,
            UNIQUE(state_id, trove_name, architecture)
        );
CREATE INDEX idx_state_members_state ON state_members(state_id);
CREATE INDEX idx_state_members_name ON state_members(trove_name);
CREATE INDEX idx_files_symlink ON files(id) WHERE symlink_target IS NOT NULL;
CREATE TABLE state_cas_hashes (
            state_id INTEGER NOT NULL REFERENCES system_states(id) ON DELETE CASCADE,
            sha256_hash TEXT NOT NULL,
            UNIQUE(state_id, sha256_hash)
        );
CREATE INDEX idx_state_cas_hashes_state ON state_cas_hashes(state_id);
CREATE INDEX idx_dependencies_group ON dependencies(trove_id, group_id);
CREATE TABLE "changesets" (
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
CREATE INDEX idx_changesets_status ON changesets(status);
CREATE INDEX idx_changesets_created_at ON changesets(created_at);
CREATE UNIQUE INDEX idx_changesets_tx_uuid
            ON changesets(tx_uuid) WHERE tx_uuid IS NOT NULL;
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
CREATE TABLE installed_file_capabilities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            capabilities_json TEXT NOT NULL,
            permitted INTEGER NOT NULL DEFAULT 1,
            effective INTEGER NOT NULL DEFAULT 1,
            inheritable INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(trove_id, path)
        );
CREATE INDEX idx_installed_file_capabilities_trove
            ON installed_file_capabilities(trove_id);
CREATE INDEX idx_installed_file_capabilities_path
            ON installed_file_capabilities(path);
