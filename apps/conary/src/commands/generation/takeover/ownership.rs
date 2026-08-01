// apps/conary/src/commands/generation/takeover/ownership.rs

//! Exact native package-manager ownership transfer for system takeover.

use crate::commands::adopt::{
    FileInfoTuple,
    cas_capture::{CapturedAdoptionFile, capture_package_files},
};
use crate::commands::open_db;
use anyhow::{Result, anyhow};
use conary_core::db::models::{
    Changeset, ChangesetStatus, ExistingDirectoryMaterialization, FileEntry, InstallSource, Trove,
};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::CasStore;
use conary_core::packages::{
    InstalledPackageIdentity, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use rusqlite::params;
use tracing::warn;

#[derive(Debug, Default)]
pub(super) struct OwnershipTransferReport {
    pub(super) query_failures: Vec<String>,
    pub(super) pm_removal_failures: Vec<String>,
}

impl OwnershipTransferReport {
    pub(super) fn is_complete(&self) -> bool {
        self.query_failures.is_empty() && self.pm_removal_failures.is_empty()
    }
}

#[derive(Debug)]
struct PreparedTakeoverEntry {
    identity: InstalledPackageIdentity,
    files: Vec<CapturedAdoptionFile>,
}

fn prepare_takeover_entry(
    identity: InstalledPackageIdentity,
    files: Vec<FileInfoTuple>,
    cas: &CasStore,
) -> Result<PreparedTakeoverEntry> {
    let files = capture_package_files(identity.selector(), &files, Some(cas))?;
    Ok(PreparedTakeoverEntry { identity, files })
}

/// Upgrade `AdoptedTrack` packages to `AdoptedFull` by privately capturing
/// their exact live payload in the CAS and updating the DB.
pub(super) fn upgrade_to_cas_backed(
    db_path: &str,
    packages: &[String],
    pm: &SystemPackageManager,
) -> Result<Vec<String>> {
    let cas = CasStore::new(objects_dir(db_path))?;

    let mut entries: Vec<PreparedTakeoverEntry> = Vec::with_capacity(packages.len());
    let mut skipped = Vec::new();
    for selector in packages {
        let identity = match query_package_identity(*pm, selector) {
            Ok(identity) => identity,
            Err(error) => {
                warn!("Skipping CAS upgrade for '{selector}': {error}");
                skipped.push(format!("{selector}: {error}"));
                continue;
            }
        };
        let files = match query_package_files(*pm, selector) {
            Ok(files) => files,
            Err(error) => {
                warn!("Skipping CAS upgrade for '{selector}': {error}");
                skipped.push(format!("{selector}: {error}"));
                continue;
            }
        };

        match prepare_takeover_entry(identity, files, &cas) {
            Ok(entry) => entries.push(entry),
            Err(error) => {
                warn!("Skipping CAS upgrade for '{selector}': {error}");
                skipped.push(format!("{selector}: {error}"));
            }
        }
    }
    if !skipped.is_empty() {
        return Ok(skipped);
    }

    // DB-only transaction: all PM queries and CAS writes are already done.
    let mut conn = open_db(db_path)?;
    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Takeover: CAS-upgrade track-only packages".into());
        changeset.insert(tx)?;
        let changeset_id = changeset.id.ok_or_else(|| {
            conary_core::Error::MissingId("changeset insert did not return an ID".into())
        })?;

        for entry in &entries {
            let trove = find_trove_for_identity(tx, &entry.identity)?.ok_or_else(|| {
                conary_core::Error::NotFound(format!(
                    "trove '{}' disappeared before CAS-upgrade commit",
                    entry.identity.selector()
                ))
            })?;
            let trove_id = trove.id.ok_or_else(|| {
                conary_core::Error::MissingId(format!(
                    "trove '{}' from DB has no ID",
                    entry.identity.selector()
                ))
            })?;

            // Replace discovery-only records with the exact payload captured
            // before this database transaction.
            for captured in &entry.files {
                if FileEntry::find_by_path(tx, &captured.source.0)?
                    .is_some_and(|file| file.trove_id == trove_id)
                {
                    let mut file = captured.file_entry(trove_id);
                    file.insert_or_replace(tx, ExistingDirectoryMaterialization::ApplyIncoming)?;
                }
            }

            tx.execute(
                "UPDATE troves SET install_source = ?1, installed_by_changeset_id = ?2 \
                 WHERE id = ?3",
                params![InstallSource::AdoptedFull.as_str(), changeset_id, trove_id],
            )?;
        }

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })?;

    Ok(Vec::new())
}

/// Mark packages as Conary-owned, then remove their native-manager records.
///
/// All native queries and CAS captures must succeed before the DB transaction.
/// A reported native-manager removal failure is compensated back to
/// `AdoptedFull`, so Conary never falsely claims sole authority.
pub(super) fn take_ownership(
    db_path: &str,
    packages: &[String],
    pm: SystemPackageManager,
) -> Result<OwnershipTransferReport> {
    let cas = CasStore::new(objects_dir(db_path))?;

    let mut entries: Vec<PreparedTakeoverEntry> = Vec::with_capacity(packages.len());
    let mut report = OwnershipTransferReport::default();
    for selector in packages {
        let identity = match query_package_identity(pm, selector) {
            Ok(identity) => identity,
            Err(error) => {
                warn!("Ownership identity query failed for '{selector}': {error}");
                report.query_failures.push(format!("{selector}: {error}"));
                continue;
            }
        };
        match query_package_files(pm, selector) {
            Ok(files) => match prepare_takeover_entry(identity, files, &cas) {
                Ok(entry) => entries.push(entry),
                Err(error) => {
                    warn!("Ownership capture failed for '{selector}': {error}");
                    report.query_failures.push(format!("{selector}: {error}"));
                }
            },
            Err(error) => {
                warn!("Ownership query failed for '{selector}': {error}");
                report.query_failures.push(format!("{selector}: {error}"));
            }
        }
    }
    if !report.query_failures.is_empty() {
        return Ok(report);
    }

    // Mark first so a crash after successful PM removal cannot leave an
    // unowned payload. Reported removal failures are compensated below.
    {
        let mut conn = open_db(db_path)?;
        conary_core::db::transaction(&mut conn, |tx| {
            let mut changeset = Changeset::new("Takeover: take ownership from system PM".into());
            changeset.insert(tx)?;
            let changeset_id = changeset
                .id
                .ok_or_else(|| conary_core::Error::MissingId("changeset".into()))?;

            for entry in &entries {
                let trove = find_trove_for_identity(tx, &entry.identity)?.ok_or_else(|| {
                    conary_core::Error::NotFound(format!(
                        "trove '{}' disappeared before ownership commit",
                        entry.identity.selector()
                    ))
                })?;
                let trove_id = trove.id.ok_or_else(|| {
                    conary_core::Error::MissingId(format!("trove '{}'", entry.identity.selector()))
                })?;

                if trove.install_source == InstallSource::AdoptedTrack {
                    for captured in &entry.files {
                        if FileEntry::find_by_path(tx, &captured.source.0)?
                            .is_some_and(|file| file.trove_id == trove_id)
                        {
                            let mut file = captured.file_entry(trove_id);
                            file.insert_or_replace(
                                tx,
                                ExistingDirectoryMaterialization::ApplyIncoming,
                            )?;
                        }
                    }
                }

                tx.execute(
                    "UPDATE troves SET install_source = ?1, installed_by_changeset_id = ?2 \
                     WHERE id = ?3",
                    params![InstallSource::Taken.as_str(), changeset_id, trove_id],
                )?;
            }

            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok(())
        })?;
    }

    let mut failed_identities = Vec::new();
    for entry in &entries {
        println!(
            "  Removing {} from {} database ...",
            entry.identity.selector(),
            pm.display_name()
        );
        if let Err(error) = remove_from_system_pm(pm, entry.identity.selector()) {
            warn!(
                "Failed to remove {} from PM: {error}",
                entry.identity.selector()
            );
            report
                .pm_removal_failures
                .push(format!("{}: {error}", entry.identity.selector()));
            failed_identities.push(entry.identity.clone());
        }
    }
    if !failed_identities.is_empty() {
        restore_native_manager_authority(db_path, &failed_identities)?;
    }

    Ok(report)
}

fn restore_native_manager_authority(
    db_path: &str,
    packages: &[InstalledPackageIdentity],
) -> Result<()> {
    let mut conn = open_db(db_path)?;
    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset =
            Changeset::new("Takeover: restore authority after native PM removal failure".into());
        changeset.insert(tx)?;
        let changeset_id = changeset.id.ok_or_else(|| {
            conary_core::Error::MissingId("ownership compensation changeset".into())
        })?;

        for identity in packages {
            let trove = find_trove_for_identity(tx, identity)?.ok_or_else(|| {
                conary_core::Error::NotFound(format!(
                    "trove '{}' disappeared before ownership compensation",
                    identity.selector()
                ))
            })?;
            let trove_id = trove.id.ok_or_else(|| {
                conary_core::Error::MissingId(format!("trove '{}' has no ID", identity.selector()))
            })?;
            tx.execute(
                "UPDATE troves SET install_source = ?1, installed_by_changeset_id = ?2 \
                 WHERE id = ?3",
                params![InstallSource::AdoptedFull.as_str(), changeset_id, trove_id],
            )?;
        }
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(())
    })?;
    Ok(())
}

fn find_trove_for_identity(
    connection: &rusqlite::Connection,
    identity: &InstalledPackageIdentity,
) -> conary_core::Result<Option<Trove>> {
    let mut matches = Trove::find_by_name(connection, identity.name())?
        .into_iter()
        .filter(|trove| trove.native_package_identity.as_ref() == Some(identity))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => Err(conary_core::Error::ConflictError(format!(
            "native identity '{}' matches {count} Conary troves",
            identity.selector()
        ))),
    }
}

fn query_package_identity(
    package_manager: SystemPackageManager,
    selector: &str,
) -> Result<InstalledPackageIdentity> {
    match package_manager {
        SystemPackageManager::Rpm => Ok(rpm_query::query_package(selector)?.identity),
        SystemPackageManager::Dpkg => Ok(dpkg_query::query_package(selector)?.identity),
        SystemPackageManager::Pacman => Ok(pacman_query::query_package(selector)?.identity),
        SystemPackageManager::Unknown => Err(anyhow!("No supported package manager")),
    }
}

fn query_package_files(
    package_manager: SystemPackageManager,
    name: &str,
) -> Result<Vec<FileInfoTuple>> {
    let raw_files = match package_manager {
        SystemPackageManager::Rpm => rpm_query::query_package_files(name)
            .map_err(|error| anyhow!("RPM file query failed for '{name}': {error}"))?,
        SystemPackageManager::Dpkg => dpkg_query::query_package_files(name)
            .map_err(|error| anyhow!("DPKG file query failed for '{name}': {error}"))?,
        SystemPackageManager::Pacman => pacman_query::query_package_files(name)
            .map_err(|error| anyhow!("Pacman file query failed for '{name}': {error}"))?,
        SystemPackageManager::Unknown => {
            return Err(anyhow!("No supported package manager"));
        }
    };
    Ok(raw_files
        .into_iter()
        .map(|file| {
            (
                file.path,
                file.size,
                file.mode,
                file.digest,
                file.user,
                file.group,
                file.link_target,
                file.absence_policy,
            )
        })
        .collect())
}

fn remove_from_system_pm(package_manager: SystemPackageManager, name: &str) -> Result<()> {
    match package_manager {
        SystemPackageManager::Rpm => {
            rpm_query::remove_from_db_only(name).map_err(|error| anyhow!("{error}"))
        }
        SystemPackageManager::Dpkg => {
            dpkg_query::remove_from_db_only(name).map_err(|error| anyhow!("{error}"))
        }
        SystemPackageManager::Pacman => {
            pacman_query::remove_from_db_only(name).map_err(|error| anyhow!("{error}"))
        }
        SystemPackageManager::Unknown => Err(anyhow!("No supported package manager")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db;
    use conary_core::db::models::TroveType;

    fn file_tuple(
        path: &str,
        size: i64,
        mode: i32,
        digest: Option<&str>,
        link_target: Option<&str>,
    ) -> FileInfoTuple {
        (
            path.to_string(),
            size,
            mode,
            digest.map(str::to_string),
            Some("root".to_string()),
            Some("root".to_string()),
            link_target.map(str::to_string),
            conary_core::packages::InstalledFileAbsencePolicy::Required,
        )
    }

    #[test]
    fn ownership_entry_rejects_missing_regular_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let files = vec![file_tuple(
            "/usr/bin/missing",
            7,
            0o100755,
            Some("package-manager-digest"),
            None,
        )];

        let identity =
            InstalledPackageIdentity::pacman("broken", "broken", "1-1", "x86_64").unwrap();
        let error = prepare_takeover_entry(identity, files, &cas)
            .unwrap_err()
            .to_string();

        assert!(error.contains("package broken"));
        assert!(error.contains("/usr/bin/missing"));
        assert!(error.contains("failed to capture exact node"));
    }

    #[test]
    fn takeover_cas_capture_uses_typed_live_symlink() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        let link = temp.path().join("libfoo.so");
        std::os::unix::fs::symlink("libfoo.so.1", &link).unwrap();
        let files = vec![file_tuple(
            link.to_str().unwrap(),
            0,
            0o120777,
            Some("package-manager-digest"),
            Some("wrong-discovery-target"),
        )];

        let identity = InstalledPackageIdentity::pacman("glibc", "glibc", "1-1", "x86_64").unwrap();
        let entry = prepare_takeover_entry(identity, files, &cas).unwrap();

        assert!(entry.files[0].content.is_none());
        assert_eq!(
            entry.files[0].node.source.kind,
            conary_core::payload::PayloadNodeKind::Symlink {
                target: "libfoo.so.1".to_string()
            }
        );
    }

    #[test]
    fn ownership_lookup_selects_only_the_requested_multiarch_variant() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("conary.db");
        db::init(db_path.to_str().unwrap()).unwrap();
        let mut connection = db::open(db_path.to_str().unwrap()).unwrap();
        let (amd64_id, i386_id, i386_identity) = db::transaction(&mut connection, |tx| {
            let mut changeset = Changeset::new("seed multiarch adoption".into());
            let changeset_id = changeset.insert(tx)?;
            let amd64_identity =
                InstalledPackageIdentity::dpkg("libc6:amd64", "libc6", "2.42-1", "amd64")?;
            let i386_identity =
                InstalledPackageIdentity::dpkg("libc6:i386", "libc6", "2.42-1", "i386")?;

            let mut amd64 = Trove::new_with_source(
                "libc6".into(),
                "2.42-1".into(),
                TroveType::Package,
                InstallSource::AdoptedFull,
                conary_core::repository::versioning::VersionScheme::Debian,
            );
            amd64.architecture = Some("amd64".into());
            amd64.debian_multi_arch =
                Some(conary_core::repository::dependency_model::DebianMultiArch::Same);
            amd64.installed_by_changeset_id = Some(changeset_id);
            amd64.native_package_identity = Some(amd64_identity);
            let amd64_id = amd64.insert(tx)?;

            let mut i386 = Trove::new_with_source(
                "libc6".into(),
                "2.42-1".into(),
                TroveType::Package,
                InstallSource::AdoptedFull,
                conary_core::repository::versioning::VersionScheme::Debian,
            );
            i386.architecture = Some("i386".into());
            i386.debian_multi_arch =
                Some(conary_core::repository::dependency_model::DebianMultiArch::Same);
            i386.installed_by_changeset_id = Some(changeset_id);
            i386.native_package_identity = Some(i386_identity.clone());
            let i386_id = i386.insert(tx)?;
            changeset.update_status(tx, ChangesetStatus::Applied)?;
            Ok((amd64_id, i386_id, i386_identity))
        })
        .unwrap();

        let matched = find_trove_for_identity(&connection, &i386_identity)
            .unwrap()
            .unwrap();
        assert_eq!(matched.id, Some(i386_id));
        assert_ne!(matched.id, Some(amd64_id));
    }
}
