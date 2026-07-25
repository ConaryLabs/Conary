// apps/conary/src/commands/adopt/system/live_root.rs

//! Synthetic live-root adoption and CAS capture.

use super::{
    FileInfoTuple, LIVE_ROOT_PACKAGE_NAME, capture_package_files, create_state_snapshot, open_db,
    write_db_checkpoint,
};
use anyhow::Result;
use conary_core::db::backup::CheckpointReason;
use conary_core::db::models::{
    Changeset, ChangesetStatus, InstallSource, ProvideEntry, Trove, TroveType,
};
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
            file_entry.insert_or_replace(tx)?;
        }

        let mut provide = ProvideEntry::new(
            trove_id,
            LIVE_ROOT_PACKAGE_NAME.to_string(),
            Some(live_root_adoption_version()),
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
    let walker = WalkDir::new(root).follow_links(false).into_iter();

    for entry in walker.filter_entry(|entry| should_visit_live_root_path(entry.path())) {
        let entry = entry?;
        let path = entry.path();
        if path == root || !should_visit_live_root_path(path) {
            continue;
        }

        let metadata = std::fs::symlink_metadata(path)?;
        let link_target = if metadata.file_type().is_symlink() {
            Some(std::fs::read_link(path)?.to_string_lossy().to_string())
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
        ));
    }

    Ok(files)
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
        return Ok(path.to_string_lossy().to_string());
    }
    let rel = path.strip_prefix(root)?;
    Ok(format!("/{}", rel.to_string_lossy()))
}

pub(super) fn should_visit_live_root_path(path: &Path) -> bool {
    let path = path.to_string_lossy();
    !conary_core::generation::metadata::is_excluded(&path)
        && path != "/conary"
        && !path.starts_with("/conary/")
}
