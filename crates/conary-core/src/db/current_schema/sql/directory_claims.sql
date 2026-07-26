-- conary-core/src/db/current_schema/sql/directory_claims.sql

CREATE TABLE directory_claims (
            path TEXT NOT NULL REFERENCES files(path)
                ON UPDATE CASCADE ON DELETE CASCADE,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            component_id INTEGER,
            anchor_policy TEXT NOT NULL DEFAULT 'directory'
                CHECK (
                    anchor_policy IN (
                        'directory',
                        'directory-or-symlink-to-directory'
                    )
                ),
            materialization_target_path TEXT REFERENCES files(path)
                ON UPDATE CASCADE,
            payload_node_json TEXT NOT NULL
                CHECK (
                    json_valid(payload_node_json)
                    AND json_extract(
                        payload_node_json, '$.source.kind.type'
                    ) = 'directory'
                ),
            claimed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            CHECK (
                materialization_target_path IS NULL
                OR materialization_target_path != path
            ),
            PRIMARY KEY(path, trove_id),
            FOREIGN KEY (component_id, trove_id)
                REFERENCES components(id, parent_trove_id)
        );
CREATE INDEX idx_directory_claims_trove_id
            ON directory_claims(trove_id);
CREATE INDEX idx_directory_claims_materialization_target
            ON directory_claims(materialization_target_path)
            WHERE materialization_target_path IS NOT NULL;
CREATE TRIGGER validate_directory_claim_anchor_before_insert
BEFORE INSERT ON directory_claims
WHEN NOT EXISTS (
    SELECT 1
    FROM files
    WHERE path = NEW.path
      AND (
          json_extract(payload_node_json, '$.source.kind.type') = 'directory'
          OR (
              NEW.anchor_policy = 'directory-or-symlink-to-directory'
              AND json_extract(payload_node_json, '$.source.kind.type') = 'symlink'
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'directory claim anchor violates its typed policy');
END;
CREATE TRIGGER validate_directory_claim_anchor_before_update
BEFORE UPDATE OF path, anchor_policy ON directory_claims
WHEN NOT EXISTS (
    SELECT 1
    FROM files
    WHERE path = NEW.path
      AND (
          json_extract(payload_node_json, '$.source.kind.type') = 'directory'
          OR (
              NEW.anchor_policy = 'directory-or-symlink-to-directory'
              AND json_extract(payload_node_json, '$.source.kind.type') = 'symlink'
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'directory claim anchor violates its typed policy');
END;
CREATE TRIGGER validate_directory_claim_target_before_insert
BEFORE INSERT ON directory_claims
WHEN NEW.materialization_target_path IS NOT NULL
  AND (
      NEW.materialization_target_path = NEW.path
      OR NOT EXISTS (
          SELECT 1
          FROM files
          WHERE path = NEW.materialization_target_path
            AND json_extract(
                payload_node_json, '$.source.kind.type'
            ) = 'directory'
      )
      OR NOT EXISTS (
          SELECT 1
          FROM files
          WHERE path = NEW.path
            AND json_extract(
                payload_node_json, '$.source.kind.type'
            ) = 'symlink'
      )
  )
BEGIN
    SELECT RAISE(
        ABORT,
        'directory claim materialization target is not a typed symlink-to-directory edge'
    );
END;
CREATE TRIGGER validate_directory_claim_target_before_update
BEFORE UPDATE OF path, materialization_target_path ON directory_claims
WHEN NEW.materialization_target_path IS NOT NULL
  AND (
      NEW.materialization_target_path = NEW.path
      OR NOT EXISTS (
          SELECT 1
          FROM files
          WHERE path = NEW.materialization_target_path
            AND json_extract(
                payload_node_json, '$.source.kind.type'
            ) = 'directory'
      )
      OR NOT EXISTS (
          SELECT 1
          FROM files
          WHERE path = NEW.path
            AND json_extract(
                payload_node_json, '$.source.kind.type'
            ) = 'symlink'
      )
  )
BEGIN
    SELECT RAISE(
        ABORT,
        'directory claim materialization target is not a typed symlink-to-directory edge'
    );
END;
CREATE TRIGGER validate_directory_anchor_node_before_update
BEFORE UPDATE OF payload_node_json ON files
WHEN EXISTS (
    SELECT 1
    FROM directory_claims
    WHERE (
        path = OLD.path
        AND NOT (
            json_extract(NEW.payload_node_json, '$.source.kind.type') = 'directory'
            OR (
                anchor_policy = 'directory-or-symlink-to-directory'
                AND json_extract(NEW.payload_node_json, '$.source.kind.type') = 'symlink'
            )
        )
    )
    OR (
        materialization_target_path = OLD.path
        AND json_extract(NEW.payload_node_json, '$.source.kind.type') != 'directory'
    )
)
BEGIN
    SELECT RAISE(ABORT, 'materialized directory anchor violates a package claim policy');
END;
CREATE TRIGGER protect_shared_directory_anchor_before_file_delete
BEFORE DELETE ON files
WHEN EXISTS (
    SELECT 1
    FROM directory_claims
    WHERE (
        path = OLD.path
        OR materialization_target_path = OLD.path
    )
      AND trove_id != OLD.trove_id
)
BEGIN
    SELECT RAISE(
        ABORT,
        'cannot delete a directory anchor while peer package claims remain'
    );
END;
CREATE TRIGGER reanchor_shared_directories_before_trove_delete
BEFORE DELETE ON troves
BEGIN
    UPDATE files
    SET trove_id = (
            SELECT claim.trove_id
            FROM directory_claims AS claim
            WHERE (
                  claim.path = files.path
                  OR claim.materialization_target_path = files.path
              )
              AND claim.trove_id != OLD.id
            ORDER BY claim.trove_id
            LIMIT 1
        ),
        component_id = (
            SELECT CASE
                       WHEN claim.path = files.path THEN claim.component_id
                       ELSE NULL
                   END
            FROM directory_claims AS claim
            WHERE (
                  claim.path = files.path
                  OR claim.materialization_target_path = files.path
              )
              AND claim.trove_id != OLD.id
            ORDER BY claim.trove_id
            LIMIT 1
        )
    WHERE files.trove_id = OLD.id
      AND EXISTS (
            SELECT 1
            FROM directory_claims AS claim
            WHERE (
                  claim.path = files.path
                  OR claim.materialization_target_path = files.path
              )
              AND claim.trove_id != OLD.id
        );
END;
