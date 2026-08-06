// conary/tests/features/collections.rs

use super::*;

// =============================================================================
// COLLECTION TESTS
// =============================================================================

/// Test collection creation and member management
#[test]
fn test_collection_management() {
    use conary_core::db::models::{CollectionMember, Trove, TroveType};

    let (_dir, _path, mut conn) = common::create_test_db();

    db::transaction(&mut conn, |tx| {
        // Create a collection
        let mut collection = Trove::new(
            "dev-tools".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        collection.description = Some("Development tools collection".to_string());
        let collection_id = collection.insert(tx)?;

        // Add members
        let mut m1 = CollectionMember::new(collection_id, "gcc".to_string());
        m1.insert(tx)?;

        let mut m2 = CollectionMember::new(collection_id, "make".to_string());
        m2.insert(tx)?;

        let mut m3 = CollectionMember::new(collection_id, "gdb".to_string()).optional();
        m3.insert(tx)?;

        Ok(())
    })
    .unwrap();

    // Verify collection was created
    let collections = Trove::find_by_name(&conn, "dev-tools").unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].trove_type, TroveType::Collection);
    assert_eq!(
        collections[0].description,
        Some("Development tools collection".to_string())
    );

    let collection_id = collections[0].id.unwrap();

    // Verify members
    let members = CollectionMember::find_by_collection(&conn, collection_id).unwrap();
    assert_eq!(members.len(), 3);

    // Check member names (should be ordered by name)
    let names: Vec<&str> = members.iter().map(|m| m.member_name.as_str()).collect();
    assert!(names.contains(&"gcc"));
    assert!(names.contains(&"make"));
    assert!(names.contains(&"gdb"));

    // Check that gdb is optional
    let gdb_member = members.iter().find(|m| m.member_name == "gdb").unwrap();
    assert!(gdb_member.is_optional);

    // Check is_member function
    assert!(CollectionMember::is_member(&conn, collection_id, "gcc").unwrap());
    assert!(!CollectionMember::is_member(&conn, collection_id, "clang").unwrap());

    // Check find_collections_containing
    let gcc_collections = CollectionMember::find_collections_containing(&conn, "gcc").unwrap();
    assert_eq!(gcc_collections.len(), 1);
    assert_eq!(gcc_collections[0], collection_id);
}

/// Test collection operations (equivalent to cmd_collection_*)
#[test]
fn test_collection_operations() {
    use conary_core::db::models::{CollectionMember, Trove, TroveType};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Create a collection
    db::transaction(&mut conn, |tx| {
        let mut collection = Trove::new(
            "webstack".to_string(),
            "1.0.0".to_string(),
            TroveType::Collection,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        collection.description = Some("Web server stack".to_string());
        let coll_id = collection.insert(tx)?;

        // Add members
        let mut m1 = CollectionMember::new(coll_id, "nginx".to_string());
        m1.insert(tx)?;

        let mut m2 = CollectionMember::new(coll_id, "openssl".to_string());
        m2.insert(tx)?;

        Ok(())
    })
    .unwrap();

    // Verify collection
    let collections = Trove::find_by_name(&conn, "webstack").unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].trove_type, TroveType::Collection);

    let coll_id = collections[0].id.unwrap();
    let members = CollectionMember::find_by_collection(&conn, coll_id).unwrap();
    assert_eq!(members.len(), 2);

    // Test membership check
    assert!(CollectionMember::is_member(&conn, coll_id, "nginx").unwrap());
    assert!(CollectionMember::is_member(&conn, coll_id, "openssl").unwrap());
    assert!(!CollectionMember::is_member(&conn, coll_id, "postgresql").unwrap());

    // Test find_collections_containing
    let nginx_collections = CollectionMember::find_collections_containing(&conn, "nginx").unwrap();
    assert_eq!(nginx_collections.len(), 1);
    assert_eq!(nginx_collections[0], coll_id);
}
