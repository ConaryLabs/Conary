// apps/remi/src/server/handlers/models/tests.rs

use super::*;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use conary_core::db::schema;
use conary_core::model::signing::sign_collection;
use ed25519_dalek::SigningKey;
use tempfile::NamedTempFile;

fn create_test_db() -> (NamedTempFile, Connection) {
    let temp_file = NamedTempFile::new().unwrap();
    let conn = Connection::open(temp_file.path()).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    schema::ensure_current(&conn).unwrap();
    (temp_file, conn)
}

fn signed_body(mut data: CollectionData) -> Vec<u8> {
    if data.content_hash.is_empty() {
        let mut hash_input = data.clone();
        hash_input.content_hash.clear();
        data.content_hash = conary_core::hash::sha256_prefixed(
            &conary_core::json::canonical_json(&hash_input).unwrap(),
        );
    }
    let key = SigningKey::from_bytes(&[11; 32]);
    let envelope = PublishedCollectionEnvelope {
        signature: BASE64.encode(sign_collection(&data, &key).unwrap()),
        public_key: hex::encode(key.verifying_key().to_bytes()),
        collection: data,
    };
    serde_json::to_vec(&envelope).unwrap()
}

#[test]
fn test_unpublished_collection_is_not_reconstructed() {
    let (temp_file, conn) = create_test_db();

    // Create a collection
    let mut trove = Trove::new(
        "group-base".to_string(),
        "1.0.0".to_string(),
        TroveType::Collection,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    trove.description = Some("Base server collection".to_string());
    let coll_id = trove.insert(&conn).unwrap();

    // Add members
    let mut m1 = CollectionMember::new(coll_id, "nginx".to_string());
    m1.insert(&conn).unwrap();

    let mut m2 = CollectionMember::new(coll_id, "redis".to_string())
        .with_version("7.0.*".to_string())
        .optional();
    m2.insert(&conn).unwrap();

    let data = build_collection_data(temp_file.path(), "group-base").unwrap();
    assert!(data.is_none());
}

#[test]
fn test_invalid_stored_collection_fails_closed() {
    let (temp_file, conn) = create_test_db();
    insert_invalid_stored_collection(&conn, "group-invalid");

    let error = build_collection_data(temp_file.path(), "group-invalid").unwrap_err();
    assert!(error.to_string().contains("invalid stored collection"));
}

#[test]
fn test_get_model_not_found() {
    let (temp_file, _conn) = create_test_db();

    let result = build_collection_data(temp_file.path(), "nonexistent").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_put_model_creates_collection() {
    let (temp_file, _conn) = create_test_db();

    let data = CollectionData {
        name: "group-web".to_string(),
        version: "1.0.0".to_string(),
        members: vec![
            CollectionMemberData {
                name: "nginx".to_string(),
                version_constraint: Some("1.24.*".to_string()),
                is_optional: false,
            },
            CollectionMemberData {
                name: "redis".to_string(),
                version_constraint: None,
                is_optional: true,
            },
        ],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: String::new(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };

    let body = signed_body(data);

    // PUT should create the collection
    let result = store_collection(temp_file.path(), "group-web", &body, false).unwrap();
    assert_eq!(result.name, "group-web");
    assert_eq!(result.version, "1.0.0");
    assert_eq!(result.members, 2);

    // Verify it can be retrieved via build_collection_data
    let fetched = build_collection_data(temp_file.path(), "group-web")
        .unwrap()
        .unwrap();
    assert_eq!(fetched.name, "group-web");
    assert_eq!(fetched.members.len(), 2);

    let nginx = fetched.members.iter().find(|m| m.name == "nginx").unwrap();
    assert_eq!(nginx.version_constraint, Some("1.24.*".to_string()));
    assert!(!nginx.is_optional);

    let redis = fetched.members.iter().find(|m| m.name == "redis").unwrap();
    assert!(redis.is_optional);

    let envelope: PublishedCollectionEnvelope = serde_json::from_slice(&body).unwrap();
    let stored_signature = query_signature(temp_file.path(), "group-web")
        .unwrap()
        .unwrap();
    assert_eq!(stored_signature.signature, envelope.signature);
    assert_eq!(stored_signature.key_id, envelope.public_key[..16]);
}

#[test]
fn test_put_model_conflict() {
    let (temp_file, conn) = create_test_db();

    // Pre-create a collection
    let mut trove = Trove::new(
        "group-existing".to_string(),
        "1.0.0".to_string(),
        TroveType::Collection,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    trove.insert(&conn).unwrap();

    let data = CollectionData {
        name: "group-existing".to_string(),
        version: "2.0.0".to_string(),
        members: vec![],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: String::new(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };

    let body = signed_body(data);

    // PUT without force should return AlreadyExists
    let result = store_collection(temp_file.path(), "group-existing", &body, false);
    assert!(matches!(result, Err(StoreError::AlreadyExists(_))));

    // PUT with force should succeed
    let result = store_collection(temp_file.path(), "group-existing", &body, true).unwrap();
    assert_eq!(result.name, "group-existing");
    assert_eq!(result.version, "2.0.0");
}

#[test]
fn test_put_model_name_mismatch() {
    let (temp_file, _conn) = create_test_db();

    let data = CollectionData {
        name: "group-a".to_string(),
        version: "1.0.0".to_string(),
        members: vec![],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: String::new(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };

    let body = signed_body(data);

    // URL name doesn't match body name
    let result = store_collection(temp_file.path(), "group-b", &body, false);
    assert!(matches!(result, Err(StoreError::NameMismatch { .. })));
}

#[test]
fn test_put_model_rejects_unsigned_collection_body() {
    let (temp_file, _conn) = create_test_db();
    let data = CollectionData {
        name: "group-unsigned".to_string(),
        version: "1.0.0".to_string(),
        members: vec![],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: "sha256:untrusted".to_string(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let body = serde_json::to_vec(&data).unwrap();

    let result = store_collection(temp_file.path(), "group-unsigned", &body, false);

    assert!(matches!(result, Err(StoreError::InvalidJson(_))));
}

#[test]
fn test_put_model_rejects_signature_from_a_different_key() {
    let (temp_file, _conn) = create_test_db();
    let data = CollectionData {
        name: "group-wrong-key".to_string(),
        version: "1.0.0".to_string(),
        members: vec![],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: String::new(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let body = signed_body(data);
    let mut envelope: PublishedCollectionEnvelope = serde_json::from_slice(&body).unwrap();
    envelope.public_key = hex::encode(SigningKey::from_bytes(&[12; 32]).verifying_key().to_bytes());

    let result = store_collection(
        temp_file.path(),
        "group-wrong-key",
        &serde_json::to_vec(&envelope).unwrap(),
        false,
    );

    assert!(matches!(
        result,
        Err(StoreError::SignatureVerificationFailed)
    ));
}

#[test]
fn test_put_model_rejects_signed_content_hash_lie() {
    let (temp_file, _conn) = create_test_db();
    let key = SigningKey::from_bytes(&[11; 32]);
    let data = CollectionData {
        name: "group-bad-hash".to_string(),
        version: "1.0.0".to_string(),
        members: vec![],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: "sha256:not-the-content".to_string(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };
    let envelope = PublishedCollectionEnvelope {
        signature: BASE64.encode(sign_collection(&data, &key).unwrap()),
        public_key: hex::encode(key.verifying_key().to_bytes()),
        collection: data,
    };

    let result = store_collection(
        temp_file.path(),
        "group-bad-hash",
        &serde_json::to_vec(&envelope).unwrap(),
        false,
    );

    assert!(matches!(result, Err(StoreError::HashMismatch { .. })));
}

#[test]
fn test_list_models() {
    let (temp_file, conn) = create_test_db();

    // Create two collections
    let mut t1 = Trove::new(
        "group-base".to_string(),
        "1.0.0".to_string(),
        TroveType::Collection,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let id1 = t1.insert(&conn).unwrap();

    let mut m = CollectionMember::new(id1, "nginx".to_string());
    m.insert(&conn).unwrap();

    let mut t2 = Trove::new(
        "group-dev".to_string(),
        "2.0.0".to_string(),
        TroveType::Collection,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    t2.description = Some("Dev tools".to_string());
    t2.insert(&conn).unwrap();

    // Also create a non-collection trove (should not appear)
    let mut pkg = Trove::new(
        "nginx".to_string(),
        "1.24.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    pkg.insert(&conn).unwrap();

    let entries = build_collection_list(temp_file.path()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "group-base");
    assert_eq!(entries[0].member_count, 1);
    assert_eq!(entries[1].name, "group-dev");
    assert_eq!(entries[1].description, Some("Dev tools".to_string()));
}
