// apps/remi/src/server/admin_service/refresh.rs

//! Typed outcomes and bounded collection for multi-source refresh.

use futures::StreamExt;
use std::future::Future;

use super::ServiceError;

mod operations;
pub(crate) use operations::refresh_repositories_uncoordinated;
pub use operations::{refresh_profile_repositories, refresh_repositories, sync_repo};

/// Result of one successful repository metadata refresh.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoRefreshResult {
    pub name: String,
    pub source_profile: Option<String>,
    pub packages_synced: usize,
    pub skipped: bool,
}

/// Stable failure class for one repository refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRefreshFailureKind {
    SourceRejected,
    SourceNotFound,
    Conflict,
    StorageCapacity,
    Internal,
}

/// Failure from one source in a multi-repository refresh.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoRefreshFailure {
    pub name: String,
    pub source_profile: Option<String>,
    pub kind: RepoRefreshFailureKind,
    pub message: String,
}

impl RepoRefreshFailure {
    pub(super) fn from_service_error(
        name: String,
        source_profile: Option<String>,
        error: ServiceError,
    ) -> Self {
        Self::from_service_error_ref(name, source_profile, &error)
    }

    pub(super) fn from_service_error_ref(
        name: String,
        source_profile: Option<String>,
        error: &ServiceError,
    ) -> Self {
        let kind = match &error {
            ServiceError::BadRequest(_) => RepoRefreshFailureKind::SourceRejected,
            ServiceError::NotFound(_) => RepoRefreshFailureKind::SourceNotFound,
            ServiceError::Conflict(_) => RepoRefreshFailureKind::Conflict,
            ServiceError::StorageCapacity(_) => RepoRefreshFailureKind::StorageCapacity,
            ServiceError::Internal(_) => RepoRefreshFailureKind::Internal,
        };
        Self {
            name,
            source_profile,
            kind,
            message: error.to_string(),
        }
    }
}

/// Aggregate state of a multi-repository refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRefreshBatchState {
    Complete,
    Partial,
    Failed,
}

impl RepoRefreshBatchState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

/// Typed per-source outcomes for one multi-repository refresh.
#[derive(Debug, Default)]
pub struct RepoRefreshBatch {
    pub results: Vec<RepoRefreshResult>,
    pub failures: Vec<RepoRefreshFailure>,
}

impl RepoRefreshBatch {
    pub fn state(&self) -> RepoRefreshBatchState {
        match (self.results.is_empty(), self.failures.is_empty()) {
            (_, true) => RepoRefreshBatchState::Complete,
            (false, false) => RepoRefreshBatchState::Partial,
            (true, false) => RepoRefreshBatchState::Failed,
        }
    }

    pub fn synced_count(&self) -> usize {
        self.results.iter().filter(|result| !result.skipped).count()
    }

    pub fn skipped_count(&self) -> usize {
        self.results.iter().filter(|result| result.skipped).count()
    }

    pub(super) fn push(&mut self, outcome: RepoRefreshOutcome) {
        match outcome {
            RepoRefreshOutcome::Success(result) => self.results.push(result),
            RepoRefreshOutcome::Failure(failure) => self.failures.push(failure),
        }
    }

    pub(super) fn sort(&mut self) {
        self.results
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.failures
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
}

pub(super) enum RepoRefreshOutcome {
    Success(RepoRefreshResult),
    Failure(RepoRefreshFailure),
}

pub(super) async fn collect_refresh_outcomes<I, F>(jobs: I) -> RepoRefreshBatch
where
    I: IntoIterator<Item = F>,
    F: Future<Output = RepoRefreshOutcome>,
{
    let mut stream = futures::stream::iter(jobs).buffer_unordered(4);
    let mut batch = RepoRefreshBatch::default();
    while let Some(outcome) = stream.next().await {
        batch.push(outcome);
    }
    batch.sort();
    batch
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{
        NativeSourceEcosystem, NativeSourceStream, RepositoryPolicyScope, RepositorySourcePolicy,
        RepositoryUpdateMode,
    };
    use conary_core::repository::{
        OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
    };
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[test]
    fn catalog_scratch_refusal_has_a_stable_refresh_failure_kind() {
        let failure = RepoRefreshFailure::from_service_error(
            "fedora".to_string(),
            Some("fedora-44".to_string()),
            ServiceError::StorageCapacity(
                conary_core::repository::catalog::CatalogScratchCapacityError {
                    required_bytes: 4096,
                    available_bytes: 4095,
                    reserved_bytes: 0,
                },
            ),
        );

        assert_eq!(failure.kind, RepoRefreshFailureKind::StorageCapacity);
        assert!(failure.message.contains("requires 4096 additional bytes"));
    }

    fn native_repository(name: &str, identity: &str) -> conary_core::db::models::Repository {
        let metadata_url = format!("https://127.0.0.1:9/{identity}");
        let mut repository =
            conary_core::db::models::Repository::new(name.to_string(), metadata_url.clone());
        repository.source_profile = Some("fedora-44".to_string());
        let is_updates = identity.contains("updates");
        repository.priority = if is_updates { 110 } else { 100 };
        repository.profile_member_role = Some(if is_updates {
            conary_core::repository::supported_profiles::ProfileSourceRole::Updates
        } else {
            conary_core::repository::supported_profiles::ProfileSourceRole::Base
        });
        repository.profile_member_required = true;
        repository
            .set_parser_config(RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            })
            .unwrap();
        repository
            .set_trust_policy(RepositoryTrustPolicy::Rpm {
                metadata: RpmMetadataAuthority::Metalink { url: metadata_url },
                package_keys: vec![
                    OpenPgpTrustRoot::new(
                        "https://example.test/fedora.gpg".to_string(),
                        "A".repeat(40),
                    )
                    .unwrap(),
                ],
            })
            .unwrap();
        repository
            .set_native_source_policy(
                RepositorySourcePolicy::new(
                    "fedora-project",
                    RepositoryPolicyScope::repository(identity).unwrap(),
                    NativeSourceEcosystem::Rpm,
                    NativeSourceStream::release("44").unwrap(),
                    RepositoryUpdateMode::Follow,
                )
                .unwrap(),
                identity,
                None,
            )
            .unwrap();
        repository
    }

    fn native_debian_repository(name: &str, identity: &str) -> conary_core::db::models::Repository {
        let metadata_url = format!("https://127.0.0.1:9/{identity}");
        let mut repository =
            conary_core::db::models::Repository::new(name.to_string(), metadata_url);
        repository.source_profile = Some("ubuntu-26.04".to_string());
        repository.priority = 100;
        repository.profile_member_role =
            Some(conary_core::repository::supported_profiles::ProfileSourceRole::Base);
        repository.profile_member_required = true;
        repository
            .set_parser_config(RepositoryParserConfig::Deb {
                distribution: "resolute".to_string(),
                component: "main".to_string(),
                architecture: "amd64".to_string(),
            })
            .unwrap();
        repository
            .set_trust_policy(RepositoryTrustPolicy::Debian {
                release_keys: vec![
                    OpenPgpTrustRoot::new(
                        "https://example.test/ubuntu.gpg".to_string(),
                        "B".repeat(40),
                    )
                    .unwrap(),
                ],
            })
            .unwrap();
        repository
            .set_native_source_policy(
                RepositorySourcePolicy::new(
                    "ubuntu",
                    RepositoryPolicyScope::repository(identity).unwrap(),
                    NativeSourceEcosystem::Deb,
                    NativeSourceStream::release("resolute").unwrap(),
                    RepositoryUpdateMode::Follow,
                )
                .unwrap(),
                identity,
                None,
            )
            .unwrap();
        repository
    }

    fn install_active_profile_fixture(conn: &rusqlite::Connection) {
        conn.execute_batch(
            "INSERT INTO remi_catalog_resources (
                 resource_sha256, resource_kind, source_profile, artifact_sha256,
                 artifact_size, logical_digest_sha256, manifest_json, durable, created_at
             ) VALUES
               ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                'source_snapshot', 'fedora-44',
                'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                1, 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
                '{}', 1, 1),
               ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                'profile_revision', 'fedora-44',
                'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
                1, 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',
                '{}', 1, 1);
             INSERT INTO remi_profile_revision_members (
                 profile_revision_sha256, ordinal, source_snapshot_sha256,
                 source_identity, repository_identity, stream_kind,
                 stream_identity, role, precedence, required
             ) VALUES (
                'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                0,
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                'fedora-project', 'fedora-44-everything-x86_64', 'release', '44',
                'base', 100, 1
             );
             INSERT INTO repository_sync_runs (
                 run_id, source_profile, owner_instance_uuid, fencing_epoch,
                 candidate_profile_digest, state, started_at, heartbeat_at,
                 lease_expires_at, finished_at
             ) VALUES (
                '00000000-0000-4000-8000-000000000001', 'fedora-44',
                '00000000-0000-4000-8000-000000000002', 1,
                'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                'published', 1, 1, 1, 1
             );
             INSERT INTO repository_sync_scopes (
                 source_profile, fencing_epoch, current_run_id
             ) VALUES (
                'fedora-44', 1, '00000000-0000-4000-8000-000000000001'
             );
             INSERT INTO remi_active_profile_revisions (
                 source_profile, profile_revision_sha256, fencing_epoch,
                 activation_run_id, owner_instance_uuid, activated_at
             ) VALUES (
                'fedora-44',
                'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                1, '00000000-0000-4000-8000-000000000001',
                '00000000-0000-4000-8000-000000000002', 1
             );",
        )
        .expect("install active profile fixture");
    }

    #[test]
    fn malformed_native_member_stays_in_its_profile_refresh_lane() {
        let mut repository = native_repository("broken", "fedora-broken-x86_64");
        repository.parser_config = None;

        assert!(super::super::profile_refresh::is_native_profile_repository(
            &repository
        ));
        assert!(repository.require_parser_config().is_err());
    }

    #[test]
    fn active_profile_member_plan_detects_precedence_change() {
        use conary_core::repository::catalog::{
            ProfileSourceMemberV2, SourceStreamKindV1, SourceStreamV1,
        };

        let everything = native_repository("everything", "fedora-44-everything-x86_64");
        let updates = native_repository("updates", "fedora-44-updates-x86_64");
        let mut plans = crate::server::catalog_refresh::plan_profile_sources(
            "fedora-44",
            vec![everything, updates],
        )
        .unwrap();
        let members = plans
            .iter()
            .map(|plan| ProfileSourceMemberV2 {
                ordinal: plan.ordinal,
                role: plan.role,
                source_snapshot_sha256: "a".repeat(64),
                source_identity: "fedora-project".to_string(),
                repository_identity: plan.repository.repository_identity.clone().unwrap(),
                stream: SourceStreamV1 {
                    kind: SourceStreamKindV1::Release,
                    identity: "44".to_string(),
                },
                precedence: plan.precedence,
                required: true,
            })
            .collect::<Vec<_>>();
        assert!(super::super::profile_refresh::profile_members_match_plan(
            &members, &plans
        ));

        plans[0].precedence += 1;
        assert!(!super::super::profile_refresh::profile_members_match_plan(
            &members, &plans
        ));
    }

    #[tokio::test]
    async fn one_source_failure_does_not_discard_successful_outcomes() {
        let jobs = vec![
            futures::future::ready(RepoRefreshOutcome::Failure(RepoRefreshFailure {
                name: "fedora-44".to_string(),
                source_profile: Some("fedora-44".to_string()),
                kind: RepoRefreshFailureKind::SourceRejected,
                message: "invalid RPM metadata".to_string(),
            })),
            futures::future::ready(RepoRefreshOutcome::Success(RepoRefreshResult {
                name: "ubuntu-26.04".to_string(),
                source_profile: Some("ubuntu-26.04".to_string()),
                packages_synced: 42,
                skipped: false,
            })),
        ];

        let batch = collect_refresh_outcomes(jobs).await;
        assert_eq!(batch.state(), RepoRefreshBatchState::Partial);
        assert_eq!(batch.results.len(), 1);
        assert_eq!(batch.failures.len(), 1);
        assert_eq!(batch.synced_count(), 1);
        assert_eq!(batch.skipped_count(), 0);
    }

    #[tokio::test]
    async fn configured_source_failure_preserves_current_source_result() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let db_path = temp.path().join("conary.db");
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&chunk_dir).expect("create chunks");
        std::fs::create_dir_all(&cache_dir).expect("create cache");

        {
            let conn = rusqlite::Connection::open(&db_path).expect("open database");
            conary_core::db::schema::ensure_current(&conn).expect("prepare schema");

            let mut current = conary_core::db::models::Repository::new(
                "current-fedora".to_string(),
                "https://current.example.test".to_string(),
            );
            current.source_profile = Some("fedora-44".to_string());
            current.metadata_expire = 21_600;
            current.insert(&conn).expect("insert current source");
            conn.execute(
                "UPDATE repositories SET last_checked_at = ?1 WHERE name = ?2",
                rusqlite::params![
                    conary_core::repository::current_timestamp(),
                    "current-fedora"
                ],
            )
            .expect("mark source current");

            let mut broken = conary_core::db::models::Repository::new(
                "broken-fedora".to_string(),
                "https://broken.example.test".to_string(),
            );
            broken.source_profile = Some("fedora-44".to_string());
            broken.insert(&conn).expect("insert broken source");
        }

        let config = crate::server::ServerConfig {
            db_path: db_path.clone(),
            chunk_dir,
            cache_dir,
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("build server state"),
        ));

        let batch = super::super::refresh_repositories(&state, false)
            .await
            .expect("enumerate configured sources");
        assert_eq!(batch.state(), RepoRefreshBatchState::Partial);
        assert_eq!(batch.results.len(), 1);
        assert_eq!(batch.results[0].name, "current-fedora");
        assert!(batch.results[0].skipped);
        assert_eq!(batch.failures.len(), 1);
        assert_eq!(batch.failures[0].name, "broken-fedora");
        assert_eq!(batch.failures[0].kind, RepoRefreshFailureKind::Internal);
        let conn = conary_core::db::open_fast(&db_path).expect("reopen operational database");
        let active_universe_count = conn
            .query_row(
                "SELECT COUNT(*) FROM remi_active_universe_revision",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count active universe pointers");
        assert_eq!(active_universe_count, 0);
    }

    #[tokio::test]
    async fn failed_required_profile_member_activates_nothing_and_writes_no_package_rows() {
        const EXPIRED_RUN_ID: &str = "00000000-0000-4000-8000-000000000001";
        const EXPIRED_OWNER_ID: &str = "00000000-0000-4000-8000-000000000002";
        let temp = tempfile::tempdir().expect("create tempdir");
        let db_path = temp.path().join("metadata/conary.db");
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        let catalog_dir = temp.path().join("catalogs");
        let catalog_candidate_dir = temp.path().join("catalog-candidates");
        for directory in [
            db_path.parent().unwrap(),
            chunk_dir.as_path(),
            cache_dir.as_path(),
            catalog_dir.as_path(),
            catalog_candidate_dir.as_path(),
        ] {
            std::fs::create_dir_all(directory).expect("create fixture directory");
        }
        conary_core::db::init(&db_path).expect("initialize operational database");
        {
            let conn = conary_core::db::open_fast(&db_path).expect("open operational database");
            native_repository("everything", "fedora-44-everything-x86_64")
                .insert(&conn)
                .expect("insert first native member");
            native_repository("updates", "fedora-44-updates-x86_64")
                .insert(&conn)
                .expect("insert second native member");
            conn.execute(
                "INSERT INTO repository_sync_runs (
                     run_id, source_profile, owner_instance_uuid, fencing_epoch,
                     state, started_at, heartbeat_at, lease_expires_at
                 ) VALUES (?1, 'fedora-44', ?2, 1, 'fetching_objects', 0, 0, 0)",
                rusqlite::params![EXPIRED_RUN_ID, EXPIRED_OWNER_ID],
            )
            .expect("insert expired profile run");
            conn.execute(
                "INSERT INTO repository_sync_scopes (
                     source_profile, fencing_epoch, current_run_id
                 ) VALUES ('fedora-44', 1, ?1)",
                [EXPIRED_RUN_ID],
            )
            .expect("bind expired profile run");
        }
        let expired_candidate = catalog_candidate_dir.join(EXPIRED_RUN_ID);
        std::fs::create_dir(&expired_candidate).expect("create expired candidate");
        std::fs::write(expired_candidate.join("partial"), b"expired")
            .expect("write expired candidate evidence");

        let config = crate::server::ServerConfig {
            db_path: db_path.clone(),
            chunk_dir,
            cache_dir,
            catalog_dir,
            catalog_candidate_dir: catalog_candidate_dir.clone(),
            ..Default::default()
        };
        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(config).expect("build server state"),
        ));

        let batch = super::super::refresh_repositories(&state, true)
            .await
            .expect("collect failed profile refresh");
        assert_eq!(batch.state(), RepoRefreshBatchState::Failed);
        assert!(batch.results.is_empty());
        assert_eq!(batch.failures.len(), 2);

        let conn = conary_core::db::open_fast(&db_path).expect("reopen operational database");
        for table in [
            "remi_active_profile_revisions",
            "remi_catalog_resources",
            "repository_packages",
        ] {
            let count = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count authority rows");
            assert_eq!(count, 0, "unexpected rows in {table}");
        }
        let mut statement = conn
            .prepare("SELECT state FROM repository_sync_runs ORDER BY fencing_epoch")
            .expect("prepare profile run states");
        let states = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("read profile run states")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect profile run states");
        assert_eq!(states, vec!["abandoned", "abandoned"]);
        assert_eq!(
            std::fs::read_dir(catalog_candidate_dir)
                .expect("read candidate root")
                .count(),
            0
        );
    }

    #[test]
    fn exact_profile_retry_plan_contains_no_unrelated_work() {
        let fedora = native_repository("fedora", "fedora-44-everything-x86_64");
        let ubuntu = native_debian_repository("ubuntu", "ubuntu-resolute-main-amd64");
        let mut native_profiles = std::collections::BTreeMap::from([
            ("fedora-44".to_string(), vec![fedora]),
            ("ubuntu-26.04".to_string(), vec![ubuntu]),
        ]);
        let mut legacy_repositories = vec![conary_core::db::models::Repository::new(
            "legacy".to_string(),
            "https://legacy.invalid".to_string(),
        )];

        super::operations::restrict_to_profile(
            &mut native_profiles,
            &mut legacy_repositories,
            "fedora-44",
        )
        .unwrap();

        assert_eq!(
            native_profiles
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["fedora-44"]
        );
        assert_eq!(native_profiles["fedora-44"].len(), 1);
        assert!(legacy_repositories.is_empty());
    }

    #[tokio::test]
    async fn refresh_retires_active_profile_without_enabled_native_members() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let db_path = temp.path().join("metadata/conary.db");
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        for directory in [db_path.parent().unwrap(), &chunk_dir, &cache_dir] {
            std::fs::create_dir_all(directory).expect("create fixture directory");
        }
        conary_core::db::init(&db_path).expect("initialize operational database");
        {
            let conn = conary_core::db::open_fast(&db_path).expect("open operational database");
            install_active_profile_fixture(&conn);
        }

        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(crate::server::ServerConfig {
                db_path: db_path.clone(),
                chunk_dir,
                cache_dir,
                ..Default::default()
            })
            .expect("build server state"),
        ));
        let batch = super::super::refresh_repositories(&state, false)
            .await
            .expect("reconcile empty active profile");
        assert_eq!(batch.state(), RepoRefreshBatchState::Complete);

        let conn = conary_core::db::open_fast(&db_path).expect("reopen operational database");
        assert!(
            conary_core::db::models::RemiActiveProfileRevision::find(&conn, "fedora-44")
                .expect("query active pointer")
                .is_none()
        );
    }

    #[tokio::test]
    async fn manual_sync_retires_disabled_last_native_member() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let db_path = temp.path().join("metadata/conary.db");
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        for directory in [db_path.parent().unwrap(), &chunk_dir, &cache_dir] {
            std::fs::create_dir_all(directory).expect("create fixture directory");
        }
        conary_core::db::init(&db_path).expect("initialize operational database");
        {
            let conn = conary_core::db::open_fast(&db_path).expect("open operational database");
            let mut repository = native_repository("everything", "fedora-44-everything-x86_64");
            repository.enabled = false;
            repository.insert(&conn).expect("insert disabled member");
            install_active_profile_fixture(&conn);
        }

        let state = Arc::new(RwLock::new(
            crate::server::ServerState::new(crate::server::ServerConfig {
                db_path: db_path.clone(),
                chunk_dir,
                cache_dir,
                ..Default::default()
            })
            .expect("build server state"),
        ));
        let result = super::super::sync_repo(&state, "everything", false)
            .await
            .expect("reconcile disabled repository")
            .expect("repository exists");
        assert!(result.skipped);
        assert_eq!(result.packages_synced, 0);

        let conn = conary_core::db::open_fast(&db_path).expect("reopen operational database");
        assert!(
            conary_core::db::models::RemiActiveProfileRevision::find(&conn, "fedora-44")
                .expect("query active pointer")
                .is_none()
        );
    }
}
