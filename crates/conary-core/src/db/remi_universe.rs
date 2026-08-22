// crates/conary-core/src/db/remi_universe.rs

//! Connection-local attachment of the active immutable client universe index.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::error::{Error, Result};

const CLIENT_INDEX_APPLICATION_ID: i64 = 0x4355_4958;

const TEMP_AUTHORITY_VIEWS: &str = r#"
CREATE TEMP VIEW resolved_repository_packages AS
SELECT id, repository_id, name, version, package_release, architecture,
       debian_multi_arch, description, checksum, size, download_url, metadata,
       synced_at, is_security_update, severity, cve_ids, advisory_id,
       advisory_url, source_profile, version_scheme,
       COALESCE(package.canonical_id, (
           SELECT implementation.canonical_id
           FROM remi_universe_index.package_implementations implementation
           JOIN main.repositories repository ON repository.id = package.repository_id
           WHERE implementation.distro = COALESCE(package.source_profile, repository.source_profile)
             AND implementation.distro_name = package.name
       )) AS canonical_id
FROM main.repository_packages package
UNION ALL
SELECT id, repository_id, name, version, package_release, architecture,
       debian_multi_arch, description, checksum, size, download_url, metadata,
       synced_at, is_security_update, severity, cve_ids, advisory_id,
       advisory_url, source_profile, version_scheme, canonical_id
FROM remi_universe_index.repository_packages;

CREATE TEMP VIEW resolved_repository_provides AS
SELECT id, repository_package_id, capability, version, version_relation, kind,
       raw, version_scheme, architecture_qualifier_kind, architecture, provenance
FROM main.repository_provides
UNION ALL
SELECT id, repository_package_id, capability, version, version_relation, kind,
       raw, version_scheme, architecture_qualifier_kind, architecture, provenance
FROM remi_universe_index.repository_provides;

CREATE TEMP VIEW resolved_repository_requirement_groups AS
SELECT id, repository_package_id, kind, behavior, description, native_text,
       expression_json
FROM main.repository_requirement_groups
UNION ALL
SELECT id, repository_package_id, kind, behavior, description, native_text,
       expression_json
FROM remi_universe_index.repository_requirement_groups;

CREATE TEMP VIEW resolved_repository_requirements AS
SELECT id, repository_package_id, group_id, capability, version_constraint,
       kind, dependency_type, raw
FROM main.repository_requirements
UNION ALL
SELECT id, repository_package_id, group_id, capability, version_constraint,
       kind, dependency_type, raw
FROM remi_universe_index.repository_requirements;

CREATE TEMP VIEW resolved_canonical_packages AS
SELECT id, name, appstream_id, description, kind, category
FROM main.canonical_packages
UNION ALL
SELECT id, name, appstream_id, description, kind, category
FROM remi_universe_index.canonical_packages;

CREATE TEMP VIEW resolved_package_implementations AS
SELECT id, canonical_id, distro, distro_name, source
FROM main.package_implementations
UNION ALL
SELECT id, canonical_id, distro, distro_name, source
FROM remi_universe_index.package_implementations;
"#;

pub(super) fn attach_active_index(conn: &Connection, database_path: &Path) -> Result<()> {
    let active = conn
        .query_row(
            "SELECT active.manifest_sha256, revision.index_path,
                    revision.index_sha256, revision.index_size
             FROM remi_active_client_universe active
             JOIN remi_client_universe_revisions revision
               ON revision.endpoint = active.endpoint
              AND revision.manifest_sha256 = active.manifest_sha256
             WHERE active.singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((manifest_sha256, stored_path, expected_sha256, expected_size)) = active else {
        return Ok(());
    };
    validate_sha256(&manifest_sha256, "active universe manifest")?;
    validate_sha256(&expected_sha256, "active universe index")?;
    let expected_size = u64::try_from(expected_size)
        .map_err(|_| Error::ConfigError("active universe index has a negative size".to_string()))?;
    let index_path = validate_index_path(database_path, &stored_path, expected_size)?;
    let mut uri = url::Url::from_file_path(&index_path).map_err(|_| {
        Error::InvalidPath(format!(
            "active universe index {} cannot be represented as a SQLite URI",
            index_path.display()
        ))
    })?;
    uri.query_pairs_mut()
        .append_pair("mode", "ro")
        .append_pair("immutable", "1");
    conn.execute("ATTACH DATABASE ?1 AS remi_universe_index", [uri.as_str()])?;

    let application_id: i64 =
        conn.query_row("PRAGMA remi_universe_index.application_id", [], |row| {
            row.get(0)
        })?;
    if application_id != CLIENT_INDEX_APPLICATION_ID {
        return Err(Error::ConfigError(format!(
            "active universe index has application id {application_id:#x}; expected {CLIENT_INDEX_APPLICATION_ID:#x}"
        )));
    }
    let stored_manifest = conn.query_row(
        "SELECT manifest_sha256 FROM remi_universe_index.universe_metadata
         WHERE singleton = 1",
        [],
        |row| row.get::<_, String>(0),
    )?;
    if stored_manifest != manifest_sha256 {
        return Err(Error::ConflictError(
            "active universe pointer disagrees with attached index authority".to_string(),
        ));
    }
    conn.execute_batch(TEMP_AUTHORITY_VIEWS)?;
    Ok(())
}

fn validate_index_path(
    database_path: &Path,
    stored_path: &str,
    expected_size: u64,
) -> Result<PathBuf> {
    let path = Path::new(stored_path);
    if !path.is_absolute() {
        return Err(Error::InvalidPath(
            "active universe index path must be absolute".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Error::InvalidPath(format!(
            "active universe index {} must be a regular file",
            path.display()
        )));
    }
    if metadata.len() != expected_size {
        return Err(Error::ChecksumMismatch {
            expected: format!("{expected_size} bytes"),
            actual: format!("{} bytes", metadata.len()),
        });
    }
    if metadata.permissions().mode() & 0o277 != 0 {
        return Err(Error::InvalidPath(format!(
            "active universe index {} must be immutable and private",
            path.display()
        )));
    }
    let canonical = path.canonicalize()?;
    let database_parent = database_path
        .canonicalize()?
        .parent()
        .ok_or_else(|| Error::InvalidPath("database has no parent directory".to_string()))?
        .to_path_buf();
    let universe_root = database_parent.join("remi-universes");
    require_private_real_directory(&universe_root, "client universe root")?;
    let indices_root = universe_root.join("indices");
    require_private_real_directory(&indices_root, "client universe index root")?;
    let trusted_root = indices_root.canonicalize()?;
    if !canonical.starts_with(&trusted_root) {
        return Err(Error::InvalidPath(format!(
            "active universe index {} is outside {}",
            canonical.display(),
            trusted_root.display()
        )));
    }
    Ok(canonical)
}

fn require_private_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::InvalidPath(format!(
            "{label} {} must be a real directory",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error::InvalidPath(format!(
            "{label} {} must be private",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) const fn client_index_application_id() -> i64 {
    CLIENT_INDEX_APPLICATION_ID
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ConfigError(format!(
            "{label} must be one lowercase SHA-256 digest"
        )));
    }
    Ok(())
}
