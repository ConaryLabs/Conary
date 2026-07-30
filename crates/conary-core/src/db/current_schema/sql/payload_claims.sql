-- conary-core/src/db/current_schema/sql/payload_claims.sql

CREATE TABLE payload_claims (
            path TEXT NOT NULL REFERENCES files(path)
                ON UPDATE CASCADE ON DELETE CASCADE,
            trove_id INTEGER NOT NULL REFERENCES troves(id) ON DELETE CASCADE,
            component_id INTEGER,
            sharing_policy TEXT NOT NULL
                CHECK (sharing_policy IN ('exclusive', 'rpm')),
            anchor_policy TEXT NOT NULL
                CHECK (
                    anchor_policy IN (
                        'exact',
                        'directory',
                        'directory-or-symlink-to-directory'
                    )
                ),
            materialization_target_path TEXT REFERENCES files(path)
                ON UPDATE CASCADE,
            payload_node_json TEXT NOT NULL CHECK (json_valid(payload_node_json)),
            content_sha256 TEXT,
            content_size INTEGER,
            claimed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            CHECK (
                (content_sha256 IS NULL AND content_size IS NULL)
                OR (
                    content_sha256 IS NOT NULL
                    AND content_size IS NOT NULL
                    AND content_size >= 0
                )
            ),
            CHECK (
                (
                    json_extract(payload_node_json, '$.source.kind.type') = 'regular'
                    AND content_sha256 IS NOT NULL
                )
                OR (
                    json_extract(payload_node_json, '$.source.kind.type') != 'regular'
                    AND content_sha256 IS NULL
                )
            ),
            CHECK (
                (
                    anchor_policy = 'exact'
                    AND json_extract(
                        payload_node_json, '$.source.kind.type'
                    ) != 'directory'
                )
                OR (
                    anchor_policy IN (
                        'directory',
                        'directory-or-symlink-to-directory'
                    )
                    AND json_extract(
                        payload_node_json, '$.source.kind.type'
                    ) = 'directory'
                )
            ),
            CHECK (
                materialization_target_path IS NULL
                OR (
                    materialization_target_path != path
                    AND anchor_policy = 'directory-or-symlink-to-directory'
                )
            ),
            PRIMARY KEY(path, trove_id),
            FOREIGN KEY (component_id, trove_id)
                REFERENCES components(id, parent_trove_id)
        );
CREATE INDEX idx_payload_claims_trove_id
            ON payload_claims(trove_id);
CREATE INDEX idx_payload_claims_materialization_target
            ON payload_claims(materialization_target_path)
            WHERE materialization_target_path IS NOT NULL;

CREATE TRIGGER validate_payload_claim_anchor_before_insert
BEFORE INSERT ON payload_claims
WHEN NOT EXISTS (
    SELECT 1
    FROM files
    WHERE path = NEW.path
      AND (
          (
              NEW.anchor_policy = 'exact'
              AND json_extract(
                  payload_node_json, '$.source.kind.type'
              ) != 'directory'
          )
          OR (
              NEW.anchor_policy = 'directory'
              AND json_extract(
                  payload_node_json, '$.source.kind.type'
              ) = 'directory'
          )
          OR (
              NEW.anchor_policy = 'directory-or-symlink-to-directory'
              AND json_extract(
                  payload_node_json, '$.source.kind.type'
              ) IN ('directory', 'symlink')
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'payload claim anchor violates its typed policy');
END;

CREATE TRIGGER validate_payload_claim_anchor_before_update
BEFORE UPDATE OF path, anchor_policy ON payload_claims
WHEN NOT EXISTS (
    SELECT 1
    FROM files
    WHERE path = NEW.path
      AND (
          (
              NEW.anchor_policy = 'exact'
              AND json_extract(
                  payload_node_json, '$.source.kind.type'
              ) != 'directory'
          )
          OR (
              NEW.anchor_policy = 'directory'
              AND json_extract(
                  payload_node_json, '$.source.kind.type'
              ) = 'directory'
          )
          OR (
              NEW.anchor_policy = 'directory-or-symlink-to-directory'
              AND json_extract(
                  payload_node_json, '$.source.kind.type'
              ) IN ('directory', 'symlink')
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'payload claim anchor violates its typed policy');
END;

CREATE TRIGGER validate_payload_claim_target_before_insert
BEFORE INSERT ON payload_claims
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
        'payload claim materialization target is not a typed symlink-to-directory edge'
    );
END;

CREATE TRIGGER validate_payload_claim_target_before_update
BEFORE UPDATE OF path, materialization_target_path ON payload_claims
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
        'payload claim materialization target is not a typed symlink-to-directory edge'
    );
END;

CREATE TRIGGER validate_payload_anchor_node_before_update
BEFORE UPDATE OF payload_node_json ON files
WHEN EXISTS (
    SELECT 1
    FROM payload_claims
    WHERE (
        path = OLD.path
        AND (
            (
                anchor_policy = 'exact'
                AND json_extract(
                    NEW.payload_node_json, '$.source.kind.type'
                ) = 'directory'
            )
            OR (
                anchor_policy = 'directory'
                AND json_extract(
                    NEW.payload_node_json, '$.source.kind.type'
                ) != 'directory'
            )
            OR (
                anchor_policy = 'directory-or-symlink-to-directory'
                AND json_extract(
                    NEW.payload_node_json, '$.source.kind.type'
                ) NOT IN ('directory', 'symlink')
            )
        )
    )
    OR (
        materialization_target_path = OLD.path
        AND json_extract(
            NEW.payload_node_json, '$.source.kind.type'
        ) != 'directory'
    )
)
BEGIN
    SELECT RAISE(ABORT, 'materialized payload anchor violates a package claim policy');
END;

CREATE TRIGGER protect_shared_payload_anchor_before_file_delete
BEFORE DELETE ON files
WHEN EXISTS (
    SELECT 1
    FROM payload_claims
    WHERE (
        path = OLD.path
        OR materialization_target_path = OLD.path
    )
      AND trove_id != OLD.trove_id
)
BEGIN
    SELECT RAISE(
        ABORT,
        'cannot delete a payload anchor while peer package claims remain'
    );
END;

CREATE TRIGGER reanchor_shared_payload_before_trove_delete
BEFORE DELETE ON troves
BEGIN
    UPDATE files
    SET trove_id = (
            SELECT claim.trove_id
            FROM payload_claims AS claim
            WHERE (
                  claim.path = files.path
                  OR claim.materialization_target_path = files.path
              )
              AND claim.trove_id != OLD.id
            ORDER BY claim.trove_id, claim.path
            LIMIT 1
        ),
        component_id = (
            SELECT CASE
                       WHEN claim.path = files.path THEN claim.component_id
                       ELSE NULL
                   END
            FROM payload_claims AS claim
            WHERE (
                  claim.path = files.path
                  OR claim.materialization_target_path = files.path
              )
              AND claim.trove_id != OLD.id
            ORDER BY claim.trove_id, claim.path
            LIMIT 1
        )
    WHERE files.trove_id = OLD.id
      AND EXISTS (
            SELECT 1
            FROM payload_claims AS claim
            WHERE (
                  claim.path = files.path
                  OR claim.materialization_target_path = files.path
              )
              AND claim.trove_id != OLD.id
        );
END;
