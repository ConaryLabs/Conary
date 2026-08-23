// crates/conary-core/src/repository/universe/client/tests.rs

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::canonical::{CanonicalMapEntry, CanonicalMapSnapshot};
use crate::ccs::signing::SigningKeyPair;
use crate::db::models::{Repository, RepositoryPackage};
use crate::repository::catalog::{
    CATALOG_CONTENT_SCHEMA_V1, CatalogContentV1, CatalogPackageOriginV1, CatalogPackageRecordV1,
    CatalogProvideRecordV1, CatalogScopeV1, CatalogSourceEvidenceV1, PROFILE_REVISION_SCHEMA_V2,
    ProfileRevisionV2, ProfileSourceMemberV2, SourceStreamKindV1, SourceStreamV1,
    write_catalog_candidate,
};
use crate::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
};
use crate::repository::universe::{
    REMI_UNIVERSE_SCHEMA_V2, RemiUniverseCanonicalMapObjectV2, RemiUniverseCatalogObjectV2,
    RemiUniverseProfileV2, enroll_remi_universe_root,
};
use crate::repository::versioning::VersionScheme;
use crate::trust::ceremony::create_initial_root;
use crate::trust::generate::{generate_snapshot, generate_targets, generate_timestamp};

const PROFILE: &str = "fedora-44";

struct Authority {
    targets: SigningKeyPair,
    snapshot: SigningKeyPair,
    timestamp: SigningKeyPair,
    signed_root: Signed<RootMetadata>,
}

impl Authority {
    fn new() -> Self {
        let root = SigningKeyPair::generate();
        let targets = SigningKeyPair::generate();
        let snapshot = SigningKeyPair::generate();
        let timestamp = SigningKeyPair::generate();
        let signed_root = create_initial_root(&root, &targets, &snapshot, &timestamp, 30).unwrap();
        Self {
            targets,
            snapshot,
            timestamp,
            signed_root,
        }
    }

    fn root_bytes(&self) -> Vec<u8> {
        crate::json::canonical_json(&self.signed_root).unwrap()
    }
}

struct PublishedBundle {
    responses: BTreeMap<String, Vec<u8>>,
    manifest_sha256: String,
    catalog_sha256: String,
    canonical_sha256: String,
}

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn profile_members() -> Vec<ProfileSourceMemberV2> {
    let mut declared = crate::repository::supported_profiles::profile_by_public_id(PROFILE)
        .unwrap()
        .members()
        .iter()
        .collect::<Vec<_>>();
    declared.sort_by(|left, right| right.precedence.cmp(&left.precedence));
    declared
        .into_iter()
        .enumerate()
        .map(|(ordinal, member)| ProfileSourceMemberV2 {
            ordinal: ordinal as u32,
            source_identity: "fedora-project".to_string(),
            repository_identity: member.repository_identity.clone(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            role: member.role,
            precedence: member.precedence,
            required: true,
            source_snapshot_sha256: digest('1'),
        })
        .collect()
}

fn profile_evidence() -> Vec<CatalogSourceEvidenceV1> {
    profile_members()
        .into_iter()
        .map(|member| CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: member.ordinal,
            source_identity: member.source_identity,
            repository_identity: member.repository_identity,
            source_snapshot_sha256: member.source_snapshot_sha256,
        })
        .collect()
}

fn package(version: &str) -> CatalogPackageRecordV1 {
    CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-44-updates-x86_64".to_string(),
            source_snapshot_sha256: digest('1'),
        },
        source_profile: PROFILE.to_string(),
        name: "demo".to_string(),
        version: version.to_string(),
        package_release: "1.fc44".to_string(),
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some("signed universe client fixture".to_string()),
        checksum: digest('2'),
        size: 4096,
        download_url: "https://packages.example.test/demo.rpm".to_string(),
        metadata: None,
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Rpm,
        provides: vec![CatalogProvideRecordV1 {
            capability: "demo".to_string(),
            version: Some(version.to_string()),
            version_relation: Some(ProvideVersionRelation::Equal),
            kind: "package".to_string(),
            raw: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }],
        requirement_groups: Vec::new(),
    }
}

fn publish_bundle(
    root: &Path,
    authority: &Authority,
    sequence: u64,
    metadata_version: u64,
    package_version: &str,
    generated_at: chrono::DateTime<Utc>,
    expires_at: chrono::DateTime<Utc>,
) -> PublishedBundle {
    let catalog_path = root.join(format!("catalog-{sequence}-{metadata_version}.sqlite"));
    let content = CatalogContentV1::new(
        CatalogScopeV1::Profile {
            profile: PROFILE.to_string(),
        },
        profile_evidence(),
        vec![package(package_version)],
    )
    .unwrap();
    let binding = write_catalog_candidate(&catalog_path, &content).unwrap();
    let catalog_bytes = fs::read(&catalog_path).unwrap();
    let revision = ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile: PROFILE.to_string(),
        projection_version: 1,
        members: profile_members(),
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    };
    let canonical_map = CanonicalMapSnapshot {
        schema_version: crate::canonical::CANONICAL_MAP_SCHEMA_VERSION,
        revision: 1,
        generated_at: Some("2026-08-22T12:00:00Z".to_string()),
        entries: vec![CanonicalMapEntry {
            canonical: "demo-app".to_string(),
            kind: "package".to_string(),
            category: None,
            implementations: BTreeMap::from([(PROFILE.to_string(), "demo".to_string())]),
        }],
    };
    let canonical_bytes = crate::json::canonical_json(&canonical_map).unwrap();
    let canonical_sha256 = crate::hash::sha256(&canonical_bytes);
    let manifest = RemiUniverseManifestV2 {
        schema_version: REMI_UNIVERSE_SCHEMA_V2,
        sequence,
        metadata_root_sha256: crate::hash::sha256(&authority.root_bytes()),
        generated_at,
        expires_at,
        profiles: vec![RemiUniverseProfileV2 {
            ordinal: 0,
            profile_revision_sha256: revision.manifest_sha256().unwrap(),
            catalog: RemiUniverseCatalogObjectV2 {
                schema_version: CATALOG_CONTENT_SCHEMA_V1,
                sha256: binding.artifact.sha256.clone(),
                size: binding.artifact.size,
                logical_digest_sha256: binding.logical_digest_sha256,
            },
            revision,
        }],
        canonical_map: RemiUniverseCanonicalMapObjectV2 {
            schema_version: canonical_map.schema_version,
            sha256: canonical_sha256.clone(),
            size: canonical_bytes.len() as u64,
            revision: canonical_map.revision,
            entry_count: canonical_map.entries.len() as u64,
        },
    };
    manifest.validate().unwrap();
    let manifest_sha256 = manifest.manifest_sha256().unwrap();
    let manifest_bytes = crate::json::canonical_json(&manifest).unwrap();
    let manifest_path = manifest.target_path().unwrap();
    let catalog_path = manifest.profiles[0].catalog.target_path();
    let canonical_path = manifest.canonical_map.target_path();
    let targets = generate_targets(
        &[
            (
                manifest_path.clone(),
                manifest_bytes.len() as u64,
                manifest_sha256.clone(),
            ),
            (
                catalog_path.clone(),
                catalog_bytes.len() as u64,
                binding.artifact.sha256.clone(),
            ),
            (
                canonical_path.clone(),
                canonical_bytes.len() as u64,
                canonical_sha256.clone(),
            ),
        ],
        &authority.targets,
        metadata_version,
        7,
    )
    .unwrap();
    let snapshot = generate_snapshot(
        authority.signed_root.signed.version,
        &targets,
        &authority.snapshot,
        metadata_version,
        7,
    )
    .unwrap();
    let timestamp =
        generate_timestamp(&snapshot, &authority.timestamp, metadata_version, 24).unwrap();
    let responses = BTreeMap::from([
        (
            "/v1/universe/tuf/timestamp.json".to_string(),
            serde_json::to_vec(&timestamp).unwrap(),
        ),
        (
            "/v1/universe/tuf/snapshot.json".to_string(),
            serde_json::to_vec(&snapshot).unwrap(),
        ),
        (
            "/v1/universe/tuf/targets.json".to_string(),
            serde_json::to_vec(&targets).unwrap(),
        ),
        (
            format!("/v1/universe/targets/{manifest_path}"),
            manifest_bytes,
        ),
        (
            format!("/v1/universe/targets/{catalog_path}"),
            catalog_bytes,
        ),
        (
            format!("/v1/universe/targets/{canonical_path}"),
            canonical_bytes,
        ),
    ]);
    PublishedBundle {
        responses,
        manifest_sha256,
        catalog_sha256: binding.artifact.sha256,
        canonical_sha256,
    }
}

#[derive(Default)]
struct HttpState {
    responses: BTreeMap<String, Vec<u8>>,
    hits: HashMap<String, usize>,
}

struct TestServer {
    endpoint: String,
    state: Arc<Mutex<HttpState>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(HttpState::default()));
        let server_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let state = Arc::clone(&server_state);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut chunk = [0_u8; 1024];
                    loop {
                        let Ok(read) = stream.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&chunk[..read]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let body = {
                        let mut guard = state.lock().unwrap();
                        *guard.hits.entry(path.clone()).or_default() += 1;
                        guard.responses.get(&path).cloned()
                    };
                    let (status, body) = body
                        .map(|body| ("200 OK", body))
                        .unwrap_or_else(|| ("404 Not Found", Vec::new()));
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self {
            endpoint,
            state,
            task,
        }
    }

    fn publish(&self, bundle: &PublishedBundle) {
        self.state.lock().unwrap().responses = bundle.responses.clone();
    }

    fn replace(&self, path: &str, bytes: Vec<u8>) {
        self.state
            .lock()
            .unwrap()
            .responses
            .insert(path.to_string(), bytes);
    }

    fn hits(&self, path: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .hits
            .get(path)
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn object_path(sha256: &str) -> String {
    format!("/v1/universe/targets/objects/sha256/{sha256}")
}

fn configure_client(db_path: &Path, endpoint: &str, authority: &Authority) {
    crate::db::init(db_path).unwrap();
    let conn = crate::db::open_fast(db_path).unwrap();
    let mut repository = Repository::new("remi-fedora".to_string(), endpoint.to_string());
    repository.default_strategy = Some("remi".to_string());
    repository.default_strategy_endpoint = Some(endpoint.to_string());
    repository.source_profile = Some(PROFILE.to_string());
    repository.insert(&conn).unwrap();
    enroll_remi_universe_root(&conn, endpoint, &authority.root_bytes()).unwrap();
}

fn active_identity(db_path: &Path) -> (String, u64) {
    let conn = crate::db::open_fast(db_path).unwrap();
    conn.query_row(
        "SELECT manifest_sha256, sequence FROM remi_active_client_universe WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map(|(sha256, sequence): (String, i64)| (sha256, sequence as u64))
    .unwrap()
}

#[tokio::test]
async fn sync_activates_one_fenced_universe_reuses_objects_and_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    let authority = Authority::new();
    let server = TestServer::start().await;
    configure_client(&db_path, &server.endpoint, &authority);
    let now = Utc::now();

    let first = publish_bundle(
        temp.path(),
        &authority,
        1,
        1,
        "1.0",
        now,
        now + Duration::days(7),
    );
    server.publish(&first);
    let outcome = sync_remi_universe(&db_path, &server.endpoint)
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RemiUniverseSyncOutcome::Activated {
            manifest_sha256: first.manifest_sha256.clone(),
            sequence: 1,
            package_count: 1,
            downloaded_objects: 2,
            reused_objects: 0,
        }
    );
    let current = crate::db::open(&db_path).unwrap();
    assert_eq!(
        RepositoryPackage::find_by_name(&current, "demo").unwrap()[0].version,
        "1.0"
    );
    assert_eq!(
        current
            .query_row("SELECT COUNT(*) FROM repository_packages", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(current);

    assert!(matches!(
        sync_remi_universe(&db_path, &server.endpoint)
            .await
            .unwrap(),
        RemiUniverseSyncOutcome::Unchanged { sequence: 1, .. }
    ));
    assert_eq!(server.hits(&object_path(&first.catalog_sha256)), 1);
    assert_eq!(server.hits(&object_path(&first.canonical_sha256)), 1);

    let second = publish_bundle(
        temp.path(),
        &authority,
        2,
        2,
        "2.0",
        now + Duration::minutes(1),
        now + Duration::days(7),
    );
    assert_eq!(first.canonical_sha256, second.canonical_sha256);
    server.publish(&second);
    assert_eq!(
        sync_remi_universe(&db_path, &server.endpoint)
            .await
            .unwrap(),
        RemiUniverseSyncOutcome::Activated {
            manifest_sha256: second.manifest_sha256.clone(),
            sequence: 2,
            package_count: 1,
            downloaded_objects: 1,
            reused_objects: 1,
        }
    );
    assert_eq!(server.hits(&object_path(&second.catalog_sha256)), 1);
    assert_eq!(server.hits(&object_path(&second.canonical_sha256)), 1);

    let third = publish_bundle(
        temp.path(),
        &authority,
        3,
        3,
        "3.0",
        now + Duration::minutes(2),
        now + Duration::days(7),
    );
    server.publish(&third);
    server.replace(
        &object_path(&third.catalog_sha256),
        vec![0_u8; third.responses[&object_path(&third.catalog_sha256)].len()],
    );
    let error = sync_remi_universe(&db_path, &server.endpoint)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.to_ascii_lowercase().contains("checksum mismatch"),
        "{error}"
    );
    assert_eq!(
        active_identity(&db_path),
        (second.manifest_sha256.clone(), 2)
    );

    server.publish(&third);
    assert!(matches!(
        sync_remi_universe(&db_path, &server.endpoint)
            .await
            .unwrap(),
        RemiUniverseSyncOutcome::Activated { sequence: 3, .. }
    ));

    let rollback = publish_bundle(
        temp.path(),
        &authority,
        2,
        4,
        "rollback",
        now + Duration::minutes(3),
        now + Duration::days(7),
    );
    server.publish(&rollback);
    assert!(
        sync_remi_universe(&db_path, &server.endpoint)
            .await
            .unwrap_err()
            .to_string()
            .contains("rolls back active sequence 3")
    );

    let fork = publish_bundle(
        temp.path(),
        &authority,
        3,
        5,
        "fork",
        now + Duration::minutes(4),
        now + Duration::days(7),
    );
    server.publish(&fork);
    assert!(
        sync_remi_universe(&db_path, &server.endpoint)
            .await
            .unwrap_err()
            .to_string()
            .contains("forks the active manifest")
    );

    let expired = publish_bundle(
        temp.path(),
        &authority,
        4,
        6,
        "expired",
        now - Duration::days(2),
        now - Duration::days(1),
    );
    server.publish(&expired);
    assert!(
        sync_remi_universe(&db_path, &server.endpoint)
            .await
            .unwrap_err()
            .to_string()
            .contains("expired at")
    );
    assert_eq!(active_identity(&db_path), (third.manifest_sha256, 3));
}
