// conary-core/src/repository/catalog/store/util.rs

//! Catalog filesystem, canonical JSON, and SQLite conversion helpers.

use std::fs::{self, File, OpenOptions};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub(in crate::repository::catalog) fn validate_candidate_path(path: &Path) -> Result<()> {
    if path.file_name().is_none() {
        return Err(Error::InvalidPath(
            "catalog candidate path must name a file".to_string(),
        ));
    }
    if fs::symlink_metadata(path).is_ok() {
        return Err(Error::AlreadyExists(format!(
            "catalog candidate {}",
            path.display()
        )));
    }
    let parent = path.parent().ok_or_else(|| {
        Error::InvalidPath(format!(
            "catalog candidate {} has no parent directory",
            path.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(Error::InvalidPath(format!(
            "catalog candidate parent {} must be a real directory",
            parent.display()
        )));
    }
    Ok(())
}

pub(in crate::repository::catalog) fn create_private_file(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    Ok(())
}

pub(super) fn reject_nonempty_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = sidecar_path(path, suffix);
        if sidecar.metadata().is_ok_and(|metadata| metadata.len() > 0) {
            return Err(Error::ConflictError(format!(
                "immutable catalog {} has non-empty SQLite sidecar {}",
                path.display(),
                sidecar.display()
            )));
        }
    }
    Ok(())
}

pub(in crate::repository::catalog) fn hash_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    Ok(crate::hash::sha256_reader_hex(&mut reader)?)
}

pub(in crate::repository::catalog) fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidPath(format!("{} has no parent directory", path.display())))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(in crate::repository::catalog) fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

pub(in crate::repository::catalog) fn canonical_json_string(
    value: &impl Serialize,
) -> Result<String> {
    let bytes = crate::json::canonical_json(value)
        .map_err(|error| Error::ParseError(format!("serialize catalog SQLite value: {error}")))?;
    String::from_utf8(bytes).map_err(|error| {
        Error::InternalError(format!("canonical catalog JSON was not UTF-8: {error}"))
    })
}

pub(in crate::repository::catalog) fn checked_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        Error::ConfigError(format!(
            "catalog {label} {value} exceeds SQLite integer range"
        ))
    })
}

pub(in crate::repository::catalog) fn checked_ordinal(value: usize, label: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        Error::ConfigError(format!(
            "catalog {label} ordinal exceeds SQLite integer range"
        ))
    })
}

pub(super) fn checked_sqlite_usize(value: usize, label: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        Error::ConfigError(format!(
            "catalog package-name page {label} exceeds SQLite integer range"
        ))
    })
}

pub(super) fn read_u64(
    row: &rusqlite::Row<'_>,
    column: usize,
    label: &str,
) -> rusqlite::Result<u64> {
    let value: i64 = row.get(column)?;
    u64::try_from(value).map_err(|_| conversion_error(column, format!("negative {label}")))
}

pub(super) fn parse_json_column<T: for<'de> Deserialize<'de>>(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<T> {
    let raw: String = row.get(column)?;
    serde_json::from_str(&raw).map_err(|error| conversion_error(column, error.to_string()))
}

pub(super) fn conversion_error(column: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    )
}
