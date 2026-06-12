// conary-core/src/db/models/repository_package_key.rs

//! Repository package signing key persistence.

use crate::error::{Error, Result};
use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryPackageKeyStatus {
    Active,
    Retired,
}

impl RepositoryPackageKeyStatus {
    fn as_db_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPackageKey {
    pub repository_id: i64,
    pub public_key: String,
    pub key_id: Option<String>,
    pub status: RepositoryPackageKeyStatus,
    pub synced_at: Option<String>,
}

impl RepositoryPackageKey {
    pub fn replace_for_repository(
        conn: &Connection,
        repository_id: i64,
        keys: &[Self],
    ) -> Result<()> {
        for key in keys {
            if key.repository_id != repository_id {
                return Err(Error::InternalError(format!(
                    "repository_id mismatch for repository package key: expected {repository_id}, got {}",
                    key.repository_id
                )));
            }
        }

        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM repository_package_keys WHERE repository_id = ?1",
            [repository_id],
        )?;

        {
            let mut insert_with_default_synced_at = tx.prepare(
                "INSERT INTO repository_package_keys (repository_id, public_key, key_id, status)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut insert_with_synced_at = tx.prepare(
                "INSERT INTO repository_package_keys
                    (repository_id, public_key, key_id, status, synced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;

            for key in keys {
                if let Some(synced_at) = &key.synced_at {
                    insert_with_synced_at.execute(params![
                        key.repository_id,
                        &key.public_key,
                        &key.key_id,
                        key.status.as_db_str(),
                        synced_at,
                    ])?;
                } else {
                    insert_with_default_synced_at.execute(params![
                        key.repository_id,
                        &key.public_key,
                        &key.key_id,
                        key.status.as_db_str(),
                    ])?;
                }
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn trusted_keys_for_repository(
        conn: &Connection,
        repository_id: i64,
    ) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT public_key
             FROM repository_package_keys
             WHERE repository_id = ?1
               AND status = 'active'
             ORDER BY public_key",
        )?;

        let keys = stmt
            .query_map([repository_id], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::Repository;
    use crate::db::testing::create_test_db;
    use rusqlite::Connection;

    fn insert_repository(conn: &Connection, name: &str) -> i64 {
        let mut repo = Repository::new(
            name.to_string(),
            format!("https://{name}.example.invalid/repo"),
        );
        repo.insert(conn).unwrap()
    }

    fn stored_rows(
        conn: &Connection,
        repository_id: i64,
    ) -> Vec<(String, Option<String>, String, Option<String>)> {
        conn.prepare(
            "SELECT public_key, key_id, status, synced_at
             FROM repository_package_keys
             WHERE repository_id = ?1
             ORDER BY public_key",
        )
        .unwrap()
        .query_map([repository_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
    }

    fn package_key(
        repository_id: i64,
        public_key: &str,
        key_id: Option<&str>,
        status: RepositoryPackageKeyStatus,
        synced_at: Option<&str>,
    ) -> RepositoryPackageKey {
        RepositoryPackageKey {
            repository_id,
            public_key: public_key.to_string(),
            key_id: key_id.map(str::to_string),
            status,
            synced_at: synced_at.map(str::to_string),
        }
    }

    #[test]
    fn active_and_retired_keys_persist() {
        let (_temp, conn) = create_test_db();
        let repo_id = insert_repository(&conn, "static-keys");

        let keys = vec![
            package_key(
                repo_id,
                "z-retired-public-key",
                Some("retired-key"),
                RepositoryPackageKeyStatus::Retired,
                Some("2026-06-10T12:00:00Z"),
            ),
            package_key(
                repo_id,
                "a-active-public-key",
                Some("active-key"),
                RepositoryPackageKeyStatus::Active,
                None,
            ),
        ];

        RepositoryPackageKey::replace_for_repository(&conn, repo_id, &keys).unwrap();

        let rows = stored_rows(&conn, repo_id);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            (
                "a-active-public-key".to_string(),
                Some("active-key".to_string()),
                "active".to_string(),
                rows[0].3.clone()
            )
        );
        assert!(rows[0].3.as_deref().is_some_and(|value| {
            value.len() == "2026-06-10T12:00:00Z".len()
                && value.contains('T')
                && value.ends_with('Z')
        }));
        assert_eq!(
            rows[1],
            (
                "z-retired-public-key".to_string(),
                Some("retired-key".to_string()),
                "retired".to_string(),
                Some("2026-06-10T12:00:00Z".to_string())
            )
        );
    }

    #[test]
    fn replace_for_repository_deletes_absent_keys_and_writes_verified_set() {
        let (_temp, conn) = create_test_db();
        let repo_id = insert_repository(&conn, "replace-keys");

        RepositoryPackageKey::replace_for_repository(
            &conn,
            repo_id,
            &[
                package_key(
                    repo_id,
                    "gone-key",
                    None,
                    RepositoryPackageKeyStatus::Active,
                    None,
                ),
                package_key(
                    repo_id,
                    "kept-key",
                    Some("old"),
                    RepositoryPackageKeyStatus::Retired,
                    None,
                ),
            ],
        )
        .unwrap();

        RepositoryPackageKey::replace_for_repository(
            &conn,
            repo_id,
            &[
                package_key(
                    repo_id,
                    "kept-key",
                    Some("new"),
                    RepositoryPackageKeyStatus::Active,
                    Some("2026-06-11T01:02:03Z"),
                ),
                package_key(
                    repo_id,
                    "new-key",
                    None,
                    RepositoryPackageKeyStatus::Retired,
                    None,
                ),
            ],
        )
        .unwrap();

        let rows = stored_rows(&conn, repo_id);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "kept-key");
        assert_eq!(rows[0].1.as_deref(), Some("new"));
        assert_eq!(rows[0].2, "active");
        assert_eq!(rows[0].3.as_deref(), Some("2026-06-11T01:02:03Z"));
        assert_eq!(rows[1].0, "new-key");
        assert_eq!(rows[1].2, "retired");
    }

    #[test]
    fn trusted_keys_for_repository_returns_only_active_keys_in_order() {
        let (_temp, conn) = create_test_db();
        let repo_id = insert_repository(&conn, "trusted-keys");

        RepositoryPackageKey::replace_for_repository(
            &conn,
            repo_id,
            &[
                package_key(
                    repo_id,
                    "z-retired-public-key",
                    None,
                    RepositoryPackageKeyStatus::Retired,
                    None,
                ),
                package_key(
                    repo_id,
                    "a-active-public-key",
                    None,
                    RepositoryPackageKeyStatus::Active,
                    None,
                ),
            ],
        )
        .unwrap();

        let trusted = RepositoryPackageKey::trusted_keys_for_repository(&conn, repo_id).unwrap();

        assert_eq!(trusted, vec!["a-active-public-key".to_string()]);
    }

    #[test]
    fn replace_for_repository_rejects_mismatched_repository_ids_without_replacing() {
        let (_temp, conn) = create_test_db();
        let repo_id = insert_repository(&conn, "mismatch-owner");
        let other_repo_id = insert_repository(&conn, "mismatch-other");

        RepositoryPackageKey::replace_for_repository(
            &conn,
            repo_id,
            &[package_key(
                repo_id,
                "existing-key",
                None,
                RepositoryPackageKeyStatus::Active,
                None,
            )],
        )
        .unwrap();

        let err = RepositoryPackageKey::replace_for_repository(
            &conn,
            repo_id,
            &[package_key(
                other_repo_id,
                "wrong-repo-key",
                None,
                RepositoryPackageKeyStatus::Active,
                None,
            )],
        )
        .unwrap_err();

        assert!(err.to_string().contains("repository_id"));
        assert_eq!(
            RepositoryPackageKey::trusted_keys_for_repository(&conn, repo_id).unwrap(),
            vec!["existing-key".to_string()]
        );
    }
}
