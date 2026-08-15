// conary-core/src/db/models/package_transaction_staging/sql.rs

//! Ephemeral SQLite schema owned by package transaction staging.

pub(super) const STAGING_SCHEMA_SQL: &str = r#"
CREATE TEMP TABLE conary_tx_components (
    trove_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    is_installed INTEGER NOT NULL CHECK(is_installed IN (0, 1)),
    PRIMARY KEY(trove_id, name)
) WITHOUT ROWID;

CREATE TEMP TABLE conary_tx_payload (
    path TEXT NOT NULL,
    trove_id INTEGER NOT NULL,
    package_name TEXT NOT NULL,
    component_name TEXT,
    payload_node_json TEXT NOT NULL CHECK(json_valid(payload_node_json)),
    content_sha256 TEXT,
    content_size INTEGER,
    sharing_policy TEXT NOT NULL CHECK(sharing_policy IN ('exclusive', 'rpm')),
    anchor_policy TEXT NOT NULL CHECK(anchor_policy IN (
        'exact', 'directory', 'directory-or-symlink-to-directory'
    )),
    disposition TEXT NOT NULL CHECK(disposition IN (
        'auto', 'replace', 'share', 'apply-directory', 'preserve-selected-root'
    )),
    selected_root_node_json TEXT CHECK(
        selected_root_node_json IS NULL OR json_valid(selected_root_node_json)
    ),
    materialization_target_path TEXT,
    history_changeset_id INTEGER,
    history_action TEXT CHECK(history_action IN ('add', 'modify')),
    PRIMARY KEY(path, trove_id),
    CHECK(
        (content_sha256 IS NULL AND content_size IS NULL)
        OR (content_sha256 IS NOT NULL AND content_size IS NOT NULL AND content_size >= 0)
    ),
    CHECK(
        (history_changeset_id IS NULL AND history_action IS NULL)
        OR (history_changeset_id IS NOT NULL AND history_action IS NOT NULL)
    )
) WITHOUT ROWID;

CREATE TEMP TABLE conary_tx_replacement_owners (
    trove_id INTEGER PRIMARY KEY
) WITHOUT ROWID;

CREATE TEMP TABLE conary_tx_decisions (
    path TEXT NOT NULL,
    trove_id INTEGER NOT NULL,
    anchor_action TEXT NOT NULL CHECK(anchor_action IN (
        'replace', 'keep', 'apply-directory', 'insert-selected-root'
    )),
    materialized INTEGER NOT NULL CHECK(materialized IN (0, 1)),
    PRIMARY KEY(path, trove_id)
) WITHOUT ROWID;

CREATE TEMP TABLE conary_tx_configs (
    path TEXT PRIMARY KEY,
    trove_id INTEGER NOT NULL,
    original_hash TEXT,
    original_md5 TEXT,
    current_hash TEXT,
    noreplace INTEGER NOT NULL CHECK(noreplace IN (0, 1)),
    ghost INTEGER NOT NULL CHECK(ghost IN (0, 1)),
    materialized INTEGER NOT NULL CHECK(materialized IN (0, 1)),
    remove_on_upgrade INTEGER NOT NULL CHECK(remove_on_upgrade IN (0, 1)),
    status TEXT NOT NULL,
    modified_at TEXT,
    source TEXT NOT NULL
) WITHOUT ROWID;

CREATE TEMP TABLE conary_tx_history (
    changeset_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    sha256_hash TEXT,
    action TEXT NOT NULL CHECK(action IN ('add', 'modify', 'delete')),
    PRIMARY KEY(changeset_id, path, action)
) WITHOUT ROWID;
"#;

pub(super) const CLEAR_STAGING_SQL: &str = r#"
DELETE FROM conary_tx_history;
DELETE FROM conary_tx_configs;
DELETE FROM conary_tx_decisions;
DELETE FROM conary_tx_replacement_owners;
DELETE FROM conary_tx_payload;
DELETE FROM conary_tx_components;
"#;

pub(super) const DROP_STAGING_SQL: &str = r#"
DROP TABLE conary_tx_history;
DROP TABLE conary_tx_configs;
DROP TABLE conary_tx_decisions;
DROP TABLE conary_tx_replacement_owners;
DROP TABLE conary_tx_payload;
DROP TABLE conary_tx_components;
"#;
