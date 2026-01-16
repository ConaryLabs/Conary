// src/db/models/mod.rs

//! Data models for Conary database entities
//!
//! This module defines Rust structs that correspond to database tables
//! and provides methods for creating, reading, updating, and deleting records.

mod changeset;
mod chunk_access;
mod collection;
mod component;
mod component_dependency;
mod config;
mod converted;
mod delta;
mod dependency;
mod derived;
mod file_entry;
mod flavor;
mod label;
mod provenance;
mod provide_entry;
mod repository;
mod scriptlet_entry;
mod state;
mod trigger;
mod trove;

pub use changeset::{Changeset, ChangesetStatus};
pub use chunk_access::{ChunkAccess, ChunkStats};
pub use collection::CollectionMember;
pub use component::Component;
pub use config::{ConfigBackup, ConfigFile, ConfigSource, ConfigStatus};
pub use component_dependency::{ComponentDependency, ComponentDepType, ComponentProvide};
pub use converted::{ConvertedPackage, CONVERSION_VERSION};
pub use delta::{DeltaStats, PackageDelta};
pub use dependency::DependencyEntry;
pub use derived::{DerivedPackage, DerivedPatch, DerivedOverride, DerivedStatus, VersionPolicy};
pub use file_entry::FileEntry;
pub use flavor::Flavor;
pub use label::{LabelEntry, LabelPathEntry, add_to_path, get_label_path, remove_from_path};
pub use provenance::Provenance;
pub use provide_entry::ProvideEntry;
pub use repository::{Repository, RepositoryPackage};
pub use scriptlet_entry::ScriptletEntry;
pub use state::{RestorePlan, StateDiff, StateEngine, StateMember, SystemState};
pub use trigger::{ChangesetTrigger, Trigger, TriggerDependency, TriggerEngine, TriggerStatus};
pub use trove::{InstallReason, InstallSource, Trove, TroveType};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;
    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    #[test]
    fn test_trove_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        trove.description = Some("A test package".to_string());

        let id = trove.insert(&conn).unwrap();
        assert!(id > 0);
        assert_eq!(trove.id, Some(id));

        // Find by ID
        let found = Trove::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.name, "test-package");
        assert_eq!(found.version, "1.0.0");
        assert_eq!(found.trove_type, TroveType::Package);

        // Find by name
        let by_name = Trove::find_by_name(&conn, "test-package").unwrap();
        assert_eq!(by_name.len(), 1);

        // List all
        let all = Trove::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        Trove::delete(&conn, id).unwrap();
        let deleted = Trove::find_by_id(&conn, id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_changeset_crud() {
        let (_temp, conn) = create_test_db();

        // Create a changeset
        let mut changeset = Changeset::new("Install test-package".to_string());
        let id = changeset.insert(&conn).unwrap();
        assert!(id > 0);
        assert_eq!(changeset.status, ChangesetStatus::Pending);

        // Find by ID
        let found = Changeset::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(found.description, "Install test-package");
        assert_eq!(found.status, ChangesetStatus::Pending);

        // Update status
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();
        let updated = Changeset::find_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(updated.status, ChangesetStatus::Applied);
        assert!(updated.applied_at.is_some());

        // List all
        let all = Changeset::list_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_file_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first (foreign key requirement)
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create a file
        let mut file = FileEntry::new(
            "/usr/bin/test".to_string(),
            "abc123def456".to_string(),
            1024,
            0o755,
            trove_id,
        );
        file.owner = Some("root".to_string());

        let id = file.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by path
        let found = FileEntry::find_by_path(&conn, "/usr/bin/test")
            .unwrap()
            .unwrap();
        assert_eq!(found.sha256_hash, "abc123def456");
        assert_eq!(found.size, 1024);

        // Find by trove
        let files = FileEntry::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(files.len(), 1);

        // Delete
        FileEntry::delete(&conn, "/usr/bin/test").unwrap();
        let deleted = FileEntry::find_by_path(&conn, "/usr/bin/test").unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with a file
        let mut trove = Trove::new(
            "test-package".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut file = FileEntry::new(
            "/usr/bin/test".to_string(),
            "abc123".to_string(),
            1024,
            0o755,
            trove_id,
        );
        file.insert(&conn).unwrap();

        // Delete the trove - file should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify file is gone
        let file_exists = FileEntry::find_by_path(&conn, "/usr/bin/test").unwrap();
        assert!(file_exists.is_none());
    }

    #[test]
    fn test_flavor_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create flavors
        let mut flavor1 = Flavor::new(trove_id, "ssl".to_string(), "enabled".to_string());
        let id1 = flavor1.insert(&conn).unwrap();
        assert!(id1 > 0);

        let mut flavor2 = Flavor::new(trove_id, "http3".to_string(), "enabled".to_string());
        flavor2.insert(&conn).unwrap();

        // Find by trove
        let flavors = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(flavors.len(), 2);
        assert_eq!(flavors[0].key, "http3"); // Ordered by key
        assert_eq!(flavors[1].key, "ssl");

        // Find by key
        let ssl_flavors = Flavor::find_by_key(&conn, "ssl").unwrap();
        assert_eq!(ssl_flavors.len(), 1);
        assert_eq!(ssl_flavors[0].value, "enabled");

        // Delete
        Flavor::delete(&conn, id1).unwrap();
        let remaining = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].key, "http3");
    }

    #[test]
    fn test_provenance_crud() {
        let (_temp, conn) = create_test_db();

        // Create a trove first
        let mut trove = Trove::new(
            "nginx".to_string(),
            "1.21.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        // Create provenance
        let mut prov = Provenance::new(trove_id);
        prov.source_url = Some("https://github.com/nginx/nginx".to_string());
        prov.source_branch = Some("main".to_string());
        prov.source_commit = Some("abc123def456".to_string());
        prov.build_host = Some("builder01.example.com".to_string());
        prov.builder = Some("builder-bot".to_string());

        let id = prov.insert(&conn).unwrap();
        assert!(id > 0);

        // Find by trove
        let found = Provenance::find_by_trove(&conn, trove_id).unwrap().unwrap();
        assert_eq!(
            found.source_url,
            Some("https://github.com/nginx/nginx".to_string())
        );
        assert_eq!(found.source_commit, Some("abc123def456".to_string()));
        assert_eq!(found.builder, Some("builder-bot".to_string()));

        // Update
        let mut updated_prov = found.clone();
        updated_prov.source_commit = Some("new_commit_hash".to_string());
        updated_prov.update(&conn).unwrap();

        let reloaded = Provenance::find_by_trove(&conn, trove_id).unwrap().unwrap();
        assert_eq!(reloaded.source_commit, Some("new_commit_hash".to_string()));

        // Delete
        Provenance::delete(&conn, trove_id).unwrap();
        let deleted = Provenance::find_by_trove(&conn, trove_id).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_flavor_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with flavors
        let mut trove = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut flavor = Flavor::new(trove_id, "feature".to_string(), "enabled".to_string());
        flavor.insert(&conn).unwrap();

        // Delete the trove - flavors should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify flavors are gone
        let flavors = Flavor::find_by_trove(&conn, trove_id).unwrap();
        assert_eq!(flavors.len(), 0);
    }

    #[test]
    fn test_provenance_cascade_delete() {
        let (_temp, conn) = create_test_db();

        // Create a trove with provenance
        let mut trove = Trove::new(
            "test-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
        );
        let trove_id = trove.insert(&conn).unwrap();

        let mut prov = Provenance::new(trove_id);
        prov.source_url = Some("https://example.com".to_string());
        prov.insert(&conn).unwrap();

        // Delete the trove - provenance should be cascade deleted
        Trove::delete(&conn, trove_id).unwrap();

        // Verify provenance is gone
        let prov_exists = Provenance::find_by_trove(&conn, trove_id).unwrap();
        assert!(prov_exists.is_none());
    }
}
