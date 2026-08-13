// conary-core/src/packages/eopkg/takeover.rs

//! Durable eopkg installed-authority removal for system takeover.

use crate::error::{Error, Result};
use crate::filesystem::durable::{remove_file_and_sync_parent, write_json_atomic};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use tracing::warn;

const JOURNAL_VERSION: u32 = 1;
const MARKERS: [&str; 4] = [
    "autoinstalled",
    "configpending",
    "needsrestart",
    "needsreboot",
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityRemovalJournal {
    version: u32,
    selectors: Vec<String>,
    package_directories: Vec<String>,
    marker_backups: BTreeMap<String, Option<Vec<u8>>>,
}

struct EopkgPaths {
    package_root: PathBuf,
    info_root: PathBuf,
    lock_path: PathBuf,
    journal_path: PathBuf,
    staging_root: PathBuf,
}

impl EopkgPaths {
    fn system() -> Self {
        let lib_root = PathBuf::from("/var/lib/eopkg");
        Self {
            package_root: lib_root.join("package"),
            info_root: lib_root.join("info"),
            lock_path: PathBuf::from("/var/lock/subsys/pisi"),
            journal_path: lib_root.join(".conary-authority-removal.json"),
            staging_root: lib_root.join(".conary-authority-removal"),
        }
    }
}

/// Complete an interrupted eopkg authority removal before takeover replans.
pub fn resume_pending_authority_removal() -> Result<()> {
    let paths = EopkgPaths::system();
    with_eopkg_lock(&paths, || resume_at(&paths))
}

/// Whether a durable eopkg authority-removal operation must be resumed.
#[must_use]
pub fn authority_removal_pending() -> bool {
    EopkgPaths::system().journal_path.exists()
}

/// Remove the complete remaining eopkg installed-package authority as one
/// durable, resumable operation.
pub fn remove_all_from_db(selectors: &[String]) -> Result<()> {
    let paths = EopkgPaths::system();
    with_eopkg_lock(&paths, || {
        resume_at(&paths)?;
        begin_at(&paths, selectors)?;
        resume_at(&paths)
    })
}

fn with_eopkg_lock<T>(paths: &EopkgPaths, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    if let Some(parent) = paths.lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&paths.lock_path)?;
    lock.try_lock_exclusive().map_err(|error| {
        Error::ConflictError(format!(
            "cannot transfer eopkg authority while another eopkg operation holds {}: {error}",
            paths.lock_path.display()
        ))
    })?;
    operation()
}

fn begin_at(paths: &EopkgPaths, selectors: &[String]) -> Result<()> {
    if paths.journal_path.exists() {
        return Err(Error::ConflictError(format!(
            "eopkg authority-removal journal {} already exists",
            paths.journal_path.display()
        )));
    }
    let requested = selectors.iter().cloned().collect::<BTreeSet<_>>();
    if requested.len() != selectors.len() {
        return Err(Error::ConflictError(
            "eopkg authority-removal request repeats a package selector".to_string(),
        ));
    }

    let database = super::query::InstalledEopkgDatabase::load_root(&paths.package_root)?;
    let installed = database
        .packages()
        .into_iter()
        .map(|record| record.identity.selector().to_string())
        .collect::<BTreeSet<_>>();
    if requested != installed {
        let missing = installed
            .difference(&requested)
            .cloned()
            .collect::<Vec<_>>();
        let unknown = requested
            .difference(&installed)
            .cloned()
            .collect::<Vec<_>>();
        return Err(Error::ConflictError(format!(
            "eopkg takeover must remove the complete installed authority; missing={missing:?}, unknown={unknown:?}"
        )));
    }

    let mut package_directories = fs::read_dir(&paths.package_root)?
        .map(|entry| {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                return Err(Error::ParseError(format!(
                    "eopkg package authority {} is not a directory",
                    entry.path().display()
                )));
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| Error::ParseError("eopkg package directory is not UTF-8".to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    package_directories.sort();

    let marker_backups = MARKERS
        .into_iter()
        .map(|marker| {
            let path = paths.info_root.join(marker);
            Ok((
                marker.to_string(),
                if path.exists() {
                    Some(fs::read(path)?)
                } else {
                    None
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let journal = AuthorityRemovalJournal {
        version: JOURNAL_VERSION,
        selectors: requested.into_iter().collect(),
        package_directories,
        marker_backups,
    };
    write_json_atomic(&paths.journal_path, &journal)
}

fn resume_at(paths: &EopkgPaths) -> Result<()> {
    if !paths.journal_path.exists() {
        return Ok(());
    }
    let journal: AuthorityRemovalJournal = serde_json::from_slice(&fs::read(&paths.journal_path)?)?;
    if journal.version != JOURNAL_VERSION {
        return Err(Error::RecoveryFailed(format!(
            "eopkg authority-removal journal version {} is unsupported",
            journal.version
        )));
    }

    fs::create_dir_all(&paths.staging_root)?;
    for directory in &journal.package_directories {
        let source = paths.package_root.join(directory);
        let staged = paths.staging_root.join(directory);
        match (source.exists(), staged.exists()) {
            (true, false) => fs::rename(&source, &staged)?,
            (false, true) => {}
            (true, true) => {
                return Err(Error::RecoveryFailed(format!(
                    "eopkg authority exists at both {} and {}",
                    source.display(),
                    staged.display()
                )));
            }
            (false, false) => {
                return Err(Error::RecoveryFailed(format!(
                    "eopkg authority is absent from both {} and {}",
                    source.display(),
                    staged.display()
                )));
            }
        }
    }

    let removed = journal
        .selectors
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for marker in MARKERS {
        let path = paths.info_root.join(marker);
        let Some(original) = journal.marker_backups.get(marker).ok_or_else(|| {
            Error::RecoveryFailed(format!("eopkg journal has no {marker} marker authority"))
        })?
        else {
            if path.exists() {
                fs::remove_file(&path)?;
            }
            continue;
        };
        let text = std::str::from_utf8(original).map_err(|error| {
            Error::ParseError(format!(
                "eopkg marker {} is not UTF-8: {error}",
                path.display()
            ))
        })?;
        let retained = text
            .split_whitespace()
            .filter(|selector| !removed.contains(*selector))
            .collect::<Vec<_>>();
        let bytes = if retained.is_empty() {
            Vec::new()
        } else {
            format!("{}\n", retained.join("\n")).into_bytes()
        };
        crate::filesystem::durable::write_file_atomic(&path, &bytes)?;
    }

    let files_db = paths.info_root.join("files.db");
    let staged_files_db = paths.staging_root.join("files.db");
    match (files_db.exists(), staged_files_db.exists()) {
        (true, false) => fs::rename(&files_db, &staged_files_db)?,
        (false, true) | (false, false) => {}
        (true, true) => {
            return Err(Error::RecoveryFailed(format!(
                "eopkg files database exists at both {} and {}",
                files_db.display(),
                staged_files_db.display()
            )));
        }
    }

    sync_directory(&paths.package_root)?;
    sync_directory(&paths.info_root)?;
    sync_directory(&paths.staging_root)?;
    remove_file_and_sync_parent(&paths.journal_path)?;
    if let Err(error) = fs::remove_dir_all(&paths.staging_root).and_then(|()| {
        crate::filesystem::durable::sync_parent_directory(&paths.staging_root)
            .map_err(std::io::Error::other)
    }) {
        warn!(
            "eopkg authority removal committed but staging cleanup at {} failed: {error}",
            paths.staging_root.display()
        );
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/eopkg/jq-metadata.xml"
    ));
    const FILES: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/eopkg/jq-files.xml"
    ));

    fn fixture_paths(root: &Path) -> EopkgPaths {
        EopkgPaths {
            package_root: root.join("package"),
            info_root: root.join("info"),
            lock_path: root.join("lock/pisi"),
            journal_path: root.join(".conary-authority-removal.json"),
            staging_root: root.join(".conary-authority-removal"),
        }
    }

    fn fixture() -> (tempfile::TempDir, EopkgPaths) {
        let root = tempfile::tempdir().unwrap();
        let paths = fixture_paths(root.path());
        fs::create_dir_all(paths.package_root.join("jq-1.8.2-13")).unwrap();
        fs::create_dir_all(&paths.info_root).unwrap();
        fs::write(
            paths.package_root.join("jq-1.8.2-13/metadata.xml"),
            METADATA,
        )
        .unwrap();
        fs::write(paths.package_root.join("jq-1.8.2-13/files.xml"), FILES).unwrap();
        fs::write(paths.info_root.join("autoinstalled"), b"jq\nretained\n").unwrap();
        fs::write(paths.info_root.join("files.db"), b"index").unwrap();
        (root, paths)
    }

    #[test]
    fn complete_authority_removal_is_batched_and_cleans_journal() {
        let (_root, paths) = fixture();
        begin_at(&paths, &["jq".to_string()]).unwrap();
        resume_at(&paths).unwrap();

        assert_eq!(fs::read_dir(&paths.package_root).unwrap().count(), 0);
        assert_eq!(
            fs::read_to_string(paths.info_root.join("autoinstalled")).unwrap(),
            "retained\n"
        );
        assert!(!paths.info_root.join("files.db").exists());
        assert!(!paths.journal_path.exists());
        assert!(!paths.staging_root.exists());
    }

    #[test]
    fn incomplete_batch_resumes_after_one_record_was_staged() {
        let (_root, paths) = fixture();
        begin_at(&paths, &["jq".to_string()]).unwrap();
        fs::create_dir(&paths.staging_root).unwrap();
        fs::rename(
            paths.package_root.join("jq-1.8.2-13"),
            paths.staging_root.join("jq-1.8.2-13"),
        )
        .unwrap();

        resume_at(&paths).unwrap();
        assert_eq!(fs::read_dir(&paths.package_root).unwrap().count(), 0);
        assert!(!paths.journal_path.exists());
    }

    #[test]
    fn partial_authority_removal_is_rejected_before_mutation() {
        let (_root, paths) = fixture();
        let error = begin_at(&paths, &[]).unwrap_err().to_string();
        assert!(error.contains("complete installed authority"));
        assert!(paths.package_root.join("jq-1.8.2-13").exists());
        assert!(!paths.journal_path.exists());
    }
}
