// conary-core/src/trust/client/tests.rs

use super::*;
use crate::ccs::signing::SigningKeyPair;
use crate::db::testing::create_test_db;
use crate::trust::TargetDescription;
use crate::trust::ceremony::create_initial_root_single_key;
use crate::trust::generate::{generate_snapshot, generate_targets, generate_timestamp};
use crate::trust::keys::sign_tuf_metadata;
use std::path::{Path, PathBuf};

struct StaticMetadataFixture {
    _tempdir: tempfile::TempDir,
    metadata_dir: PathBuf,
    key: SigningKeyPair,
    root: Signed<RootMetadata>,
    snapshot: Signed<SnapshotMetadata>,
    targets: Signed<TargetsMetadata>,
}

impl StaticMetadataFixture {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let metadata_dir = tempdir.path().join("metadata");
        std::fs::create_dir_all(&metadata_dir).unwrap();

        let key = SigningKeyPair::generate();
        let root = create_initial_root_single_key(&key, 365).unwrap();
        let targets = generate_targets(&[], &key, 1, 30).unwrap();
        let snapshot = generate_snapshot(root.signed.version, &targets, &key, 1, 7).unwrap();
        let timestamp = generate_timestamp(&snapshot, &key, 1, 6).unwrap();

        write_signed_metadata(&metadata_dir, "root.json", &root);
        write_signed_metadata(&metadata_dir, "targets.json", &targets);
        write_signed_metadata(&metadata_dir, "snapshot.json", &snapshot);
        write_signed_metadata(&metadata_dir, "timestamp.json", &timestamp);

        Self {
            _tempdir: tempdir,
            metadata_dir,
            key,
            root,
            snapshot,
            targets,
        }
    }

    fn client(&self, repo_id: i64) -> TufClient {
        let metadata_url = format!("file://{}", self.metadata_dir.display());
        TufClient::new_static(repo_id, "file:///unused", Some(&metadata_url)).unwrap()
    }

    fn generic_client(&self, repo_id: i64) -> TufClient {
        let metadata_url = format!("file://{}", self.metadata_dir.display());
        TufClient::new(repo_id, "file:///unused", Some(&metadata_url)).unwrap()
    }

    fn bootstrap(&self, client: &TufClient, conn: &Connection) {
        let root_json = serde_json::to_vec(&self.root).unwrap();
        client.bootstrap(conn, &root_json).unwrap();
    }

    fn write_greater_snapshot_without_root(&self) {
        let mut snapshot =
            generate_snapshot(self.root.signed.version, &self.targets, &self.key, 2, 7).unwrap();
        snapshot.signed.meta.remove("root.json");
        snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
        let timestamp = generate_timestamp(&snapshot, &self.key, 2, 6).unwrap();

        write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn write_expired_timestamp(&self) {
        let snapshot =
            generate_snapshot(self.root.signed.version, &self.targets, &self.key, 1, 7).unwrap();
        let timestamp = generate_timestamp(&snapshot, &self.key, 1, -1).unwrap();
        write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn write_same_version_timestamp_with_different_bytes(&self) {
        let snapshot =
            generate_snapshot(self.root.signed.version, &self.targets, &self.key, 1, 7).unwrap();
        let mut timestamp = generate_timestamp(&snapshot, &self.key, 1, 6).unwrap();
        timestamp.signed.expires += chrono::Duration::seconds(1);
        timestamp.signatures = vec![sign_tuf_metadata(&self.key, &timestamp.signed).unwrap()];
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn write_timestamp_for_stored_snapshot_version(
        &self,
        conn: &Connection,
        repo_id: i64,
        version: u64,
    ) {
        let snapshot: Signed<SnapshotMetadata> =
            load_stored_signed_metadata(conn, repo_id, "snapshot");
        let timestamp = generate_timestamp(&snapshot, &self.key, version, 6).unwrap();
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn write_timestamp_for_cached_snapshot_with_bad_hash(&self, version: u64) {
        let mut timestamp = generate_timestamp(&self.snapshot, &self.key, version, 6).unwrap();
        set_bad_hash(timestamp.signed.meta.get_mut("snapshot.json").unwrap());
        timestamp.signatures = vec![sign_tuf_metadata(&self.key, &timestamp.signed).unwrap()];
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn write_timestamp_for_cached_snapshot_without_hash(&self, version: u64) {
        let mut timestamp = generate_timestamp(&self.snapshot, &self.key, version, 6).unwrap();
        timestamp
            .signed
            .meta
            .get_mut("snapshot.json")
            .unwrap()
            .hashes = None;
        timestamp.signatures = vec![sign_tuf_metadata(&self.key, &timestamp.signed).unwrap()];
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn write_snapshot_for_cached_targets_with_bad_hash(&self) {
        let mut snapshot =
            generate_snapshot(self.root.signed.version, &self.targets, &self.key, 2, 7).unwrap();
        set_bad_hash(snapshot.signed.meta.get_mut("targets.json").unwrap());
        snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
        let timestamp = generate_timestamp(&snapshot, &self.key, 2, 6).unwrap();

        write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn write_snapshot_for_cached_targets_without_hash(&self) {
        let mut snapshot =
            generate_snapshot(self.root.signed.version, &self.targets, &self.key, 2, 7).unwrap();
        snapshot.signed.meta.get_mut("targets.json").unwrap().hashes = None;
        snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
        let timestamp = generate_timestamp(&snapshot, &self.key, 2, 6).unwrap();

        write_signed_metadata(&self.metadata_dir, "snapshot.json", &snapshot);
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
    }

    fn expire_stored_snapshot(&self, conn: &Connection, repo_id: i64) {
        let mut snapshot: Signed<SnapshotMetadata> =
            load_stored_signed_metadata(conn, repo_id, "snapshot");
        snapshot.signed.expires = chrono::Utc::now() - chrono::Duration::hours(1);
        snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
        update_stored_signed_metadata(conn, repo_id, "snapshot", &snapshot);

        let timestamp = generate_timestamp(&snapshot, &self.key, 1, 6).unwrap();
        write_signed_metadata(&self.metadata_dir, "timestamp.json", &timestamp);
        update_stored_signed_metadata(conn, repo_id, "timestamp", &timestamp);
    }

    fn expire_stored_targets(&self, conn: &Connection, repo_id: i64) {
        let mut targets: Signed<TargetsMetadata> =
            load_stored_signed_metadata(conn, repo_id, "targets");
        targets.signed.expires = chrono::Utc::now() - chrono::Duration::hours(1);
        targets.signatures = vec![sign_tuf_metadata(&self.key, &targets.signed).unwrap()];
        update_stored_signed_metadata(conn, repo_id, "targets", &targets);

        let mut snapshot: Signed<SnapshotMetadata> =
            load_stored_signed_metadata(conn, repo_id, "snapshot");
        let targets_json = serde_json::to_string(&targets).unwrap();
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert(
            "sha256".to_string(),
            metadata_hash_for_persistence(&targets).unwrap(),
        );
        let targets_ref = snapshot.signed.meta.get_mut("targets.json").unwrap();
        targets_ref.length = Some(targets_json.len() as u64);
        targets_ref.hashes = Some(hashes);
        snapshot.signatures = vec![sign_tuf_metadata(&self.key, &snapshot.signed).unwrap()];
        update_stored_signed_metadata(conn, repo_id, "snapshot", &snapshot);
    }

    fn alter_stored_targets_same_version(&self, conn: &Connection, repo_id: i64) {
        let mut targets: Signed<TargetsMetadata> =
            load_stored_signed_metadata(conn, repo_id, "targets");
        targets.signed.expires += chrono::Duration::seconds(1);
        targets.signatures = vec![sign_tuf_metadata(&self.key, &targets.signed).unwrap()];
        update_stored_signed_metadata(conn, repo_id, "targets", &targets);
    }

    fn expire_stored_root(&self, conn: &Connection, repo_id: i64) {
        let mut root: Signed<RootMetadata> = conn
            .query_row(
                "SELECT signed_metadata FROM tuf_roots
                     WHERE repository_id = ?1
                     ORDER BY version DESC LIMIT 1",
                params![repo_id],
                |row| {
                    let json: String = row.get(0)?;
                    Ok(serde_json::from_str(&json).unwrap())
                },
            )
            .unwrap();
        root.signed.expires = chrono::Utc::now() - chrono::Duration::hours(1);
        root.signatures = vec![sign_tuf_metadata(&self.key, &root.signed).unwrap()];
        let json = serde_json::to_string(&root).unwrap();
        conn.execute(
            "UPDATE tuf_roots
                 SET signed_metadata = ?1, expires_at = ?2
                 WHERE repository_id = ?3 AND version = ?4",
            params![
                json,
                root.signed.expires.to_rfc3339(),
                repo_id,
                root.signed.version as i64,
            ],
        )
        .unwrap();
        update_stored_signed_metadata(conn, repo_id, "root", &root);
    }
}

fn write_signed_metadata<T: serde::Serialize>(
    metadata_dir: &Path,
    filename: &str,
    signed: &Signed<T>,
) {
    let bytes = serde_json::to_vec(signed).unwrap();
    std::fs::write(metadata_dir.join(filename), bytes).unwrap();
}

fn set_bad_hash(meta: &mut crate::trust::MetaFile) {
    let mut hashes = std::collections::BTreeMap::new();
    hashes.insert("sha256".to_string(), "bad-hash".to_string());
    meta.hashes = Some(hashes);
}

fn load_stored_signed_metadata<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    repo_id: i64,
    role: &str,
) -> Signed<T> {
    let json: String = conn
        .query_row(
            "SELECT signed_metadata FROM tuf_metadata WHERE repository_id = ?1 AND role = ?2",
            params![repo_id, role],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&json).unwrap()
}

fn update_stored_signed_metadata<T: serde::Serialize + TufMetadataFields>(
    conn: &Connection,
    repo_id: i64,
    role: &str,
    signed: &Signed<T>,
) {
    let json = serde_json::to_string(signed).unwrap();
    let hash = metadata_hash_for_persistence(signed).unwrap();
    conn.execute(
        "UPDATE tuf_metadata
             SET signed_metadata = ?1, metadata_hash = ?2, expires_at = ?3
             WHERE repository_id = ?4 AND role = ?5",
        params![
            json,
            hash,
            signed.signed.expires().to_rfc3339(),
            repo_id,
            role,
        ],
    )
    .unwrap();
}

fn stored_metadata_version(conn: &Connection, repo_id: i64, role: &str) -> i64 {
    conn.query_row(
        "SELECT version FROM tuf_metadata WHERE repository_id = ?1 AND role = ?2",
        params![repo_id, role],
        |row| row.get(0),
    )
    .unwrap()
}

fn insert_test_repository(conn: &Connection) -> i64 {
    conn.execute(
        "INSERT INTO repositories (name, url) VALUES (?1, ?2)",
        params!["static-test", "file:///static-test"],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn test_tuf_client_new() {
    let client = TufClient::new(1, "https://repo.example.com", None).unwrap();
    assert_eq!(client.tuf_base_url, "https://repo.example.com/tuf");

    let client2 = TufClient::new(
        1,
        "https://repo.example.com",
        Some("https://tuf.example.com"),
    )
    .unwrap();
    assert_eq!(client2.tuf_base_url, "https://tuf.example.com");
}

#[test]
fn test_tuf_client_new_strips_trailing_slash() {
    let client = TufClient::new(1, "https://repo.example.com/", None).unwrap();
    assert_eq!(client.tuf_base_url, "https://repo.example.com/tuf");
}

#[test]
fn persist_targets_rejects_missing_sha256_without_writing_empty_authority() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    let mut targets = fixture.targets.signed.clone();
    targets.targets.insert(
        "packages/missing-digest.ccs".to_string(),
        TargetDescription {
            length: 1,
            hashes: std::collections::BTreeMap::new(),
        },
    );

    let error = client.persist_targets(&conn, &targets).unwrap_err();

    assert!(error.to_string().contains("missing its required sha256"));
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tuf_targets WHERE repository_id = ?1",
            params![repo_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn static_file_repo_update_accepts_identical_timestamp_bytes_without_rollback() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);

    let first = client.update(&conn).await.unwrap();
    assert_eq!(first.timestamp_version, 1);

    let second = client.update(&conn).await.unwrap();
    assert_eq!(
        (
            second.timestamp_version,
            second.snapshot_version,
            second.targets_version
        ),
        (1, 1, 1)
    );
}

#[tokio::test]
async fn static_equal_timestamp_rejects_altered_cached_targets_hash() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.alter_stored_targets_same_version(&conn, repo_id);
    let err = client.update(&conn).await.unwrap_err();
    assert!(err.to_string().contains("Hash mismatch"));
}

#[tokio::test]
async fn static_update_rejects_equal_timestamp_version_with_different_bytes() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.write_same_version_timestamp_with_different_bytes();
    let err = client.update(&conn).await.unwrap_err();
    assert!(err.to_string().contains("metadata bytes/hash differ"));
}

#[tokio::test]
async fn generic_equal_timestamp_version_remains_rollback_even_when_hash_matches() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.generic_client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    let err = client.update(&conn).await.unwrap_err();
    assert!(matches!(
        err,
        TrustError::RollbackAttack {
            role,
            new: 1,
            stored: 1
        } if role == "timestamp"
    ));
}

#[tokio::test]
async fn static_update_rejects_greater_snapshot_missing_root_before_persistence() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.write_greater_snapshot_without_root();
    let err = client.update(&conn).await.unwrap_err();
    assert!(err.to_string().contains("root.json"));

    let stored_timestamp_version: i64 = conn
        .query_row(
            "SELECT version FROM tuf_metadata WHERE repository_id = ?1 AND role = 'timestamp'",
            params![repo_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_timestamp_version, 1);
}

#[tokio::test]
async fn static_update_expired_metadata_names_publish_refresh_remedy() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    fixture.write_expired_timestamp();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);

    let err = client.update(&conn).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("timestamp"));
    assert!(message.contains("conary publish --refresh"));
}

#[tokio::test]
async fn static_update_rechecks_cached_root_expiry_with_refresh_remedy() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);

    fixture.expire_stored_root(&conn, repo_id);
    let err = client.update(&conn).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("root"));
    assert!(message.contains("conary publish --refresh"));
}

#[tokio::test]
async fn static_equal_timestamp_rechecks_cached_snapshot_expiry_with_refresh_remedy() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.expire_stored_snapshot(&conn, repo_id);
    let err = client.update(&conn).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("snapshot"));
    assert!(message.contains("conary publish --refresh"));
}

#[tokio::test]
async fn static_greater_timestamp_rechecks_cached_targets_expiry_before_persistence() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.expire_stored_targets(&conn, repo_id);
    fixture.write_timestamp_for_stored_snapshot_version(&conn, repo_id, 2);
    let err = client.update(&conn).await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("targets"));
    assert!(message.contains("conary publish --refresh"));

    assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
}

#[tokio::test]
async fn static_update_rejects_bad_timestamp_hash_for_cached_snapshot_without_persistence() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.write_timestamp_for_cached_snapshot_with_bad_hash(2);
    let err = client.update(&conn).await.unwrap_err();
    assert!(err.to_string().contains("Hash mismatch"));
    assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
}

#[tokio::test]
async fn static_update_rejects_missing_timestamp_hash_for_cached_snapshot_without_persistence() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.write_timestamp_for_cached_snapshot_without_hash(2);
    let err = client.update(&conn).await.unwrap_err();
    assert!(err.to_string().contains("missing required sha256 hash"));
    assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
}

#[tokio::test]
async fn static_update_rejects_bad_snapshot_hash_for_cached_targets_without_persistence() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.write_snapshot_for_cached_targets_with_bad_hash();
    let err = client.update(&conn).await.unwrap_err();
    assert!(err.to_string().contains("Hash mismatch"));
    assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
    assert_eq!(stored_metadata_version(&conn, repo_id, "snapshot"), 1);
}

#[tokio::test]
async fn static_update_rejects_missing_snapshot_hash_for_cached_targets_without_persistence() {
    let (_db, conn) = create_test_db();
    let repo_id = insert_test_repository(&conn);
    let fixture = StaticMetadataFixture::new();
    let client = fixture.client(repo_id);
    fixture.bootstrap(&client, &conn);
    client.update(&conn).await.unwrap();

    fixture.write_snapshot_for_cached_targets_without_hash();
    let err = client.update(&conn).await.unwrap_err();
    assert!(err.to_string().contains("missing required sha256 hash"));
    assert_eq!(stored_metadata_version(&conn, repo_id, "timestamp"), 1);
    assert_eq!(stored_metadata_version(&conn, repo_id, "snapshot"), 1);
}
