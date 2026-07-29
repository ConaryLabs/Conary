// apps/conary/src/commands/adopt/system/captured_root.rs

//! Exact unowned selected-root continuity for full system adoption.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use conary_core::db::models::{
    DirectoryClaim, ExistingDirectoryMaterialization, FileEntry, InstallSource, ProvideEntry,
    Trove, TroveType,
};
use conary_core::filesystem::CasStore;
use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, GenerationRootEntry, SelectedRootCaptureExclusions,
    scan_selected_root_with_exclusions,
};
use conary_core::repository::versioning::VersionScheme;
use conary_core::runtime_root::ConaryRuntimeRoot;
use rusqlite::Transaction;

use super::LIVE_ROOT_PACKAGE_NAME;

const CAPTURED_ROOT_VERSION: &str = "snapshot";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CapturedRootSync {
    pub(super) captured_entries: usize,
    pub(super) package_entries: usize,
    pub(super) changed: bool,
}

pub(super) fn ensure_complete_native_partition<'a>(
    tracked_packages: &HashMap<String, Trove>,
    installed_selectors: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    let installed = installed_selectors.into_iter().collect::<HashSet<_>>();
    let mut stale = tracked_packages
        .iter()
        .filter(|(selector, trove)| {
            trove.install_source == InstallSource::AdoptedTrack
                && !installed.contains(selector.as_str())
        })
        .map(|(selector, _)| selector.clone())
        .collect::<Vec<_>>();
    stale.sort();
    if stale.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "Complete full-system adoption found track-only native identities absent from the current package-manager inventory: {}. Refresh or unadopt that stale authority before retrying",
        stale.join(", ")
    ))
}

pub(super) fn capture_live_selected_root(
    db_path: &str,
    cas: &CasStore,
) -> Result<CapturedSelectedRoot> {
    let exclusions = capture_exclusions(db_path)?;
    scan_selected_root_with_exclusions(Path::new("/"), cas, &exclusions)
        .map_err(anyhow::Error::from)
        .context("failed to capture exact selected-root continuity state")
}

pub(super) fn synchronize_captured_root(
    tx: &Transaction<'_>,
    changeset_id: i64,
    captured: &CapturedSelectedRoot,
) -> conary_core::Result<CapturedRootSync> {
    let mut entries = captured
        .generation
        .entries
        .iter()
        .chain(&captured.state.entries)
        .cloned()
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    let captured_troves = Trove::list_all(tx)?
        .into_iter()
        .filter(|trove| trove.install_source == InstallSource::CapturedRoot)
        .collect::<Vec<_>>();
    if let Some(sync) = matching_existing_authority(tx, &entries, &captured_troves)? {
        return Ok(sync);
    }

    // Deleting the prior synthetic authority is transaction-local. The schema
    // reanchors shared directory materializations to surviving package claims
    // before cascading paths that only the captured root owned.
    for trove in captured_troves {
        let trove_id = trove.id.ok_or_else(|| {
            conary_core::Error::MissingId(format!(
                "captured-root trove {} has no database identity",
                trove.name
            ))
        })?;
        Trove::delete(tx, trove_id)?;
    }

    let captured_trove_id = insert_captured_root_trove(tx, changeset_id)?;
    let mut captured_entries = 0;
    let mut package_entries = 0;
    for entry in entries {
        if FileEntry::find_by_path(tx, &entry.path)?.is_some() {
            FileEntry::reconcile_selected_root_materialization(
                tx,
                &entry.path,
                &entry.node,
                entry.content.as_ref(),
            )?;
            package_entries += 1;
        } else {
            let mut file = FileEntry::new(entry.path, entry.node, entry.content, captured_trove_id);
            file.insert_or_replace(tx, ExistingDirectoryMaterialization::ApplyIncoming)?;
            captured_entries += 1;
        }
    }

    Ok(CapturedRootSync {
        captured_entries,
        package_entries,
        changed: true,
    })
}

fn matching_existing_authority(
    tx: &Transaction<'_>,
    entries: &[GenerationRootEntry],
    captured_troves: &[Trove],
) -> conary_core::Result<Option<CapturedRootSync>> {
    let [captured_trove] = captured_troves else {
        return Ok(None);
    };
    if captured_trove.name != LIVE_ROOT_PACKAGE_NAME
        || captured_trove.version != CAPTURED_ROOT_VERSION
    {
        return Ok(None);
    }
    let captured_trove_id = captured_trove.id.ok_or_else(|| {
        conary_core::Error::MissingId(format!(
            "captured-root trove {} has no database identity",
            captured_trove.name
        ))
    })?;
    let captured_paths = FileEntry::find_by_trove(tx, captured_trove_id)?
        .into_iter()
        .map(|file| file.path)
        .chain(
            DirectoryClaim::find_by_trove(tx, captured_trove_id)?
                .into_iter()
                .map(|claim| claim.path),
        )
        .collect::<HashSet<_>>();
    let expected_paths = entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<HashSet<_>>();
    if captured_paths
        .iter()
        .any(|path| !expected_paths.contains(path.as_str()))
    {
        return Ok(None);
    }

    let mut captured_entries = 0;
    let mut package_entries = 0;
    for entry in entries {
        let Some(existing) = FileEntry::find_by_path(tx, &entry.path)? else {
            return Ok(None);
        };
        if existing.node != entry.node || existing.content != entry.content {
            return Ok(None);
        }
        let claims = DirectoryClaim::find_retaining_path(tx, &entry.path)?;
        let has_package_owner = existing.trove_id != captured_trove_id
            || claims
                .iter()
                .any(|claim| claim.trove_id != captured_trove_id);
        let has_captured_owner = existing.trove_id == captured_trove_id
            || claims
                .iter()
                .any(|claim| claim.trove_id == captured_trove_id);
        if has_package_owner {
            if has_captured_owner {
                return Ok(None);
            }
            package_entries += 1;
        } else {
            if !has_captured_owner {
                return Ok(None);
            }
            captured_entries += 1;
        }
    }

    Ok(Some(CapturedRootSync {
        captured_entries,
        package_entries,
        changed: false,
    }))
}

fn insert_captured_root_trove(tx: &Transaction<'_>, changeset_id: i64) -> conary_core::Result<i64> {
    let mut trove = Trove::new_with_source(
        LIVE_ROOT_PACKAGE_NAME.to_string(),
        CAPTURED_ROOT_VERSION.to_string(),
        TroveType::Package,
        InstallSource::CapturedRoot,
        VersionScheme::Conary,
    );
    trove.architecture = Some(std::env::consts::ARCH.to_string());
    trove.description =
        Some("Exact selected-root continuity state without a package owner".to_string());
    trove.installed_by_changeset_id = Some(changeset_id);
    trove.selection_reason = Some("Captured during complete full-system adoption".to_string());
    let trove_id = trove.insert(tx)?;

    let mut provide = ProvideEntry::new(
        trove_id,
        LIVE_ROOT_PACKAGE_NAME.to_string(),
        Some(CAPTURED_ROOT_VERSION.to_string()),
        VersionScheme::Conary,
    );
    provide.insert_or_ignore(tx)?;
    Ok(trove_id)
}

fn capture_exclusions(db_path: &str) -> Result<SelectedRootCaptureExclusions> {
    let runtime = ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let runtime_root = absolute_normalized(runtime.root())?;
    let database = absolute_normalized(runtime.db_path())?;
    if runtime_root == Path::new("/") {
        return Err(anyhow::anyhow!(
            "Conary runtime root cannot be / during selected-root capture"
        ));
    }

    let mut exclusions = Vec::new();
    push_exclusion(&mut exclusions, &runtime_root)?;
    push_database_exclusions(&mut exclusions, &database)?;

    if let Some(resolved_runtime_root) = resolve_existing_authority(&runtime_root)? {
        if resolved_runtime_root == Path::new("/") {
            return Err(anyhow::anyhow!(
                "Conary runtime root {} resolves to / during selected-root capture",
                runtime_root.display()
            ));
        }
        push_exclusion(&mut exclusions, &resolved_runtime_root)?;
    }
    if let Some(resolved_database_parent) =
        resolve_existing_authority(database.parent().unwrap_or(Path::new("/")))?
        && resolved_database_parent != Path::new("/")
    {
        push_exclusion(&mut exclusions, &resolved_database_parent)?;
    }
    if let Some(resolved_database) = resolve_existing_authority(&database)? {
        push_database_exclusions(&mut exclusions, &resolved_database)?;
    }

    SelectedRootCaptureExclusions::new(exclusions)
        .map_err(anyhow::Error::from)
        .context("invalid Conary runtime capture boundary")
}

fn push_database_exclusions(exclusions: &mut Vec<String>, database: &Path) -> Result<()> {
    if let Some(parent) = database.parent().filter(|parent| *parent != Path::new("/")) {
        push_exclusion(exclusions, parent)?;
    }
    let database = database.to_str().with_context(|| {
        format!(
            "Conary database exclusion is not valid UTF-8: {}",
            database.display()
        )
    })?;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        exclusions.push(format!("{database}{suffix}"));
    }
    Ok(())
}

fn push_exclusion(exclusions: &mut Vec<String>, path: &Path) -> Result<()> {
    if path == Path::new("/") {
        return Err(anyhow::anyhow!(
            "selected-root capture cannot exclude the complete root"
        ));
    }
    exclusions.push(
        path.to_str()
            .with_context(|| {
                format!(
                    "Conary runtime exclusion is not valid UTF-8: {}",
                    path.display()
                )
            })?
            .to_string(),
    );
    Ok(())
}

fn resolve_existing_authority(path: &Path) -> Result<Option<PathBuf>> {
    match std::fs::canonicalize(path) {
        Ok(resolved) => absolute_normalized(&resolved).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to resolve Conary runtime authority {}",
                path.display()
            )
        }),
    }
}

fn absolute_normalized(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::RootDir => normalized.push("/"),
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized != Path::new("/") {
                    normalized.pop();
                }
            }
            Component::Prefix(_) => {
                return Err(anyhow::anyhow!(
                    "Conary runtime path has an unsupported platform prefix: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Changeset, ChangesetStatus};
    use conary_core::generation::root_manifest::{
        SelectedRootCaptureExclusions, scan_selected_root_with_exclusions,
    };
    use conary_core::payload::PayloadNodeKind;
    use std::os::unix::fs::PermissionsExt;

    fn insert_changeset(tx: &Transaction<'_>, description: &str) -> i64 {
        let mut changeset = Changeset::new(description.to_string());
        let id = changeset.insert(tx).unwrap();
        changeset
            .update_status(tx, ChangesetStatus::Applied)
            .unwrap();
        id
    }

    #[test]
    fn full_capture_partitions_package_and_unowned_authority_exactly() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let cas = CasStore::new(temp.path().join("objects")).unwrap();
        for directory in [
            "conary/objects",
            "etc/systemd/system/multi-user.target.wants",
            "run",
            "usr/bin",
            "var/lib/conary",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join("conary/objects/private"), b"runtime").unwrap();
        std::fs::write(root.join("run/private"), b"ephemeral").unwrap();
        std::fs::write(root.join("var/lib/conary/conary.db"), b"runtime").unwrap();
        std::fs::write(root.join("etc/owned-primary"), b"hardlinked").unwrap();
        std::fs::hard_link(
            root.join("etc/owned-primary"),
            root.join("etc/unowned-link"),
        )
        .unwrap();
        std::fs::set_permissions(
            root.join("etc/owned-primary"),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "/usr/sbin/sshd",
            root.join("etc/systemd/system/multi-user.target.wants/sshd.service"),
        )
        .unwrap();
        std::fs::write(root.join("usr/bin/owned"), b"package").unwrap();

        let exclusions = SelectedRootCaptureExclusions::new(vec![
            "/conary".to_string(),
            "/var/lib/conary".to_string(),
        ])
        .unwrap();
        let captured = scan_selected_root_with_exclusions(&root, &cas, &exclusions).unwrap();
        let entries = captured
            .generation
            .entries
            .iter()
            .chain(&captured.state.entries)
            .map(|entry| (entry.path.as_str(), entry))
            .collect::<std::collections::HashMap<_, _>>();

        let db_path = temp.path().join("conary.db");
        conary_core::db::init(&db_path).unwrap();
        let mut conn = conary_core::db::open(&db_path).unwrap();
        let tx = conn.transaction().unwrap();
        let changeset_id = insert_changeset(&tx, "seed package");
        let mut package = Trove::new_with_source(
            "native-package".to_string(),
            "1-1".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            VersionScheme::Rpm,
        );
        package.architecture = Some("x86_64".to_string());
        package.native_package_identity = Some(
            conary_core::packages::InstalledPackageIdentity::rpm(
                "native-package-1-1.x86_64",
                "native-package",
                None,
                "1",
                "1",
                "x86_64",
            )
            .unwrap(),
        );
        package.installed_by_changeset_id = Some(changeset_id);
        let package_id = package.insert(&tx).unwrap();
        for path in [
            "/etc",
            "/etc/owned-primary",
            "/usr",
            "/usr/bin",
            "/usr/bin/owned",
        ] {
            let entry = entries[path];
            let mut node = entry.node.clone();
            if path == "/etc/owned-primary" {
                node.source.kind = PayloadNodeKind::Regular {
                    hardlink_identity: None,
                };
            }
            let mut file =
                FileEntry::new(path.to_string(), node, entry.content.clone(), package_id);
            file.insert_or_replace(&tx, ExistingDirectoryMaterialization::ApplyIncoming)
                .unwrap();
        }

        let first_changeset = insert_changeset(&tx, "capture selected root");
        let first = synchronize_captured_root(&tx, first_changeset, &captured).unwrap();
        assert!(first.changed);
        assert!(first.captured_entries > 0);
        assert!(first.package_entries >= 5);

        let captured_trove = Trove::list_all(&tx)
            .unwrap()
            .into_iter()
            .find(|trove| trove.install_source == InstallSource::CapturedRoot)
            .unwrap();
        let captured_id = captured_trove.id.unwrap();
        assert_eq!(
            FileEntry::find_by_path(&tx, "/etc/owned-primary")
                .unwrap()
                .unwrap()
                .trove_id,
            package_id
        );
        assert_eq!(
            FileEntry::find_by_path(&tx, "/etc/unowned-link")
                .unwrap()
                .unwrap()
                .trove_id,
            captured_id
        );
        let primary = FileEntry::find_by_path(&tx, "/etc/owned-primary")
            .unwrap()
            .unwrap();
        let linked = FileEntry::find_by_path(&tx, "/etc/unowned-link")
            .unwrap()
            .unwrap();
        let PayloadNodeKind::Regular {
            hardlink_identity: Some(primary_identity),
        } = primary.node.source.kind
        else {
            panic!("package-owned hardlink primary lost its typed identity");
        };
        let PayloadNodeKind::Hardlink { identity, target } = linked.node.source.kind else {
            panic!("captured-root hardlink lost its typed link authority");
        };
        assert_eq!(identity, primary_identity);
        assert_eq!(target, "/etc/owned-primary");
        assert!(
            FileEntry::find_by_path(
                &tx,
                "/etc/systemd/system/multi-user.target.wants/sshd.service"
            )
            .unwrap()
            .is_some()
        );
        for excluded in ["/conary", "/run", "/var/lib/conary"] {
            assert!(
                FileEntry::find_all_ordered(&tx)
                    .unwrap()
                    .iter()
                    .all(|file| file.path != excluded
                        && !file.path.starts_with(&format!("{excluded}/")))
            );
        }

        let second_changeset = insert_changeset(&tx, "repeat selected-root capture");
        let second = synchronize_captured_root(&tx, second_changeset, &captured).unwrap();
        assert!(!second.changed);
        assert_eq!(second.captured_entries, first.captured_entries);
        assert_eq!(second.package_entries, first.package_entries);
        assert_eq!(
            Trove::list_all(&tx)
                .unwrap()
                .iter()
                .filter(|trove| trove.install_source == InstallSource::CapturedRoot)
                .count(),
            1
        );
        tx.commit().unwrap();
    }

    #[test]
    fn runtime_capture_exclusions_follow_default_and_isolated_database_roots() {
        let default = capture_exclusions("/var/lib/conary/conary.db").unwrap();
        assert!(default.excludes("/conary/objects/aa"));
        assert!(default.excludes("/var/lib/conary/conary.db"));
        assert!(default.excludes("/var/lib/conary/conary.db-wal"));
        assert!(!default.excludes("/var/lib/conary-adjacent/state"));

        let isolated = capture_exclusions("target/captured-root-test/conary.db").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let isolated_root = cwd.join("target/captured-root-test");
        assert!(isolated.excludes(isolated_root.to_str().unwrap()));
    }

    #[test]
    fn runtime_capture_exclusions_cover_lexical_and_resolved_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let resolved = temp.path().join("resolved-runtime");
        let alias = temp.path().join("runtime-alias");
        std::fs::create_dir_all(&resolved).unwrap();
        std::fs::write(resolved.join("conary.db"), b"database").unwrap();
        std::os::unix::fs::symlink(&resolved, &alias).unwrap();

        let exclusions = capture_exclusions(alias.join("conary.db").to_str().unwrap()).unwrap();

        assert!(exclusions.excludes(alias.join("objects").to_str().unwrap()));
        assert!(exclusions.excludes(resolved.join("objects").to_str().unwrap()));
        assert!(exclusions.excludes(resolved.join("conary.db-wal").to_str().unwrap()));
    }

    #[test]
    fn runtime_capture_rejects_a_root_runtime_authority() {
        let error = capture_exclusions("/conary.db").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("runtime root cannot be / during selected-root capture"),
            "{error:#}"
        );
    }

    #[test]
    fn complete_root_capture_detects_stale_track_only_authority() {
        fn trove(name: &str, source: InstallSource) -> Trove {
            Trove::new_with_source(
                name.to_string(),
                "1-1".to_string(),
                TroveType::Package,
                source,
                VersionScheme::Rpm,
            )
        }

        let tracked = HashMap::from([
            (
                "current-1-1.x86_64".to_string(),
                trove("current", InstallSource::AdoptedTrack),
            ),
            (
                "stale-1-1.x86_64".to_string(),
                trove("stale", InstallSource::AdoptedTrack),
            ),
            (
                "repository-1-1.x86_64".to_string(),
                trove("repository", InstallSource::Repository),
            ),
        ]);

        let error = ensure_complete_native_partition(&tracked, ["current-1-1.x86_64"]).unwrap_err();

        assert!(error.to_string().contains("stale-1-1.x86_64"), "{error:#}");
        assert!(
            !error.to_string().contains("repository-1-1.x86_64"),
            "{error:#}"
        );
    }
}
