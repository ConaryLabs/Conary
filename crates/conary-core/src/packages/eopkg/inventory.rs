// conary-core/src/packages/eopkg/inventory.rs

//! Shared installed-inventory projection for retained eopkg XML authority.

use crate::error::{Error, Result};
use crate::packages::{
    InstalledConfigAuthority, InstalledFileAbsencePolicy, InstalledFileInfo,
    InstalledInventoryFile, InstalledInventoryPackage, InstalledInventorySnapshot,
    InstalledInventoryWork, NativeInstallReason, SystemPackageManager,
};
use crate::repository::dependency_model::RepositoryRequirementKind;

use super::query::InstalledEopkgDatabase;

pub(super) fn from_database(
    database: &InstalledEopkgDatabase,
) -> Result<InstalledInventorySnapshot> {
    let explicit = database.user_installed()?;
    let mut ownership_records = 0_u64;
    let mut packages = Vec::with_capacity(database.entries.len());
    for entry in database.entries.values() {
        let identity = entry.package.identity.clone();
        let is_explicit = explicit.contains(identity.selector());
        let files = entry
            .files
            .iter()
            .map(|record| {
                ownership_records = ownership_records.checked_add(1).ok_or_else(|| {
                    Error::ParseError("eopkg ownership record count overflowed u64".to_string())
                })?;
                let path = crate::packages::archive_utils::normalize_path(&record.path)
                    .map_err(|error| Error::ParseError(error.to_string()))?;
                let size = i64::try_from(record.size.unwrap_or(0)).map_err(|_| {
                    Error::ParseError(format!("eopkg installed size for {path:?} exceeds i64"))
                })?;
                let permissions = record.mode.unwrap_or(0);
                let mode = i32::try_from(libc::S_IFREG | permissions).map_err(|_| {
                    Error::ParseError(format!("eopkg installed mode for {path:?} exceeds i32"))
                })?;
                Ok(InstalledInventoryFile {
                    file: InstalledFileInfo {
                        path,
                        size,
                        mode,
                        digest: record.sha1.clone(),
                        user: record.uid.map(|value| value.to_string()),
                        group: record.gid.map(|value| value.to_string()),
                        link_target: None,
                        mtime: None,
                        absence_policy: if record.permanent {
                            InstalledFileAbsencePolicy::EopkgPermanent
                        } else {
                            InstalledFileAbsencePolicy::Required
                        },
                    },
                    config: record
                        .permanent
                        .then_some(InstalledConfigAuthority::Eopkg { permanent: true }),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let requirements = super::relation_groups(
            entry.metadata.dependencies.clone(),
            RepositoryRequirementKind::Depends,
        )?;
        let provides = super::query::project_provides(&entry.metadata, &identity)?;
        packages.push(InstalledInventoryPackage {
            identity,
            description: entry.package.info.description.clone(),
            install_reason: if is_explicit {
                NativeInstallReason::Explicit
            } else {
                NativeInstallReason::Dependency
            },
            files,
            requirements,
            provides,
            // Current eopkg authority has one transaction-global usysconf
            // event and no per-package installed script body.
            lifecycle: Vec::new(),
        });
    }
    InstalledInventorySnapshot::from_packages(
        SystemPackageManager::Eopkg,
        packages,
        InstalledInventoryWork {
            manager_processes: 0,
            database_bulk_reads: 2,
            ownership_records,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const METADATA: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/eopkg/jq-metadata.xml"
    ));
    const FILES: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/eopkg/jq-files.xml"
    ));

    fn fixture_database(automatic: &str) -> (tempfile::TempDir, InstalledEopkgDatabase) {
        let root = tempfile::tempdir().unwrap();
        let package_root = root.path().join("package");
        let package = package_root.join("jq-1.8.2-13");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("metadata.xml"), METADATA).unwrap();
        fs::write(package.join("files.xml"), FILES).unwrap();
        let automatic_path = root.path().join("info/autoinstalled");
        fs::create_dir_all(automatic_path.parent().unwrap()).unwrap();
        fs::write(&automatic_path, automatic).unwrap();
        let database = InstalledEopkgDatabase::load_roots(&package_root, automatic_path).unwrap();
        (root, database)
    }

    #[test]
    fn eopkg_inventory_is_one_fixed_database_capture() {
        let (_root, database) = fixture_database("");
        let snapshot = database.inventory_snapshot().unwrap();
        let package = snapshot.package("jq").unwrap();

        assert_eq!(package.install_reason, NativeInstallReason::Explicit);
        assert_eq!(package.files.len(), 8);
        assert_eq!(package.requirements.len(), 2);
        assert!(package.provides.iter().any(|provide| provide.name == "jq"));
        assert_eq!(
            snapshot.work(),
            InstalledInventoryWork {
                manager_processes: 0,
                database_bulk_reads: 2,
                ownership_records: 8,
            }
        );
    }

    #[test]
    fn eopkg_automatic_state_participates_in_snapshot_token() {
        let (_explicit_root, explicit_db) = fixture_database("");
        let (_automatic_root, automatic_db) = fixture_database("jq\n");
        let explicit = explicit_db.inventory_snapshot().unwrap();
        let automatic = automatic_db.inventory_snapshot().unwrap();

        assert_eq!(
            automatic.package("jq").unwrap().install_reason,
            NativeInstallReason::Dependency
        );
        assert_ne!(explicit.token(), automatic.token());
    }
}
