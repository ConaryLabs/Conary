// crates/conary-core/src/model/remote/tests.rs

use super::*;
use crate::db::testing::create_test_db;
use crate::model::signing::sign_collection;
use ed25519_dalek::SigningKey;

fn test_signature(data: &CollectionData) -> (Vec<u8>, Vec<String>) {
    let key = SigningKey::from_bytes(&[7; 32]);
    (
        sign_collection(data, &key).unwrap(),
        vec![hex::encode(key.verifying_key().to_bytes())],
    )
}

#[test]
fn test_collection_data_roundtrip() {
    let data = CollectionData {
        name: "group-base".to_string(),
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
        includes: vec!["group-core@upstream:stable".to_string()],
        pins: BTreeMap::from([("openssl".to_string(), "3.0.*".to_string())]),
        exclude: vec!["sendmail".to_string()],
        content_hash: "sha256:abc123".to_string(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };

    let json = serde_json::to_string(&data).unwrap();
    let deserialized: CollectionData = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.name, "group-base");
    assert_eq!(deserialized.version, "1.0.0");
    assert_eq!(deserialized.members.len(), 2);
    assert_eq!(deserialized.members[0].name, "nginx");
    assert!(deserialized.members[1].is_optional);
    assert_eq!(deserialized.includes.len(), 1);
    assert_eq!(deserialized.pins.get("openssl"), Some(&"3.0.*".to_string()));
    assert_eq!(deserialized.exclude, vec!["sendmail"]);
}

#[test]
fn test_parse_simple_label() {
    let (repo, tag) = parse_simple_label("myrepo:stable").unwrap();
    assert_eq!(repo, "myrepo");
    assert_eq!(tag, "stable");

    let (repo, tag) = parse_simple_label("fedora:f41").unwrap();
    assert_eq!(repo, "fedora");
    assert_eq!(tag, "f41");
}

#[test]
fn test_parse_simple_label_invalid() {
    assert!(parse_simple_label("nocolon").is_err());
    assert!(parse_simple_label(":empty_repo").is_err());
    assert!(parse_simple_label("empty_tag:").is_err());
}

#[test]
fn test_resolve_label_with_repository_id() {
    let (_temp, conn) = create_test_db();

    // Create a repository
    let mut repo = Repository::new("myrepo".to_string(), "https://remi.example.com".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    // Create a label linked to the repository
    let mut label = LabelEntry::new("myrepo".to_string(), "ns".to_string(), "stable".to_string());
    label.insert(&conn).unwrap();
    label.set_repository(&conn, Some(repo_id)).unwrap();

    // Resolve should follow label -> repository -> URL
    let url = resolve_label_to_url(&conn, "group-base", "myrepo:stable").unwrap();
    assert_eq!(url, "https://remi.example.com/v1/models/group-base");
}

#[test]
fn repository_name_without_label_is_not_remote_authority() {
    let (_temp, conn) = create_test_db();

    // Create a repository with no labels pointing to it
    let mut repo = Repository::new("myrepo".to_string(), "https://remi.example.com".to_string());
    repo.insert(&conn).unwrap();

    let error = resolve_label_to_url(&conn, "group-base", "myrepo:stable").unwrap_err();
    assert!(error.to_string().contains("no exact label contract"));
}

#[test]
fn test_resolve_label_not_found() {
    let (_temp, conn) = create_test_db();

    let result = resolve_label_to_url(&conn, "group-base", "nonexistent:stable");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("nonexistent"));
}

#[tokio::test]
async fn test_fetch_uses_cache_when_fresh() {
    let (_temp, conn) = create_test_db();

    // Pre-populate cache
    let mut data = CollectionData {
        name: "group-cached".to_string(),
        version: "2.0".to_string(),
        members: vec![CollectionMemberData {
            name: "cached-pkg".to_string(),
            version_constraint: None,
            is_optional: false,
        }],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: String::new(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };
    data.content_hash = collection_content_hash(&data).unwrap();

    let data_json = serde_json::to_string(&data).unwrap();
    let mut cache_entry = RemoteCollection::new(
        "group-cached".to_string(),
        Some("repo:tag".to_string()),
        data.content_hash.clone(),
        data_json,
        "2099-12-31T23:59:59".to_string(),
    );
    let (signature, trusted_keys) = test_signature(&data);
    cache_entry.signature = Some(signature);
    cache_entry.signer_key_id = Some(trusted_keys[0][..16].to_string());
    cache_entry.upsert(&conn).unwrap();

    let result = fetch_remote_collection(&conn, "group-cached", "repo:tag", false, &trusted_keys)
        .await
        .unwrap();
    assert_eq!(result.name, "group-cached");
    assert_eq!(result.members.len(), 1);
    assert_eq!(result.members[0].name, "cached-pkg");
}

#[tokio::test]
async fn test_fetch_offline_no_cache() {
    let (_temp, conn) = create_test_db();

    // No cache entry exists, offline mode should fail
    let key = SigningKey::from_bytes(&[7; 32]);
    let trusted_keys = vec![hex::encode(key.verifying_key().to_bytes())];
    let result =
        fetch_remote_collection(&conn, "group-missing", "repo:tag", true, &trusted_keys).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("offline"));
}

#[tokio::test]
async fn test_fetch_requires_trusted_keys() {
    let (_temp, conn) = create_test_db();

    let result =
        fetch_and_verify_remote_collection(&conn, "group-missing", "repo:tag", true, &[]).await;
    let err = result.unwrap_err().to_string();
    assert!(err.contains("no trusted keys are configured"));
}

#[test]
fn test_model_to_collection_data() {
    use crate::model::parser::parse_model_string;

    let toml = r#"
[model]
version = 1
install = ["nginx", "redis", "postgresql"]
exclude = ["sendmail"]

[pin]
nginx = "1.24.*"
openssl = "3.0.*"

[optional]
packages = ["nginx-module-geoip", "redis"]

[include]
models = ["group-core@upstream:stable"]
trusted_keys = ["d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"]
"#;

    let model = parse_model_string(toml).unwrap();
    let data = build_collection_data_from_model(&model, "group-web", "2.0.0").unwrap();

    assert_eq!(data.name, "group-web");
    assert_eq!(data.version, "2.0.0");
    assert!(data.content_hash.starts_with("sha256:"));
    assert!(!data.content_hash.is_empty());

    // nginx is in install and has a pin
    let nginx = data.members.iter().find(|m| m.name == "nginx").unwrap();
    assert_eq!(nginx.version_constraint, Some("1.24.*".to_string()));
    assert!(!nginx.is_optional);

    // redis is in install AND optional
    let redis = data.members.iter().find(|m| m.name == "redis").unwrap();
    assert!(redis.is_optional);

    // postgresql is in install, not optional, no pin
    let pg = data
        .members
        .iter()
        .find(|m| m.name == "postgresql")
        .unwrap();
    assert!(!pg.is_optional);
    assert!(pg.version_constraint.is_none());

    // nginx-module-geoip is optional-only (not in install)
    let geoip = data
        .members
        .iter()
        .find(|m| m.name == "nginx-module-geoip")
        .unwrap();
    assert!(geoip.is_optional);

    // Includes are passed through
    assert_eq!(data.includes, vec!["group-core@upstream:stable"]);

    // Pins are passed through
    assert_eq!(data.pins.get("openssl"), Some(&"3.0.*".to_string()));

    // Exclude is passed through
    assert_eq!(data.exclude, vec!["sendmail"]);
}

#[test]
fn test_collection_data_to_fetched_collection() {
    let data = CollectionData {
        name: "group-test".to_string(),
        version: "1.0".to_string(),
        members: vec![
            CollectionMemberData {
                name: "pkg-a".to_string(),
                version_constraint: Some(">=2.0".to_string()),
                is_optional: false,
            },
            CollectionMemberData {
                name: "pkg-b".to_string(),
                version_constraint: None,
                is_optional: true,
            },
        ],
        includes: vec!["group-core@upstream:stable".to_string()],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: "sha256:test".to_string(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };

    let fetched = data.to_fetched_collection();
    assert_eq!(fetched.name, "group-test");
    assert_eq!(fetched.members.len(), 2);
    assert_eq!(fetched.members[0].name, "pkg-a");
    assert_eq!(
        fetched.members[0].version_constraint,
        Some(">=2.0".to_string())
    );
    assert!(!fetched.members[0].is_optional);
    assert!(fetched.members[1].is_optional);
    assert_eq!(fetched.includes.len(), 1);
}

#[test]
fn test_verify_against_trusted_keys_rejects_empty_trust_anchor_set() {
    let data = CollectionData {
        name: "group-test".to_string(),
        version: "1.0".to_string(),
        members: vec![],
        includes: vec![],
        pins: BTreeMap::new(),
        exclude: vec![],
        content_hash: "sha256:test".to_string(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
    };

    let err = verify_against_trusted_keys(&data, &[0u8; 64], &[])
        .unwrap_err()
        .to_string();
    assert!(err.contains("no trusted keys are configured"));
}
