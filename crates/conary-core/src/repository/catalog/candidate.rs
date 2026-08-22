// conary-core/src/repository/catalog/candidate.rs

//! Bounded construction of one deterministic standalone catalog candidate.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::store::{
    CATALOG_APPLICATION_ID, CATALOG_SCHEMA, canonical_json_string, checked_i64, checked_ordinal,
    create_private_file, digest_catalog_connection, hash_file, insert_package, package_by_key,
    replace_package_provides, sidecar_path, sync_parent, validate_candidate_path,
};
use super::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogArtifactV1, CatalogBindingV1, CatalogPackageOriginV1,
    CatalogPackageRecordV1, CatalogProvideRecordV1, CatalogReader, CatalogScopeV1,
    CatalogSourceEvidenceV1,
};
use crate::error::{Error, Result};

/// Private catalog writer that retains one normalized package at a time.
pub struct CatalogCandidateWriter {
    path: PathBuf,
    scope: CatalogScopeV1,
    connection: Option<Connection>,
    complete: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::repository) struct CatalogProvideMerge {
    pub matched_packages: usize,
    pub added: usize,
    pub already_known: usize,
}

impl CatalogCandidateWriter {
    pub fn create(path: impl AsRef<Path>, scope: CatalogScopeV1) -> Result<Self> {
        scope.validate()?;
        let path = path.as_ref().to_path_buf();
        validate_candidate_path(&path)?;
        create_private_file(&path)?;
        let result = Self::open_created(path.clone(), scope);
        if result.is_err() {
            remove_candidate_files(&path);
        }
        result
    }

    fn open_created(path: PathBuf, scope: CatalogScopeV1) -> Result<Self> {
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.execute_batch(&format!(
            "PRAGMA journal_mode = DELETE;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA page_size = 4096;
             PRAGMA auto_vacuum = NONE;
             PRAGMA application_id = {CATALOG_APPLICATION_ID};
             PRAGMA user_version = {CATALOG_CONTENT_SCHEMA_V1};"
        ))?;
        connection.execute_batch(CATALOG_SCHEMA)?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self {
            path,
            scope,
            connection: Some(connection),
            complete: false,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &CatalogScopeV1 {
        &self.scope
    }

    /// Insert one complete package projection into private SQLite state.
    pub fn package(&mut self, mut package: CatalogPackageRecordV1) -> Result<()> {
        package.canonicalize_for_scope(&self.scope)?;
        insert_package(self.connection()?, &package)
    }

    /// Replay one exact normalized source catalog without native parsing or a
    /// package-sized relation vector.
    pub(in crate::repository) fn copy_source_catalog(
        &mut self,
        reader: &CatalogReader,
    ) -> Result<()> {
        let scope = self.scope.clone();
        reader.copy_packages_to(self.connection()?, &scope, None)
    }

    /// Project one verified source member into a profile catalog while
    /// streaming its normalized relation rows.
    pub(in crate::repository) fn copy_profile_member(
        &mut self,
        reader: &CatalogReader,
        member_ordinal: u32,
        source_identity: String,
        repository_identity: String,
        source_snapshot_sha256: String,
    ) -> Result<()> {
        let scope = self.scope.clone();
        let origin = CatalogPackageOriginV1::Profile {
            member_ordinal,
            source_identity,
            repository_identity,
            source_snapshot_sha256,
        };
        reader.copy_packages_to(self.connection()?, &scope, Some(&origin))
    }

    pub(in crate::repository) fn extend_package_provides(
        &mut self,
        join: &str,
        checksum: &str,
        name: &str,
        version: &str,
        architecture: Option<&str>,
        provides: Vec<CatalogProvideRecordV1>,
    ) -> Result<CatalogProvideMerge> {
        self.connection()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS catalog_ingest_join_marks (
                 join_kind TEXT NOT NULL,
                 package_key_sha256 TEXT NOT NULL
                     REFERENCES catalog_packages(package_key_sha256) ON DELETE CASCADE,
                 PRIMARY KEY (join_kind, package_key_sha256)
             ) STRICT, WITHOUT ROWID;",
        )?;
        let primary = self
            .connection()?
            .query_row(
                "SELECT package_key_sha256, name, version, architecture
                 FROM catalog_packages WHERE checksum = ?1
                 ORDER BY package_key_sha256 LIMIT 1",
                [checksum],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                Error::ParseError(format!(
                    "signed filelists.xml publishes file records for pkgid {checksum}, which the signed primary.xml does not publish"
                ))
            })?;
        let (package_key, primary_name, primary_version, primary_architecture) = primary;
        let disagreement = if primary_name != name {
            Some(("name", name, primary_name.as_str()))
        } else if primary_architecture.as_deref() != architecture {
            Some((
                "architecture",
                architecture.unwrap_or_default(),
                primary_architecture.as_deref().unwrap_or_default(),
            ))
        } else if primary_version != version {
            Some(("version", version, primary_version.as_str()))
        } else {
            None
        };
        if let Some((field, child, primary)) = disagreement {
            return Err(Error::ParseError(format!(
                "signed filelists.xml and primary.xml disagree on pkgid {checksum}: filelists {field} is '{child}' but primary {field} is '{primary}'"
            )));
        }
        if let Err(error) = self.connection()?.execute(
            "INSERT INTO catalog_ingest_join_marks (join_kind, package_key_sha256)
             VALUES (?1, ?2)",
            params![join, &package_key],
        ) {
            return Err(Error::ConflictError(format!(
                "authenticated child metadata repeats package {name} {version} ({checksum}): {error}"
            )));
        }

        let mut package = package_by_key(self.connection()?, &self.scope, &package_key)?;
        let mut result = CatalogProvideMerge {
            matched_packages: 1,
            ..CatalogProvideMerge::default()
        };
        for provide in provides {
            if package.provides.contains(&provide) {
                result.already_known += 1;
            } else {
                package.provides.push(provide);
                result.added += 1;
            }
        }
        package.canonicalize_for_scope(&self.scope)?;
        replace_package_provides(self.connection()?, &package)?;
        Ok(result)
    }

    pub(in crate::repository) fn finish_package_join(&mut self, join: &str) -> Result<()> {
        let marked: i64 = self.connection()?.query_row(
            "SELECT count(*) FROM catalog_ingest_join_marks WHERE join_kind = ?1",
            [join],
            |row| row.get(0),
        )?;
        let packages: i64 =
            self.connection()?
                .query_row("SELECT count(*) FROM catalog_packages", [], |row| {
                    row.get(0)
                })?;
        if marked != packages {
            let missing = self.connection()?.query_row(
                "SELECT name, version, checksum FROM catalog_packages AS package
                 WHERE NOT EXISTS (
                     SELECT 1 FROM catalog_ingest_join_marks AS mark
                     WHERE mark.join_kind = ?1
                       AND mark.package_key_sha256 = package.package_key_sha256
                 )
                 ORDER BY package.package_key_sha256 LIMIT 1",
                [join],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?;
            return Err(Error::ParseError(format!(
                "signed filelists.xml publishes no file record for package {} {} (pkgid {})",
                missing.0, missing.1, missing.2
            )));
        }
        self.connection()?.execute(
            "DELETE FROM catalog_ingest_join_marks WHERE join_kind = ?1",
            [join],
        )?;
        Ok(())
    }

    /// Bind exact source evidence, calculate the canonical logical identity by
    /// ordered iteration, and reopen the resulting artifact before returning.
    pub fn finish(
        mut self,
        source_evidence: Vec<CatalogSourceEvidenceV1>,
    ) -> Result<CatalogBindingV1> {
        let scope = self.scope.clone();
        if table_exists(self.connection()?, "catalog_ingest_join_marks")? {
            let unfinished: i64 = self.connection()?.query_row(
                "SELECT count(*) FROM catalog_ingest_join_marks",
                [],
                |row| row.get(0),
            )?;
            if unfinished != 0 {
                return Err(Error::ConflictError(format!(
                    "catalog candidate has {unfinished} unfinished authenticated child join marks"
                )));
            }
            self.connection()?
                .execute_batch("DROP TABLE catalog_ingest_join_marks")?;
        }
        self.connection()?.execute_batch("COMMIT")?;

        let (logical_digest_sha256, counts) =
            digest_catalog_connection(self.connection()?, &scope, &source_evidence)?;

        let connection = self.connection()?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        connection.execute(
            "INSERT INTO catalog_metadata (
                 singleton, schema_version, scope_json, logical_digest_sha256,
                 package_count, provide_count, requirement_group_count,
                 requirement_atom_count, source_evidence_count
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                i64::from(CATALOG_CONTENT_SCHEMA_V1),
                canonical_json_string(&scope)?,
                &logical_digest_sha256,
                checked_i64(counts.packages, "package count")?,
                checked_i64(counts.provides, "provide count")?,
                checked_i64(counts.requirement_groups, "requirement group count")?,
                checked_i64(counts.requirement_atoms, "requirement atom count")?,
                checked_i64(counts.source_evidence, "source evidence count")?,
            ],
        )?;
        for (ordinal, evidence) in source_evidence.iter().enumerate() {
            connection.execute(
                "INSERT INTO catalog_source_evidence (ordinal, evidence_json) VALUES (?1, ?2)",
                params![
                    checked_ordinal(ordinal, "source evidence")?,
                    canonical_json_string(evidence)?,
                ],
            )?;
        }
        connection.execute_batch("COMMIT; VACUUM;")?;
        let integrity: String =
            connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(Error::InitError(format!(
                "new catalog {} failed SQLite integrity_check: {integrity}",
                self.path.display()
            )));
        }

        self.connection
            .take()
            .expect("catalog candidate connection exists")
            .close()
            .map_err(|(_, error)| Error::Database(error))?;
        std::fs::File::open(&self.path)?.sync_all()?;
        sync_parent(&self.path)?;
        let artifact = CatalogArtifactV1 {
            sha256: hash_file(&self.path)?,
            size: fs::metadata(&self.path)?.len(),
        };
        let binding = CatalogBindingV1 {
            scope,
            artifact,
            logical_digest_sha256,
            counts,
        };
        drop(CatalogReader::open_verified(&self.path, &binding)?);
        self.complete = true;
        Ok(binding)
    }

    fn connection(&self) -> Result<&Connection> {
        self.connection.as_ref().ok_or_else(|| {
            Error::InternalError("catalog candidate writer has no open connection".to_string())
        })
    }
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl Drop for CatalogCandidateWriter {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        if let Some(connection) = self.connection.take() {
            let _ = connection.close();
        }
        remove_candidate_files(&self.path);
    }
}

fn remove_candidate_files(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-journal", "-wal", "-shm"] {
        let _ = fs::remove_file(sidecar_path(path, suffix));
    }
}
