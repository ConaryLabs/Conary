-- Current Remi conversion, administration, and scriptlet-evidence schema.
-- Part of the single current pre-alpha schema epoch.

CREATE TABLE scriptlet_evidence_clusters (
    cluster_key TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL,
    distro TEXT NOT NULL,
    target_profile TEXT NOT NULL,
    blocked_class TEXT NOT NULL,
    command TEXT NOT NULL,
    normalized_command_shape TEXT NOT NULL,
    normalized_command_shape_hash TEXT NOT NULL,
    lifecycle_phase TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'needs-triage'
        CHECK (state IN (
            'needs-triage',
            'adapter-candidate',
            'in-design',
            'in-implementation',
            'covered-partial',
            'covered-public-ready',
            'wont-support'
        )),
    first_seen TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_seen TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
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
            -- When the conversion occurred
            converted_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, enhancement_version INTEGER DEFAULT 0, extracted_provenance_json TEXT, enhancement_status TEXT DEFAULT 'pending', enhancement_error TEXT, enhancement_attempted_at TEXT, enhancement_priority INTEGER DEFAULT 1, package_name TEXT, package_version TEXT, distro TEXT, chunk_hashes_json TEXT, total_size INTEGER, content_hash TEXT, ccs_path TEXT, package_architecture TEXT, scriptlet_fidelity TEXT NOT NULL DEFAULT 'unknown', target_compatibility TEXT NOT NULL DEFAULT 'unknown', publication_status TEXT NOT NULL DEFAULT 'public', evidence_digest TEXT, curation_evidence_digest TEXT, blocked_reason_codes_json TEXT NOT NULL DEFAULT '[]', scriptlet_summary_json TEXT NOT NULL DEFAULT '{}', review_artifact_path TEXT,
            -- Unique on checksum to prevent duplicate conversions
            UNIQUE(original_checksum)
        );
CREATE INDEX idx_converted_packages_trove ON converted_packages(trove_id);
CREATE INDEX idx_converted_packages_format ON converted_packages(original_format);
CREATE INDEX idx_converted_packages_checksum ON converted_packages(original_checksum);
CREATE INDEX idx_converted_enhancement_status ON converted_packages(enhancement_status);
CREATE INDEX idx_converted_enhancement_version ON converted_packages(enhancement_version);
CREATE INDEX idx_converted_enhancement_priority
            ON converted_packages(enhancement_status, enhancement_priority DESC);
CREATE INDEX idx_converted_packages_identity
            ON converted_packages(distro, package_name, package_version);
CREATE TABLE admin_tokens (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            scopes TEXT NOT NULL DEFAULT 'admin',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_used_at TEXT
        );
CREATE INDEX idx_admin_tokens_hash ON admin_tokens(token_hash);
CREATE TABLE admin_audit_log (
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
CREATE INDEX idx_audit_log_timestamp ON admin_audit_log(timestamp);
CREATE INDEX idx_audit_log_action ON admin_audit_log(action);
CREATE TABLE server_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
INSERT INTO server_metadata (key, value)
VALUES ('canonical_map_version', '0');
CREATE TABLE client_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
CREATE TABLE seeds (
            seed_id TEXT PRIMARY KEY,
            target_triple TEXT NOT NULL,
            source TEXT NOT NULL,
            builder TEXT,
            packages_json TEXT NOT NULL DEFAULT '[]',
            verified_by_json TEXT NOT NULL DEFAULT '[]',
            image_cas_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
CREATE INDEX idx_seeds_target
            ON seeds(target_triple, created_at DESC);
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
CREATE INDEX idx_converted_packages_identity_arch
            ON converted_packages(distro, package_name, package_version, package_architecture);
CREATE INDEX idx_converted_packages_scriptlet_fidelity
            ON converted_packages(scriptlet_fidelity);
CREATE INDEX idx_converted_packages_publication_status
            ON converted_packages(publication_status);
CREATE TABLE scriptlet_evidence_cluster_samples (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            cluster_key TEXT NOT NULL REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
            converted_package_id INTEGER REFERENCES converted_packages(id) ON DELETE SET NULL,
            original_checksum TEXT NOT NULL,
            distro TEXT NOT NULL,
            package_name TEXT NOT NULL,
            package_version TEXT NOT NULL,
            package_architecture TEXT,
            publication_status TEXT NOT NULL,
            scriptlet_fidelity TEXT NOT NULL,
            target_compatibility TEXT NOT NULL,
            typed_evidence_json TEXT NOT NULL,
            reason_codes_json TEXT NOT NULL,
            blocked_classes_json TEXT NOT NULL,
            boot_security_intents_json TEXT NOT NULL,
            review_artifact_path TEXT,
            review_artifact_stale INTEGER NOT NULL DEFAULT 0,
            evidence_digest TEXT,
            curation_evidence_digest TEXT,
            observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        , security_policy_intents_json TEXT NOT NULL DEFAULT '[]');
CREATE TABLE scriptlet_evidence_state_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            cluster_key TEXT NOT NULL REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
            from_state TEXT,
            to_state TEXT NOT NULL,
            actor TEXT NOT NULL,
            reason TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );
CREATE TABLE scriptlet_evidence_notes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            cluster_key TEXT NOT NULL REFERENCES scriptlet_evidence_clusters(cluster_key) ON DELETE CASCADE,
            actor TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );
CREATE TABLE scriptlet_evidence_backfill_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'failed')),
            started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            completed_at TEXT,
            last_converted_package_id INTEGER NOT NULL DEFAULT 0,
            scanned_count INTEGER NOT NULL DEFAULT 0,
            clustered_count INTEGER NOT NULL DEFAULT 0,
            error_message TEXT
        );
CREATE INDEX idx_scriptlet_evidence_clusters_state_last_seen
            ON scriptlet_evidence_clusters(state, last_seen DESC);
CREATE INDEX idx_scriptlet_evidence_clusters_class
            ON scriptlet_evidence_clusters(blocked_class, command);
CREATE INDEX idx_scriptlet_evidence_samples_cluster
            ON scriptlet_evidence_cluster_samples(cluster_key, observed_at DESC);
CREATE INDEX idx_scriptlet_evidence_samples_package
            ON scriptlet_evidence_cluster_samples(distro, package_name, package_version, package_architecture);
CREATE UNIQUE INDEX idx_scriptlet_evidence_samples_unique_observation
            ON scriptlet_evidence_cluster_samples(
                cluster_key,
                original_checksum,
                package_name,
                package_version,
                COALESCE(package_architecture, '')
            );
CREATE INDEX idx_scriptlet_evidence_backfill_status
            ON scriptlet_evidence_backfill_runs(status, updated_at DESC);
