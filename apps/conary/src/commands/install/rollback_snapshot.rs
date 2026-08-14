// apps/conary/src/commands/install/rollback_snapshot.rs

//! Exact rollback authority for ordinary selected-root package installs.

use anyhow::{Context, Result, bail};
use conary_core::db::models::{FileEntry, Trove};
use conary_core::generation::root_manifest::SelectedRootSnapshot;
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
    rollback_root: SelectedRootSnapshot,
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
         SET metadata = ?1,
             rollback_selected_root_snapshot_id = ?2
         WHERE id = ?3
           AND metadata IS NULL
           AND rollback_selected_root_snapshot_id IS NULL",
        rusqlite::params![metadata, rollback_root.id(), changeset_id],
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
    use conary_core::ccs::native_lifecycle::{
        NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleBundle,
        ScriptletFidelity, SourceFormat, VersionScheme as LifecycleVersionScheme,
    };
    use conary_core::db::models::{
        Changeset, ChangesetStatus, InstalledNativeLifecycleBundle, TroveType,
    };
    use conary_core::generation::root_manifest::{
        CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootManifest,
        MutableStateManifest,
    };
    use conary_core::payload::{PayloadNode, PayloadNodeKind, ResolvedPayloadNode};
    use conary_core::repository::dependency_model::DebianMultiArch;
    use conary_core::repository::versioning::VersionScheme;

    fn rollback_root(conn: &rusqlite::Connection) -> SelectedRootSnapshot {
        let mut root = PayloadNode::regular(0o755);
        root.kind = PayloadNodeKind::Directory;
        root.mode = libc::S_IFDIR | 0o755;
        let captured = CapturedSelectedRoot {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: ResolvedPayloadNode::from_numeric_source(root).unwrap(),
                entries: Vec::new(),
            },
            state: MutableStateManifest::empty(),
        };
        SelectedRootSnapshot::capture(conn, &captured).unwrap()
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
            rollback_root(&conn),
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

    #[test]
    fn records_debian_lifecycle_snapshot_without_wire_name_comparison() {
        let (_root, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(&db_path).unwrap();
        let mut installed = Trove::new(
            "snapshot-deb-fixture".to_string(),
            "1.0.0-1".to_string(),
            TroveType::Package,
            VersionScheme::Debian,
        );
        installed.architecture = Some("amd64".to_string());
        installed.debian_multi_arch = Some(DebianMultiArch::No);
        installed.id = Some(installed.insert(&conn).unwrap());

        let bundle = NativeLifecycleBundle {
            schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
            schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
            source_format: SourceFormat::Deb,
            source_family: "debian".to_string(),
            source_profile: None,
            source_release: None,
            source_arch: Some("amd64".to_string()),
            source_package: installed.name.clone(),
            source_version: installed.version.clone(),
            source_checksum: None,
            version_scheme: LifecycleVersionScheme::Deb,
            conversion_tool: "conary".to_string(),
            conversion_tool_version: env!("CARGO_PKG_VERSION").to_string(),
            conversion_policy: "typed-rollback-test".to_string(),
            evidence_digest: None,
            scriptlet_fidelity: ScriptletFidelity::NativeFree,
            entries: Vec::new(),
        };
        let mut lifecycle =
            InstalledNativeLifecycleBundle::new(installed.id.unwrap(), None, &bundle).unwrap();
        lifecycle.insert_or_replace(&conn).unwrap();

        let mut changeset = Changeset::new("Upgrade Debian snapshot fixture".to_string());
        let changeset_id = changeset.insert(&conn).unwrap();
        record_install_rollback_snapshots(
            &conn,
            changeset_id,
            [&installed],
            std::iter::empty(),
            rollback_root(&conn),
            Vec::new(),
        )
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
        assert_eq!(
            snapshots[0]
                .native_lifecycle
                .as_ref()
                .unwrap()
                .source_format,
            "deb"
        );
        assert_eq!(snapshots[0].version_scheme, VersionScheme::Debian);
    }
}
