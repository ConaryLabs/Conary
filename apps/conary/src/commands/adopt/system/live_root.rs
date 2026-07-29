// apps/conary/src/commands/adopt/system/live_root.rs

//! Synthetic live-root adoption and CAS capture.

use super::{
    FileInfoTuple, LIVE_ROOT_PACKAGE_NAME, capture_package_files, create_state_snapshot, open_db,
    write_db_checkpoint,
};
use anyhow::{Context, Result};
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{
    Changeset, ChangesetStatus, ExistingDirectoryMaterialization, InstallSource, ProvideEntry,
    Trove, TroveType,
};
use conary_core::packages::InstalledFileAbsencePolicy;
use std::path::Path;
use walkdir::WalkDir;

pub(super) fn adopt_live_root_as_full_package(
    db_path: &str,
    dry_run: bool,
    pattern: Option<&str>,
    exclude: Option<&str>,
    explicit_only: bool,
) -> Result<()> {
    if pattern.is_some() || exclude.is_some() || explicit_only {
        return Err(anyhow::anyhow!(
            "Full live-root adoption without a system package manager does not support package filters"
        ));
    }

    println!(
        "No supported package manager found; adopting the live root as one CAS-backed package"
    );

    let mut conn = open_db(db_path)?;
    let tracked_packages: std::collections::HashSet<String> = Trove::list_all(&conn)?
        .into_iter()
        .map(|t| t.name)
        .collect();
    if tracked_packages.contains(LIVE_ROOT_PACKAGE_NAME) {
        println!("Live root is already adopted as {LIVE_ROOT_PACKAGE_NAME}; nothing to do");
        return Ok(());
    }

    let files = collect_live_root_file_info(Path::new("/"))?;
    if dry_run {
        println!("Dry run: would adopt 1 synthetic package");
        println!("  Package: {LIVE_ROOT_PACKAGE_NAME}");
        println!("  Files: {}", files.len());
        println!("  Mode: full (CAS storage)");
        return Ok(());
    }

    let objects_dir = conary_core::db::paths::objects_dir(db_path);
    let cas = conary_core::filesystem::CasStore::new(&objects_dir)?;
    let captured_files = capture_package_files(LIVE_ROOT_PACKAGE_NAME, &files, Some(&cas))?;
    let mut changeset = Changeset::new(format!(
        "Adopt live root as CAS-backed package ({LIVE_ROOT_PACKAGE_NAME})"
    ));
    write_db_checkpoint(db_path, CheckpointReason::PreMutation)?;
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let changeset_id = changeset.insert(tx)?;
        let mut trove = Trove::new_with_source(
            LIVE_ROOT_PACKAGE_NAME.to_string(),
            live_root_adoption_version(),
            TroveType::Package,
            InstallSource::CapturedRoot,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some(std::env::consts::ARCH.to_string());
        trove.description = Some(
            "Synthetic CAS-backed package imported from a system without a native package manager"
                .to_string(),
        );
        trove.installed_by_changeset_id = Some(changeset_id);
        trove.selection_reason = Some(
            "Captured live root because no supported package manager was available".to_string(),
        );
        let trove_id = trove.insert(tx)?;

        for captured in &captured_files {
            let mut file_entry = captured.file_entry(trove_id);
            file_entry.insert_or_replace(tx, ExistingDirectoryMaterialization::ApplyIncoming)?;
        }

        let mut provide = ProvideEntry::new(
            trove_id,
            LIVE_ROOT_PACKAGE_NAME.to_string(),
            Some(live_root_adoption_version()),
            trove.version_scheme,
        );
        provide.insert_or_ignore(tx)?;

        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok(changeset_id)
    })?;

    create_state_snapshot(
        &conn,
        changeset_id,
        &format!("Adopt live root as {LIVE_ROOT_PACKAGE_NAME}"),
    )?;
    write_db_checkpoint(db_path, CheckpointReason::PostSuccess)?;

    println!(
        "Adopted live root as {LIVE_ROOT_PACKAGE_NAME}: {} files (full)",
        captured_files.len()
    );

    Ok(())
}

fn live_root_adoption_version() -> String {
    "snapshot".to_string()
}

fn collect_live_root_file_info(root: &Path) -> Result<Vec<FileInfoTuple>> {
    let mut files = Vec::new();
    let mut walker = WalkDir::new(root).follow_links(false).into_iter();

    while let Some(entry) = walker.next() {
        let entry = entry?;
        let path = entry.path();
        if !should_visit_live_root_path(path)? {
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }
        if path == root {
            continue;
        }

        let metadata = std::fs::symlink_metadata(path)?;
        let link_target = if metadata.file_type().is_symlink() {
            Some(read_live_root_link_target(path)?)
        } else {
            None
        };
        let size = if let Some(target) = &link_target {
            i64::try_from(target.len())?
        } else {
            i64::try_from(metadata.len())?
        };

        files.push((
            live_root_db_path(root, path)?,
            size,
            live_root_file_mode(&metadata),
            None,
            Some("root".to_string()),
            Some("root".to_string()),
            link_target,
            InstalledFileAbsencePolicy::Required,
        ));
    }

    Ok(files)
}

fn read_live_root_link_target(path: &Path) -> Result<String> {
    let target = std::fs::read_link(path)?;
    target.into_os_string().into_string().map_err(|target| {
        anyhow::anyhow!(
            "Cannot adopt symlink {} because its target is not valid UTF-8: {:?}",
            path.display(),
            target
        )
    })
}

#[cfg(unix)]
fn live_root_file_mode(metadata: &std::fs::Metadata) -> i32 {
    use std::os::unix::fs::PermissionsExt;

    #[allow(clippy::cast_possible_wrap)]
    {
        metadata.permissions().mode() as i32
    }
}

#[cfg(not(unix))]
fn live_root_file_mode(_metadata: &std::fs::Metadata) -> i32 {
    0
}

pub(super) fn live_root_db_path(root: &Path, path: &Path) -> Result<String> {
    if root == Path::new("/") {
        return path.to_str().map(str::to_string).with_context(|| {
            format!(
                "Cannot adopt live-root path {} because it is not valid UTF-8",
                path.display()
            )
        });
    }
    let rel = path.strip_prefix(root)?;
    let rel = rel.to_str().with_context(|| {
        format!(
            "Cannot adopt live-root path {} because it is not valid UTF-8",
            path.display()
        )
    })?;
    Ok(format!("/{rel}"))
}

pub(super) fn should_visit_live_root_path(path: &Path) -> Result<bool> {
    let path = path.to_str().with_context(|| {
        format!(
            "Cannot classify live-root path {} because it is not valid UTF-8",
            path.display()
        )
    })?;
    Ok(!conary_core::generation::metadata::is_excluded(path)
        && path != "/conary"
        && !path.starts_with("/conary/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    #[test]
    fn live_root_paths_with_distinct_invalid_bytes_fail_closed() {
        let root = Path::new("/captured");
        let first = root.join(OsString::from_vec(vec![b'n', b'a', b'm', b'e', 0x80]));
        let second = root.join(OsString::from_vec(vec![b'n', b'a', b'm', b'e', 0x81]));

        assert!(live_root_db_path(root, &first).is_err());
        assert!(live_root_db_path(root, &second).is_err());
    }

    #[test]
    fn live_root_symlink_with_invalid_byte_target_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        symlink(
            OsString::from_vec(vec![b't', b'a', b'r', b'g', b'e', b't', 0x80]),
            root.path().join("link"),
        )
        .unwrap();

        let error = read_live_root_link_target(&root.path().join("link")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("because its target is not valid UTF-8"),
            "{error:#}"
        );
    }
}
