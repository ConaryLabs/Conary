// conary-core/src/repository/parsers/arch/spool.rs

//! Disk-backed exact pairing of ALPM `desc` and `depends` records.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::error::{Error, Result};

pub(super) struct ArchPackageSpool {
    path: PathBuf,
    connection: Option<Connection>,
}

impl ArchPackageSpool {
    pub(super) fn create(path: &Path) -> Result<Self> {
        if fs::symlink_metadata(path).is_ok() {
            return Err(Error::AlreadyExists(format!(
                "Arch ingestion spool {}",
                path.display()
            )));
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA synchronous = FULL;
             PRAGMA trusted_schema = OFF;
             CREATE TABLE arch_package_records (
                 directory TEXT PRIMARY KEY,
                 desc TEXT,
                 depends TEXT,
                 CHECK (desc IS NOT NULL OR depends IS NOT NULL)
             ) STRICT, WITHOUT ROWID;
             BEGIN IMMEDIATE;",
        )?;
        Ok(Self {
            path: path.to_path_buf(),
            connection: Some(connection),
        })
    }

    pub(super) fn desc(&self, directory: &str, content: String) -> Result<()> {
        self.admit(directory, content, true)
    }

    pub(super) fn depends(&self, directory: &str, content: String) -> Result<()> {
        self.admit(directory, content, false)
    }

    fn admit(&self, directory: &str, content: String, desc: bool) -> Result<()> {
        let (field, label) = if desc {
            ("desc", "desc")
        } else {
            ("depends", "depends")
        };
        let changed = self.connection()?.execute(
            &format!(
                "INSERT INTO arch_package_records (directory, {field}) VALUES (?1, ?2)
                 ON CONFLICT(directory) DO UPDATE SET {field} = excluded.{field}
                 WHERE arch_package_records.{field} IS NULL"
            ),
            params![directory, content],
        )?;
        if changed != 1 {
            return Err(Error::ParseError(format!(
                "Arch repository repeats {label} metadata for {directory}"
            )));
        }
        Ok(())
    }

    pub(super) fn finish(
        mut self,
        mut visitor: impl FnMut(&str, &str, Option<&str>) -> Result<()>,
    ) -> Result<u64> {
        self.connection()?.execute_batch("COMMIT")?;
        if let Some(orphan) = self
            .connection()?
            .query_row(
                "SELECT directory FROM arch_package_records
                 WHERE desc IS NULL ORDER BY directory LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Err(Error::ParseError(format!(
                "Arch repository has depends metadata without desc metadata for {orphan}"
            )));
        }

        let mut count = 0_u64;
        {
            let mut statement = self.connection()?.prepare(
                "SELECT directory, desc, depends FROM arch_package_records ORDER BY directory",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let directory: String = row.get(0)?;
                let desc: String = row.get(1)?;
                let depends: Option<String> = row.get(2)?;
                visitor(&directory, &desc, depends.as_deref())?;
                count = count.checked_add(1).ok_or_else(|| {
                    Error::ParseError("Arch repository package count exceeds u64".to_string())
                })?;
            }
        }
        self.connection
            .take()
            .expect("Arch spool connection exists")
            .close()
            .map_err(|(_, error)| Error::Database(error))?;
        fs::remove_file(&self.path)?;
        Ok(count)
    }

    fn connection(&self) -> Result<&Connection> {
        self.connection.as_ref().ok_or_else(|| {
            Error::InternalError("Arch package spool has no open connection".to_string())
        })
    }
}

impl Drop for ArchPackageSpool {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ = connection.close();
        }
        let _ = fs::remove_file(&self.path);
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = self.path.as_os_str().to_os_string();
            sidecar.push(suffix);
            let _ = fs::remove_file(PathBuf::from(sidecar));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_order_records_pair_by_exact_directory_and_emit_canonically() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("records.sqlite");
        let spool = ArchPackageSpool::create(&path).unwrap();
        spool
            .depends("zeta-2-1", "%DEPENDS%\nlibc\n".to_string())
            .unwrap();
        spool
            .desc("alpha-1-1", "%NAME%\nalpha\n".to_string())
            .unwrap();
        spool
            .desc("zeta-2-1", "%NAME%\nzeta\n".to_string())
            .unwrap();

        let mut records = Vec::new();
        assert_eq!(
            spool
                .finish(|directory, desc, depends| {
                    records.push((
                        directory.to_string(),
                        desc.to_string(),
                        depends.map(str::to_string),
                    ));
                    Ok(())
                })
                .unwrap(),
            2
        );
        assert_eq!(records[0].0, "alpha-1-1");
        assert_eq!(records[1].0, "zeta-2-1");
        assert_eq!(records[1].2.as_deref(), Some("%DEPENDS%\nlibc\n"));
        assert!(!path.exists());
    }

    #[test]
    fn duplicate_and_orphan_records_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let duplicate_path = root.path().join("duplicate.sqlite");
        let duplicate = ArchPackageSpool::create(&duplicate_path).unwrap();
        duplicate.desc("pkg", "first".to_string()).unwrap();
        assert!(duplicate.desc("pkg", "second".to_string()).is_err());
        drop(duplicate);
        assert!(!duplicate_path.exists());

        let orphan_path = root.path().join("orphan.sqlite");
        let orphan = ArchPackageSpool::create(&orphan_path).unwrap();
        orphan.depends("pkg", "depends".to_string()).unwrap();
        let error = orphan.finish(|_, _, _| Ok(())).unwrap_err();
        assert!(error.to_string().contains("without desc metadata for pkg"));
        assert!(!orphan_path.exists());
    }
}
