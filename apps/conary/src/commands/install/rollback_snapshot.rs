// apps/conary/src/commands/install/rollback_snapshot.rs

//! Exact rollback authority for ordinary selected-root package installs.

use anyhow::{Context, Result, bail};
use conary_core::db::models::{FileEntry, Trove};
use conary_core::generation::root_manifest::CapturedSelectedRoot;
use conary_core::transaction::PackageRelationRemoval;
use std::collections::BTreeMap;

/// Persist snapshots of every installed trove that this install replaces.
///
/// The remove subsystem owns the canonical typed snapshot. Keeping install
/// transactions on that same authority ensures upgrade rollback restores the
/// same package state as rollback of an explicit removal.
pub(super) fn record_install_rollback_snapshots<'a>(
    conn: &rusqlite::Connection,
    changeset_id: i64,
    upgraded_troves: impl IntoIterator<Item = &'a Trove>,
    relation_removals: impl IntoIterator<Item = &'a PackageRelationRemoval>,
    rollback_root: CapturedSelectedRoot,
    materialized_directories: Vec<FileEntry>,
) -> Result<()> {
    let mut replaced = BTreeMap::new();
    for trove in upgraded_troves {
        let trove_id = trove.id.with_context(|| {
            format!(
                "upgrade target '{}-{}' has no trove ID",
                trove.name, trove.version
            )
        })?;
        insert_exact_trove(
            conn,
            &mut replaced,
            trove_id,
            &trove.name,
            &trove.version,
            trove.architecture.as_deref(),
        )?;
    }
    for removal in relation_removals {
        insert_exact_trove(
            conn,
            &mut replaced,
            removal.trove_id,
            &removal.package_name,
            &removal.package_version,
            removal.package_architecture.as_deref(),
        )?;
    }

    let snapshots = replaced
        .values()
        .map(|trove| crate::commands::remove::snapshot_trove(conn, trove))
        .collect::<Result<Vec<_>>>()?;
    let materialized_directories =
        crate::commands::capture_materialized_directory_snapshots(conn, materialized_directories)?;
    let system = crate::commands::RollbackSystemAuthority::capture(conn)?;
    let metadata = crate::commands::metadata_with_removed_troves(
        snapshots,
        materialized_directories,
        rollback_root,
        system,
    )?;
    let updated = conn.execute(
        "UPDATE changesets
         SET metadata = ?1
         WHERE id = ?2 AND metadata IS NULL",
        rusqlite::params![metadata, changeset_id],
    )?;
    if updated != 1 {
        bail!(
            "install changeset {changeset_id} already has metadata before rollback authority was recorded"
        );
    }
    Ok(())
}

fn insert_exact_trove(
    conn: &rusqlite::Connection,
    replaced: &mut BTreeMap<i64, Trove>,
    trove_id: i64,
    expected_name: &str,
    expected_version: &str,
    expected_architecture: Option<&str>,
) -> Result<()> {
    let trove = Trove::find_by_id(conn, trove_id)?
        .with_context(|| format!("install replacement target {trove_id} disappeared"))?;
    if trove.name != expected_name
        || trove.version != expected_version
        || trove.architecture.as_deref() != expected_architecture
    {
        bail!(
            "install replacement target {trove_id} changed after exact relation and upgrade planning"
        );
    }
    if let Some(existing) = replaced.get(&trove_id)
        && (existing.name != trove.name
            || existing.version != trove.version
            || existing.architecture != trove.architecture)
    {
        bail!("install replacement target {trove_id} has conflicting planned identities");
    }
    replaced.insert(trove_id, trove);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{Changeset, ChangesetStatus, TroveType};
    use conary_core::generation::root_manifest::{
        GENERATION_ROOT_MANIFEST_VERSION, GenerationRootManifest, MutableStateManifest,
    };
    use conary_core::payload::{PayloadNode, PayloadNodeKind, ResolvedPayloadNode};
    use conary_core::repository::versioning::VersionScheme;

    fn rollback_root() -> CapturedSelectedRoot {
        let mut root = PayloadNode::regular(0o755);
        root.kind = PayloadNodeKind::Directory;
        root.mode = libc::S_IFDIR | 0o755;
        CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: ResolvedPayloadNode::from_numeric_source(root).unwrap(),
                entries: Vec::new(),
            },
            state: MutableStateManifest::empty(),
        }
    }

    #[test]
    fn records_exact_upgrade_snapshot_on_new_changeset() {
        let (_root, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut installed = Trove::new(
            "snapshot-fixture".to_string(),
            "1.0.0-1".to_string(),
            TroveType::Package,
            VersionScheme::Rpm,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.id = Some(installed.insert(&conn).unwrap());

        let mut changeset = Changeset::new("Upgrade snapshot fixture".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        record_install_rollback_snapshots(
            &conn,
            changeset_id,
            [&installed],
            std::iter::empty(),
            rollback_root(),
            Vec::new(),
        )
        .unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();

        let metadata: String = conn
            .query_row(
                "SELECT metadata FROM changesets WHERE id = ?1",
                [changeset_id],
                |row| row.get(0),
            )
            .unwrap();
        let snapshots = crate::commands::parse_rollback_snapshots(&metadata).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].name, "snapshot-fixture");
        assert_eq!(snapshots[0].version, "1.0.0-1");
        assert_eq!(snapshots[0].version_scheme, VersionScheme::Rpm);
    }
}
