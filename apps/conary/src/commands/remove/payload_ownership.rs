// apps/conary/src/commands/remove/payload_ownership.rs
//! Exact package-path and materialized-removal authority.

pub(crate) use conary_core::db::models::PackagePayloadOwnership;

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{
        ExistingDirectoryMaterialization, FileEntry, PayloadClaim, Trove, TroveType,
    };
    use conary_core::payload::{
        PayloadContentAuthority, PayloadNode, PayloadSharingPolicy, ResolvedPayloadNode,
    };
    use conary_core::repository::versioning::VersionScheme;

    fn insert_trove(conn: &rusqlite::Connection, name: &str) -> i64 {
        Trove::new(
            name.to_string(),
            "1".to_string(),
            TroveType::Package,
            VersionScheme::Conary,
        )
        .insert(conn)
        .unwrap()
    }

    fn directory(path: &str, trove_id: i64) -> FileEntry {
        let mut node = PayloadNode::regular(0o755);
        node.kind = conary_core::payload::PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        FileEntry::new(
            path.to_string(),
            ResolvedPayloadNode::from_numeric_source(node).unwrap(),
            None,
            trove_id,
        )
    }

    #[test]
    fn every_claim_is_a_lifecycle_path_but_only_the_last_claim_removes_the_directory() {
        let (_root, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(db_path).unwrap();
        let first = insert_trove(&conn, "first");
        let second = insert_trove(&conn, "second");

        directory("/etc", first).insert(&conn).unwrap();
        PayloadClaim::new_directory(
            "/etc".to_string(),
            second,
            directory("/unused", second).node,
        )
        .unwrap()
        .insert(&conn)
        .unwrap();

        for trove_id in [first, second] {
            let ownership = PackagePayloadOwnership::load(&conn, trove_id).unwrap();
            assert_eq!(ownership.lifecycle_paths(), &["/etc"]);
            assert!(ownership.materialized_removal_paths().is_empty());
        }

        Trove::delete(&conn, first).unwrap();
        let ownership = PackagePayloadOwnership::load(&conn, second).unwrap();
        assert_eq!(ownership.materialized_removal_paths(), &["/etc"]);
    }

    #[test]
    fn regular_files_and_last_claim_directories_are_materialized_removals() {
        let (_root, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(db_path).unwrap();
        let trove_id = insert_trove(&conn, "owner");
        directory("/usr/share/owner", trove_id)
            .insert(&conn)
            .unwrap();
        let mut file = FileEntry::new(
            "/usr/share/owner/data".to_string(),
            ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap(),
            Some(conary_core::payload::PayloadContentAuthority {
                sha256: conary_core::hash::sha256(b"data"),
                size: 4,
            }),
            trove_id,
        );
        file.insert(&conn).unwrap();

        let ownership = PackagePayloadOwnership::load(&conn, trove_id).unwrap();
        assert_eq!(
            ownership.lifecycle_paths(),
            &["/usr/share/owner", "/usr/share/owner/data"]
        );
        assert_eq!(
            ownership.materialized_removal_paths(),
            &["/usr/share/owner", "/usr/share/owner/data"]
        );
    }

    #[test]
    fn shared_regular_payload_is_removed_only_by_the_final_claimant() {
        let (_root, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(db_path).unwrap();
        let first = insert_trove(&conn, "first");
        let second = insert_trove(&conn, "second");
        let content = PayloadContentAuthority {
            sha256: conary_core::hash::sha256(b"shared"),
            size: 6,
        };
        let node = ResolvedPayloadNode::from_numeric_source(PayloadNode::regular(0o644)).unwrap();
        let mut first_entry = FileEntry::new(
            "/usr/share/shared".to_string(),
            node.clone(),
            Some(content.clone()),
            first,
        )
        .with_claim_policy(PayloadSharingPolicy::Rpm);
        first_entry.insert(&conn).unwrap();
        let mut second_entry =
            FileEntry::new("/usr/share/shared".to_string(), node, Some(content), second)
                .with_claim_policy(PayloadSharingPolicy::Rpm);
        second_entry
            .insert_or_replace(&conn, ExistingDirectoryMaterialization::ApplyIncoming)
            .unwrap();

        for trove_id in [first, second] {
            let ownership = PackagePayloadOwnership::load(&conn, trove_id).unwrap();
            assert_eq!(ownership.lifecycle_paths(), &["/usr/share/shared"]);
            assert!(ownership.materialized_removal_paths().is_empty());
        }

        Trove::delete(&conn, first).unwrap();
        assert_eq!(
            PackagePayloadOwnership::load(&conn, second)
                .unwrap()
                .materialized_removal_paths(),
            &["/usr/share/shared"]
        );
    }

    #[test]
    fn last_claim_nonempty_directory_is_retained_and_actual_stats_report_zero() {
        let (_db_root, db_path) = crate::commands::test_helpers::create_test_db();
        let conn = conary_core::db::open(db_path).unwrap();
        let trove_id = insert_trove(&conn, "owner");
        directory("/usr/share/owner", trove_id)
            .insert(&conn)
            .unwrap();
        let ownership = PackagePayloadOwnership::load(&conn, trove_id).unwrap();

        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let root = temp.path().join("root");
        std::fs::create_dir_all(root.join("usr/share/owner")).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(root.join("usr/share/owner/untracked-child"), b"retained").unwrap();
        let mut transaction = crate::commands::LiveRootTransaction::begin(
            &runtime,
            &root,
            uuid::Uuid::new_v4().to_string(),
            "remove last directory claim",
        )
        .unwrap();

        let stats = transaction
            .apply_remove_paths(ownership.materialized_removal_paths())
            .unwrap();

        assert_eq!(stats.dirs_removed, 0);
        assert!(root.join("usr/share/owner").is_dir());
        transaction.rollback().unwrap();
    }
}
