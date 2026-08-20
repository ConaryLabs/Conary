-- Current local package-manager state schema.
-- Part of the current pre-alpha schema epoch.

CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE schema_identity (
    epoch TEXT PRIMARY KEY,
    revision INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE generation_db_mutation_epoch (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    token TEXT NOT NULL CHECK(length(token) = 36)
);
INSERT INTO generation_db_mutation_epoch (singleton, token)
VALUES (1, '00000000-0000-0000-0000-000000000000');
CREATE TABLE troves (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            package_release TEXT
                CHECK(package_release IS NULL OR length(package_release) > 0),
            type TEXT NOT NULL CHECK(type IN ('package', 'component', 'collection')),
            architecture TEXT CHECK(architecture IS NULL OR length(architecture) > 0),
            description TEXT,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            installed_by_changeset_id INTEGER,
            install_source TEXT NOT NULL
                CHECK (install_source IN (
                    'file', 'repository', 'adopted-track', 'adopted-full', 'taken',
                    'captured-root'
                )),
            install_reason TEXT NOT NULL
                CHECK (install_reason IN ('explicit', 'dependency')),
            flavor_spec TEXT,
            pinned INTEGER NOT NULL DEFAULT 0,
            selection_reason TEXT,
            label_id INTEGER REFERENCES labels(id),
            orphan_since TEXT,
            source_profile TEXT
                CHECK(source_profile IS NULL OR source_profile IN (
                    'fedora-44', 'ubuntu-26.04', 'arch', 'solus'
                )),
            version_scheme TEXT NOT NULL
                CHECK(
                    version_scheme IN ('conary', 'rpm', 'debian', 'arch', 'eopkg')
                    AND (
                        source_profile IS NULL
                        OR (
                            source_profile = 'fedora-44'
                            AND version_scheme = 'rpm'
                        )
                        OR (
                            source_profile = 'ubuntu-26.04'
                            AND version_scheme = 'debian'
                        )
                        OR (
                            source_profile = 'arch'
                            AND version_scheme = 'arch'
                        )
                        OR (
                            source_profile = 'solus'
                            AND version_scheme = 'eopkg'
                        )
                    )
                ),
            native_package_identity_json TEXT,
            installed_from_repository_id INTEGER REFERENCES repositories(id) ON DELETE SET NULL,
            debian_multi_arch TEXT
                CHECK(debian_multi_arch IN ('no', 'same', 'allowed', 'foreign')),
            CHECK(
                (version_scheme = 'debian' AND debian_multi_arch IS NOT NULL)
                OR (version_scheme != 'debian' AND debian_multi_arch IS NULL)
            ),
            CHECK (
                CASE
                    WHEN install_source IN ('adopted-track', 'adopted-full', 'taken') THEN
                        CASE
                            WHEN native_package_identity_json IS NOT NULL
                                 AND json_valid(native_package_identity_json)
                            THEN (
                                json_type(native_package_identity_json) = 'object'
                                AND json_extract(
                                    native_package_identity_json, '$.manager'
                                ) IN ('rpm', 'dpkg', 'pacman', 'eopkg')
                                AND json_type(
                                    native_package_identity_json, '$.selector'
                                ) = 'text'
                                AND length(json_extract(
                                    native_package_identity_json, '$.selector'
                                )) > 0
                                AND json_type(
                                    native_package_identity_json, '$.name'
                                ) = 'text'
                                AND length(json_extract(
                                    native_package_identity_json, '$.name'
                                )) > 0
                                AND json_type(
                                    native_package_identity_json, '$.version'
                                ) = 'text'
                                AND length(json_extract(
                                    native_package_identity_json, '$.version'
                                )) > 0
                                AND json_type(
                                    native_package_identity_json, '$.architecture'
                                ) = 'text'
                                AND length(json_extract(
                                    native_package_identity_json, '$.architecture'
                                )) > 0
                                AND json_extract(
                                    native_package_identity_json, '$.name'
                                ) = name
                                AND json_extract(
                                    native_package_identity_json, '$.architecture'
                                ) = architecture
                                AND CASE json_extract(
                                    native_package_identity_json, '$.manager'
                                )
                                    WHEN 'rpm' THEN (
                                        CASE
                                            WHEN json_type(
                                                native_package_identity_json, '$.epoch'
                                            ) = 'integer'
                                            AND json_extract(
                                                native_package_identity_json, '$.epoch'
                                            ) > 0
                                            THEN CAST(json_extract(
                                                native_package_identity_json, '$.epoch'
                                            ) AS TEXT) || ':'
                                            ELSE ''
                                        END
                                        || json_extract(
                                            native_package_identity_json, '$.version'
                                        )
                                        || '-'
                                        || json_extract(
                                            native_package_identity_json, '$.release'
                                        )
                                    ) = version
                                    WHEN 'dpkg' THEN json_extract(
                                        native_package_identity_json, '$.version'
                                    ) = version
                                    WHEN 'pacman' THEN json_extract(
                                        native_package_identity_json, '$.version'
                                    ) = version
                                    WHEN 'eopkg' THEN (
                                        json_extract(
                                            native_package_identity_json, '$.version'
                                        )
                                        || '-'
                                        || CAST(json_extract(
                                            native_package_identity_json, '$.release'
                                        ) AS TEXT)
                                    ) = version
                                    ELSE 0
                                END
                                AND CASE json_extract(
                                    native_package_identity_json, '$.manager'
                                )
                                    WHEN 'rpm' THEN
                                        json_type(
                                            native_package_identity_json, '$.release'
                                        ) = 'text'
                                        AND length(json_extract(
                                            native_package_identity_json, '$.release'
                                        )) > 0
                                        AND json_type(
                                            native_package_identity_json, '$.epoch'
                                        ) IN ('integer', 'null')
                                        AND json_extract(
                                            native_package_identity_json, '$.selector'
                                        ) = (
                                            json_extract(
                                                native_package_identity_json, '$.name'
                                            )
                                            || '-'
                                            || CASE
                                                WHEN json_type(
                                                    native_package_identity_json, '$.epoch'
                                                ) = 'integer'
                                                THEN CAST(json_extract(
                                                    native_package_identity_json, '$.epoch'
                                                ) AS TEXT) || ':'
                                                ELSE ''
                                            END
                                            || json_extract(
                                                native_package_identity_json, '$.version'
                                            )
                                            || '-'
                                            || json_extract(
                                                native_package_identity_json, '$.release'
                                            )
                                            || '.'
                                            || json_extract(
                                                native_package_identity_json, '$.architecture'
                                            )
                                        )
                                    WHEN 'dpkg' THEN (
                                        json_extract(
                                            native_package_identity_json, '$.selector'
                                        ) = json_extract(
                                            native_package_identity_json, '$.name'
                                        )
                                        OR json_extract(
                                            native_package_identity_json, '$.selector'
                                        ) = (
                                            json_extract(
                                                native_package_identity_json, '$.name'
                                            )
                                            || ':'
                                            || json_extract(
                                                native_package_identity_json, '$.architecture'
                                            )
                                        )
                                    )
                                    WHEN 'pacman' THEN
                                        json_extract(
                                            native_package_identity_json, '$.selector'
                                        ) = json_extract(
                                            native_package_identity_json, '$.name'
                                        )
                                    WHEN 'eopkg' THEN
                                        json_type(
                                            native_package_identity_json, '$.release'
                                        ) = 'integer'
                                        AND json_extract(
                                            native_package_identity_json, '$.release'
                                        ) >= 0
                                        AND json_extract(
                                                native_package_identity_json, '$.selector'
                                            ) = json_extract(
                                                native_package_identity_json, '$.name'
                                            )
                                    ELSE 0
                                END
                            ) IS TRUE
                            ELSE 0
                        END
                    WHEN install_source IN ('file', 'repository', 'captured-root') THEN
                        native_package_identity_json IS NULL
                    ELSE 0
                END
            ),
            FOREIGN KEY (installed_by_changeset_id) REFERENCES changesets(id)
        );
CREATE INDEX idx_troves_name ON troves(name);
CREATE INDEX idx_troves_type ON troves(type);
CREATE UNIQUE INDEX idx_troves_exact_identity
ON troves(name, version, COALESCE(package_release, ''), COALESCE(architecture, ''));
CREATE TABLE files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL UNIQUE,
            payload_node_json TEXT NOT NULL,
            content_sha256 TEXT,
            content_size INTEGER,
            trove_id INTEGER NOT NULL,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            component_id INTEGER,
            CHECK (
                (content_sha256 IS NULL AND content_size IS NULL)
                OR (content_sha256 IS NOT NULL AND content_size IS NOT NULL AND content_size >= 0)
            ),
            FOREIGN KEY (trove_id) REFERENCES troves(id) ON DELETE CASCADE,
            FOREIGN KEY (component_id, trove_id)
                REFERENCES components(id, parent_trove_id)
        );
CREATE INDEX idx_files_path ON files(path);
CREATE INDEX idx_files_trove_id ON files(trove_id);
CREATE INDEX idx_files_content_sha256 ON files(content_sha256)
            WHERE content_sha256 IS NOT NULL;
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
            version TEXT,
            version_relation TEXT
                CHECK(version_relation IN ('lt', 'le', 'eq', 'ge', 'gt')),
            kind TEXT NOT NULL CHECK(kind IN (
                'package', 'virtual', 'soname', 'file',
                'path', 'binary', 'pkgconfig', 'pkgconfig32', 'comar', 'generic'
            )),
            version_scheme TEXT NOT NULL
                CHECK(version_scheme IN ('conary', 'rpm', 'debian', 'arch', 'eopkg')),
            architecture_qualifier_kind TEXT NOT NULL
                CHECK(architecture_qualifier_kind IN ('implicit', 'any', 'exact')),
            architecture_qualifier TEXT,
            provenance TEXT NOT NULL CHECK(json_valid(provenance)),
            canonical_id INTEGER REFERENCES canonical_packages(id),
            CHECK(
                (version IS NULL AND version_relation IS NULL)
                OR
                (version IS NOT NULL AND version_relation IS NOT NULL)
            ),
            CHECK(
                (architecture_qualifier_kind = 'exact'
                    AND architecture_qualifier IS NOT NULL
                    AND length(architecture_qualifier) > 0)
                OR
                (architecture_qualifier_kind IN ('implicit', 'any')
                    AND architecture_qualifier IS NULL)
            )
        );
CREATE INDEX idx_provides_capability ON provides(capability);
CREATE UNIQUE INDEX idx_provides_exact_contract
        ON provides(
            trove_id, kind, capability, COALESCE(version, ''),
            COALESCE(version_relation, ''), version_scheme,
            architecture_qualifier_kind, COALESCE(architecture_qualifier, ''),
            provenance
        );
CREATE TABLE package_requirement_groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK(kind IN (
                'depends', 'pre_depends', 'optional', 'build',
                'conflict', 'breaks', 'replace', 'obsolete'
            )),
            version_scheme TEXT NOT NULL
                CHECK(version_scheme IN ('conary', 'rpm', 'debian', 'arch', 'eopkg')),
            requirement_json TEXT NOT NULL,
            UNIQUE(trove_id, kind, requirement_json)
        );
CREATE INDEX idx_package_requirement_groups_trove
    ON package_requirement_groups(trove_id);
CREATE INDEX idx_package_requirement_groups_kind
    ON package_requirement_groups(kind);
CREATE TABLE installed_ccs_remove_hooks (
            trove_id INTEGER PRIMARY KEY REFERENCES troves(id) ON DELETE CASCADE,
            script TEXT NOT NULL,
            reversible INTEGER
                CHECK (reversible IS NULL OR reversible IN (0, 1))
        );
CREATE TABLE components (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            parent_trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            description TEXT,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            is_installed INTEGER NOT NULL DEFAULT 1,
            UNIQUE(parent_trove_id, name),
            UNIQUE(id, parent_trove_id)
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
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
CREATE INDEX idx_triggers_name ON triggers(name);
CREATE INDEX idx_triggers_enabled ON triggers(enabled) WHERE enabled = 1;
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
            status TEXT NOT NULL DEFAULT 'pending'
                CHECK(status IN ('pending', 'running', 'completed', 'failed', 'skipped')),
            matched_files INTEGER NOT NULL DEFAULT 0,
            started_at TEXT,
            completed_at TEXT,
            output TEXT,
            UNIQUE(changeset_id, trigger_id)
        );
CREATE INDEX idx_changeset_triggers_changeset ON changeset_triggers(changeset_id);
CREATE INDEX idx_changeset_triggers_status ON changeset_triggers(status);
-- Trigger rows are explicit operator authority. Package path spelling does
-- not seed or infer lifecycle mutation handlers.
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
            file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
            path TEXT NOT NULL,
            trove_id INTEGER REFERENCES troves(id) ON DELETE SET NULL,
            -- Stable owner identity remains after Debian package removal so
            -- conffile state can survive until an explicit purge/reinstall.
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            package_architecture TEXT,
            -- Hash of the file as shipped by the package. RPM %ghost config
            -- entries own a path without shipping content.
            original_hash TEXT,
            -- Exact Debian Conffiles compatibility digest. This is not
            -- integrity authority; original_hash remains authoritative.
            original_md5 TEXT,
            -- Current hash on filesystem (NULL if not checked)
            current_hash TEXT,
            -- If true, preserve user's version on upgrade (like RPM %config(noreplace))
            noreplace INTEGER NOT NULL DEFAULT 0 CHECK(noreplace IN (0, 1)),
            -- RPM %ghost ownership: never create or back up, erase if present.
            ghost INTEGER NOT NULL DEFAULT 0 CHECK(ghost IN (0, 1)),
            -- Declaration-to-payload association; false retains source
            -- authority without inventing a payload node.
            materialized INTEGER NOT NULL DEFAULT 1
                CHECK(materialized IN (0, 1))
                CHECK(ghost = 0 OR materialized = 0),
            -- Debian control declaration retaining the obsolete path identity
            -- while removing it during the next upgrade.
            remove_on_upgrade INTEGER NOT NULL DEFAULT 0
                CHECK(remove_on_upgrade IN (0, 1))
                CHECK(remove_on_upgrade = 0 OR materialized = 0),
            -- Status: pristine (unchanged), modified (user changed), missing (deleted)
            status TEXT NOT NULL DEFAULT 'pristine',
            -- When the modification was detected
            modified_at TEXT,
            -- Package source that declared this as config (rpm, deb, arch, auto)
            source TEXT NOT NULL DEFAULT 'auto'
                CHECK(source IN ('rpm', 'deb', 'arch', 'eopkg', 'auto'))
                CHECK(ghost = 0 OR source = 'rpm')
                CHECK(remove_on_upgrade = 0 OR source = 'deb'),
            CHECK(ghost = 0 OR remove_on_upgrade = 0),
            UNIQUE(path)
        );
CREATE INDEX idx_config_files_path ON config_files(path);
CREATE INDEX idx_config_files_trove ON config_files(trove_id);
CREATE INDEX idx_config_files_package ON config_files(package_name, package_architecture);
CREATE INDEX idx_config_files_status ON config_files(status);
CREATE TABLE debian_alternative_groups (
            name TEXT PRIMARY KEY,
            mode TEXT NOT NULL CHECK(mode IN ('auto', 'manual')),
            master_link TEXT NOT NULL,
            selected_path TEXT NOT NULL
        );
CREATE TABLE debian_alternative_slaves (
            group_name TEXT NOT NULL
                REFERENCES debian_alternative_groups(name) ON DELETE CASCADE,
            name TEXT NOT NULL,
            link TEXT NOT NULL,
            PRIMARY KEY(group_name, name),
            UNIQUE(group_name, link)
        );
CREATE TABLE debian_alternative_choices (
            group_name TEXT NOT NULL
                REFERENCES debian_alternative_groups(name) ON DELETE CASCADE,
            path TEXT NOT NULL,
            priority INTEGER NOT NULL
                CHECK(priority BETWEEN -2147483648 AND 2147483647),
            PRIMARY KEY(group_name, path)
        );
CREATE TABLE debian_alternative_choice_slaves (
            group_name TEXT NOT NULL,
            choice_path TEXT NOT NULL,
            slave_name TEXT NOT NULL,
            target TEXT NOT NULL,
            PRIMARY KEY(group_name, choice_path, slave_name),
            FOREIGN KEY(group_name, choice_path)
                REFERENCES debian_alternative_choices(group_name, path)
                ON DELETE CASCADE,
            FOREIGN KEY(group_name, slave_name)
                REFERENCES debian_alternative_slaves(group_name, name)
                ON DELETE CASCADE
        );
CREATE TABLE debian_debconf_templates (
            name TEXT PRIMARY KEY CHECK(name <> ''),
            template_type TEXT NOT NULL CHECK(template_type <> '')
        );
CREATE TABLE debian_debconf_template_fields (
            template_name TEXT NOT NULL
                REFERENCES debian_debconf_templates(name) ON DELETE CASCADE,
            name TEXT NOT NULL CHECK(name <> ''),
            value BLOB NOT NULL,
            PRIMARY KEY(template_name, name)
        );
CREATE TABLE debian_debconf_questions (
            name TEXT PRIMARY KEY CHECK(name <> ''),
            template_name TEXT NOT NULL
                REFERENCES debian_debconf_templates(name) ON DELETE RESTRICT,
            value BLOB,
            seen INTEGER NOT NULL CHECK(seen IN (0, 1))
        );
CREATE TABLE debian_debconf_question_owners (
            question_name TEXT NOT NULL
                REFERENCES debian_debconf_questions(name) ON DELETE CASCADE,
            owner TEXT NOT NULL CHECK(owner <> ''),
            PRIMARY KEY(question_name, owner)
        );
CREATE TABLE debian_debconf_flags (
            question_name TEXT NOT NULL
                REFERENCES debian_debconf_questions(name) ON DELETE CASCADE,
            name TEXT NOT NULL CHECK(name <> ''),
            value INTEGER NOT NULL CHECK(value IN (0, 1)),
            PRIMARY KEY(question_name, name)
        );
CREATE TABLE debian_debconf_substitutions (
            question_name TEXT NOT NULL
                REFERENCES debian_debconf_questions(name) ON DELETE CASCADE,
            name TEXT NOT NULL CHECK(name <> ''),
            value BLOB NOT NULL,
            PRIMARY KEY(question_name, name)
        );
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
            kind TEXT NOT NULL                      -- executable daemon jobs only
                CHECK (kind IN ('install', 'remove', 'update', 'enhance')),
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
            source_identity TEXT PRIMARY KEY
                CHECK(length(source_identity) BETWEEN 1 AND 255
                    AND trim(source_identity) = source_identity),
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
            package_release TEXT
                CHECK(package_release IS NULL OR length(package_release) > 0),
            architecture TEXT CHECK(architecture IS NULL OR length(architecture) > 0),
            install_reason TEXT NOT NULL,
            selection_reason TEXT
        );
CREATE UNIQUE INDEX idx_state_members_exact_identity
ON state_members(
    state_id,
    trove_name,
    trove_version,
    COALESCE(package_release, ''),
    COALESCE(architecture, '')
);
CREATE INDEX idx_state_members_state ON state_members(state_id);
CREATE INDEX idx_state_members_name ON state_members(trove_name);
CREATE TABLE selected_root_snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            parent_id INTEGER REFERENCES selected_root_snapshots(id) ON DELETE RESTRICT,
            root_node_json TEXT NOT NULL CHECK(json_valid(root_node_json)),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            CHECK(parent_id IS NULL OR parent_id < id)
        );
CREATE INDEX idx_selected_root_snapshots_parent
            ON selected_root_snapshots(parent_id);
CREATE TABLE selected_root_snapshot_entries (
            snapshot_id INTEGER NOT NULL
                REFERENCES selected_root_snapshots(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            remove_self INTEGER NOT NULL DEFAULT 0 CHECK(remove_self IN (0, 1)),
            remove_descendants INTEGER NOT NULL DEFAULT 0
                CHECK(remove_descendants IN (0, 1)),
            entry_json TEXT CHECK(entry_json IS NULL OR json_valid(entry_json)),
            hardlink_identity TEXT,
            PRIMARY KEY (snapshot_id, path),
            CHECK (
                remove_self = 1
                OR remove_descendants = 1
                OR entry_json IS NOT NULL
            ),
            CHECK (
                hardlink_identity IS NULL
                OR (entry_json IS NOT NULL AND length(hardlink_identity) > 0)
            ),
            CHECK (substr(path, 1, 1) = '/'),
            CHECK (
                entry_json IS NULL
                OR json_extract(entry_json, '$.path') = path
            )
        );
CREATE INDEX idx_selected_root_entries_path
            ON selected_root_snapshot_entries(path, snapshot_id);
CREATE INDEX idx_selected_root_entries_hardlink
            ON selected_root_snapshot_entries(hardlink_identity, snapshot_id, path)
            WHERE hardlink_identity IS NOT NULL;
CREATE TRIGGER selected_root_snapshots_no_update
BEFORE UPDATE ON selected_root_snapshots
BEGIN
    SELECT RAISE(ABORT, 'selected-root snapshots are append-only');
END;
CREATE TRIGGER selected_root_snapshots_no_delete
BEFORE DELETE ON selected_root_snapshots
BEGIN
    SELECT RAISE(ABORT, 'selected-root snapshots are append-only');
END;
CREATE TRIGGER selected_root_snapshot_entries_no_update
BEFORE UPDATE ON selected_root_snapshot_entries
BEGIN
    SELECT RAISE(ABORT, 'selected-root snapshot entries are append-only');
END;
CREATE TRIGGER selected_root_snapshot_entries_no_delete
BEFORE DELETE ON selected_root_snapshot_entries
BEGIN
    SELECT RAISE(ABORT, 'selected-root snapshot entries are append-only');
END;
CREATE TABLE "changesets" (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            description TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'mutation' CHECK(
                kind IN ('mutation', 'rollback')
            ),
            status TEXT NOT NULL CHECK(
                status IN ('pending', 'applied', 'rolled_back')
            ),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            applied_at TEXT,
            rolled_back_at TEXT,
            reversed_by_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            reverts_changeset_id INTEGER REFERENCES changesets(id),
            tx_uuid TEXT,
            rollback_selected_root_snapshot_id INTEGER
                REFERENCES selected_root_snapshots(id) ON DELETE RESTRICT,
            metadata TEXT,
            CHECK (
                (kind = 'mutation' AND reverts_changeset_id IS NULL)
                OR (kind = 'rollback' AND reverts_changeset_id IS NOT NULL)
            ),
            CHECK (kind = 'mutation' OR reversed_by_changeset_id IS NULL),
            CHECK (kind = 'mutation' OR status != 'rolled_back')
        );
CREATE INDEX idx_changesets_status ON changesets(status);
CREATE INDEX idx_changesets_created_at ON changesets(created_at);
CREATE UNIQUE INDEX idx_changesets_tx_uuid
            ON changesets(tx_uuid) WHERE tx_uuid IS NOT NULL;
CREATE UNIQUE INDEX idx_changesets_reverts
            ON changesets(reverts_changeset_id)
            WHERE reverts_changeset_id IS NOT NULL;
CREATE TABLE generation_publications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trigger_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            published_through_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            tx_uuid TEXT,
            selected_root_snapshot_id INTEGER
                REFERENCES selected_root_snapshots(id) ON DELETE RESTRICT,
            db_path TEXT NOT NULL,
            runtime_root TEXT NOT NULL,
            phase TEXT NOT NULL CHECK(phase IN (
                'pending_build',
                'building',
                'artifact_ready',
                'current_published',
                'configuration_projected',
                'active_marked',
                'database_backed_up'
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
            config_transaction_json TEXT NOT NULL DEFAULT '{"schema_version":5,"entries":[]}',
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
CREATE TABLE activation_requests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            changeset_id INTEGER NOT NULL REFERENCES changesets(id) ON DELETE CASCADE,
            sequence INTEGER NOT NULL CHECK(sequence >= 0),
            source_kind TEXT NOT NULL CHECK(source_kind IN (
                'captured-systemctl',
                'captured-openrc',
                'ccs-service',
                'captured-selinux',
                'captured-apparmor',
                'captured-boot-runtime'
            )),
            source_package TEXT NOT NULL,
            source_version TEXT NOT NULL,
            source_entry TEXT NOT NULL,
            invocation_json TEXT NOT NULL,
            invocation_sha256 TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(changeset_id, sequence)
        );
CREATE INDEX idx_activation_requests_changeset
            ON activation_requests(changeset_id, sequence);
CREATE INDEX idx_activation_requests_digest
            ON activation_requests(invocation_sha256);
CREATE TABLE lifecycle_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            changeset_id INTEGER NOT NULL REFERENCES changesets(id) ON DELETE CASCADE,
            sequence INTEGER NOT NULL CHECK(sequence >= 0),
            source_package TEXT NOT NULL,
            source_version TEXT NOT NULL,
            source_entry TEXT NOT NULL,
            failure_kind TEXT NOT NULL CHECK(failure_kind IN (
                'ScriptExited',
                'ScriptTimedOut',
                'ContractViolation',
                'ProgramUnavailable',
                'ProcessSetupFailed',
                'SandboxSetupUnavailable',
                'EnforcementSetupFailed'
            )),
            requested_sandbox_mode TEXT NOT NULL CHECK(
                requested_sandbox_mode IN ('always')
            ),
            effective_sandbox TEXT NOT NULL CHECK(
                effective_sandbox IN ('target-root')
            ),
            phase TEXT NOT NULL,
            message TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(changeset_id, sequence)
        );
CREATE INDEX idx_lifecycle_events_changeset
            ON lifecycle_events(changeset_id, sequence);
CREATE TABLE generation_activation_intents (
            generation_number INTEGER NOT NULL
                REFERENCES system_states(state_number) ON DELETE CASCADE,
            request_id INTEGER NOT NULL
                REFERENCES activation_requests(id) ON DELETE CASCADE,
            status TEXT NOT NULL CHECK(status IN (
                'pending',
                'executing',
                'applied',
                'failed',
                'superseded'
            )),
            attempt_count INTEGER NOT NULL CHECK(attempt_count >= 0),
            last_error TEXT,
            started_at TEXT,
            completed_at TEXT,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(generation_number, request_id)
        );
CREATE INDEX idx_generation_activation_intents_status
            ON generation_activation_intents(generation_number, status);
CREATE INDEX idx_generation_activation_intents_request
            ON generation_activation_intents(request_id, status);
CREATE TABLE installed_native_lifecycle_bundles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trove_id INTEGER NOT NULL UNIQUE REFERENCES troves(id) ON DELETE CASCADE,
            source_format TEXT NOT NULL,
            source_family TEXT NOT NULL,
            source_profile TEXT
                CHECK(
                    source_profile IS NULL
                    OR (
                        source_profile = 'fedora-44'
                        AND source_format = 'rpm'
                    )
                    OR (
                        source_profile = 'ubuntu-26.04'
                        AND source_format = 'deb'
                    )
                    OR (
                        source_profile = 'arch'
                        AND source_format = 'arch'
                    )
                    OR (
                        source_profile = 'solus'
                        AND source_format = 'eopkg'
                    )
                ),
            source_release TEXT,
            source_arch TEXT,
            source_package TEXT NOT NULL,
            source_version TEXT NOT NULL,
            scriptlet_fidelity TEXT NOT NULL,
            evidence_digest TEXT,
            lifecycle_state TEXT NOT NULL DEFAULT 'installed' CHECK(
                lifecycle_state IN (
                    'not-installed',
                    'config-files',
                    'half-installed',
                    'unpacked',
                    'half-configured',
                    'triggers-awaited',
                    'triggers-pending',
                    'installed'
                )
            ),
            pending_triggers_json TEXT NOT NULL DEFAULT '[]',
            awaited_packages_json TEXT NOT NULL DEFAULT '[]',
            bundle_toml TEXT NOT NULL,
            installed_changeset_id INTEGER REFERENCES changesets(id) ON DELETE SET NULL,
            installed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
CREATE INDEX idx_installed_native_lifecycle_bundles_trove
            ON installed_native_lifecycle_bundles(trove_id);
CREATE INDEX idx_installed_native_lifecycle_bundles_evidence
            ON installed_native_lifecycle_bundles(evidence_digest);
CREATE TABLE native_lifecycle_residual_states (
            identity_digest TEXT PRIMARY KEY,
            source_format TEXT NOT NULL,
            source_package TEXT NOT NULL,
            source_version TEXT NOT NULL,
            source_arch TEXT,
            lifecycle_state TEXT NOT NULL CHECK(
                lifecycle_state IN (
                    'not-installed',
                    'config-files',
                    'half-installed',
                    'unpacked',
                    'half-configured',
                    'triggers-awaited',
                    'triggers-pending',
                    'installed'
                )
            ),
            pending_triggers_json TEXT NOT NULL,
            awaited_packages_json TEXT NOT NULL,
            bundle_toml TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
CREATE INDEX idx_native_lifecycle_residual_states_package
            ON native_lifecycle_residual_states(
                source_format, source_package, source_version, source_arch
            );
CREATE TABLE try_sessions (
            id TEXT PRIMARY KEY,
            package_path TEXT NOT NULL,
            package_signing_key TEXT NOT NULL,
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
