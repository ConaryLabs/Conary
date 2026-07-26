-- Current Remi conversion and administration schema.
-- Part of the current pre-alpha schema epoch.

CREATE TABLE converted_packages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            artifact_kind TEXT NOT NULL
                CHECK (artifact_kind IN ('installed', 'repository')),
            -- Reference to the converted trove (CCS package that was installed)
            trove_id INTEGER REFERENCES troves(id) ON DELETE CASCADE,
            -- Original package format (rpm, deb, arch)
            original_format TEXT NOT NULL,
            -- Checksum of the original package file. Repository conversion
            -- identity is scoped by exact source_profile.
            original_checksum TEXT NOT NULL,
            -- Conversion algorithm version (re-convert if upgraded)
            conversion_version INTEGER NOT NULL DEFAULT 1,
            -- When the conversion occurred
            converted_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, enhancement_version INTEGER DEFAULT 0, extracted_provenance_json TEXT, enhancement_status TEXT DEFAULT 'pending', enhancement_error TEXT, enhancement_attempted_at TEXT, enhancement_priority INTEGER DEFAULT 1, package_name TEXT, package_version TEXT, source_profile TEXT CHECK(source_profile IS NULL OR source_profile IN ('fedora-44', 'ubuntu-26.04', 'arch')), chunk_hashes_json TEXT, total_size INTEGER, content_hash TEXT, ccs_path TEXT, package_architecture TEXT, scriptlet_fidelity TEXT NOT NULL DEFAULT 'native-free', evidence_digest TEXT, scriptlet_summary_json TEXT NOT NULL DEFAULT '{"scriptlet_fidelity":"native-free","evidence_digest":null}',
            CHECK (
                (
                    artifact_kind = 'installed'
                    AND trove_id IS NOT NULL
                    AND package_name IS NULL
                    AND package_version IS NULL
                    AND source_profile IS NULL
                    AND package_architecture IS NULL
                    AND chunk_hashes_json IS NULL
                    AND total_size IS NULL
                    AND content_hash IS NULL
                    AND ccs_path IS NULL
                )
                OR (
                    artifact_kind = 'repository'
                    AND trove_id IS NULL
                    AND package_name IS NOT NULL
                    AND length(package_name) > 0
                    AND package_version IS NOT NULL
                    AND length(package_version) > 0
                    AND source_profile IS NOT NULL
                    AND length(source_profile) > 0
                    AND package_architecture IS NOT NULL
                    AND length(package_architecture) > 0
                    AND chunk_hashes_json IS NOT NULL
                    AND json_valid(chunk_hashes_json)
                    AND json_type(chunk_hashes_json) = 'array'
                    AND total_size IS NOT NULL
                    AND total_size >= 0
                    AND content_hash IS NOT NULL
                    AND length(content_hash) > 0
                    AND ccs_path IS NOT NULL
                    AND length(ccs_path) > 0
                )
            )
        );
CREATE INDEX idx_converted_packages_trove ON converted_packages(trove_id);
CREATE INDEX idx_converted_packages_format ON converted_packages(original_format);
CREATE INDEX idx_converted_packages_checksum ON converted_packages(original_checksum);
CREATE UNIQUE INDEX idx_converted_installed_checksum
            ON converted_packages(original_checksum)
            WHERE artifact_kind = 'installed';
CREATE UNIQUE INDEX idx_converted_repository_checksum_profile
            ON converted_packages(original_checksum, source_profile)
            WHERE artifact_kind = 'repository';
CREATE INDEX idx_converted_enhancement_status ON converted_packages(enhancement_status);
CREATE INDEX idx_converted_enhancement_version ON converted_packages(enhancement_version);
CREATE INDEX idx_converted_enhancement_priority
            ON converted_packages(enhancement_status, enhancement_priority DESC);
CREATE INDEX idx_converted_packages_identity
            ON converted_packages(source_profile, package_name, package_version);
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
VALUES ('canonical_map_revision', '0');
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
            image_cas_hash TEXT NOT NULL CHECK(
                length(image_cas_hash) = 64
                AND image_cas_hash NOT GLOB '*[^0-9a-f]*'
            ),
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
            ON converted_packages(source_profile, package_name, package_version, package_architecture);
CREATE INDEX idx_converted_packages_scriptlet_fidelity
            ON converted_packages(scriptlet_fidelity);
