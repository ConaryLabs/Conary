// apps/remi/src/server/catalog_refresh/tests.rs

use super::*;
use conary_core::db::models::{
    NativeSourceEcosystem, NativeSourceStream, RepositoryPolicyScope, RepositorySourcePolicy,
    RepositoryUpdateMode,
};
use conary_core::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2, ProfileRevisionV2,
    ProfileSourceMemberV2, SourceSnapshotV1, SourceStreamKindV1, SourceStreamV1,
    verify_source_catalog_bundle,
};
use conary_core::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

use crate::server::catalog_authority::{
    ProfileRevisionSelection, test_support::ActiveCatalogFixture,
};
use crate::server::catalog_capacity::CatalogScratchCoordinator;

use std::sync::{Condvar, Mutex};
use std::time::Duration;

fn repository(name: &str, identity: &str, priority: i32) -> Repository {
    let mut repository =
        Repository::new(name.to_string(), format!("https://example.test/{identity}"));
    repository.id = Some(i64::from(priority) + 100);
    repository.source_profile = Some("fedora-44".to_string());
    repository.priority = priority;
    repository.profile_member_role = Some(if identity.contains("updates") {
        ProfileSourceRole::Updates
    } else {
        ProfileSourceRole::Base
    });
    repository.profile_member_required = true;
    repository
        .set_parser_config(RepositoryParserConfig::Rpm {
            architecture: "x86_64".to_string(),
        })
        .unwrap();
    repository
        .set_trust_policy(RepositoryTrustPolicy::Rpm {
            metadata: RpmMetadataAuthority::Metalink {
                url: "https://example.test/metalink".to_string(),
            },
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

#[test]
fn member_plan_is_independent_of_input_and_display_name_order() {
    let updates = repository("aaa-display-name", "fedora-44-updates-x86_64", 110);
    let everything = repository("zzz-display-name", "fedora-44-everything-x86_64", 100);
    let forward =
        plan_profile_sources("fedora-44", vec![everything.clone(), updates.clone()]).unwrap();
    let reversed = plan_profile_sources("fedora-44", vec![updates, everything]).unwrap();

    let identities = |plans: &[ProfileSourcePlan]| {
        plans
            .iter()
            .map(|plan| {
                (
                    plan.ordinal,
                    plan.precedence,
                    plan.repository.repository_identity.clone().unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&forward), identities(&reversed));
    assert_eq!(
        identities(&forward),
        vec![
            (0, 110, "fedora-44-updates-x86_64".to_string()),
            (1, 100, "fedora-44-everything-x86_64".to_string()),
        ]
    );
}

#[test]
fn member_plan_rejects_mixed_profiles_before_fetch() {
    let mut mixed = repository("mixed", "fedora-44-updates-x86_64", 110);
    mixed.source_profile = Some("fedora-45".to_string());
    let error = plan_profile_sources("fedora-44", vec![mixed])
        .err()
        .expect("mixed profile must fail");
    assert!(error.to_string().contains("cannot plan"));
}

#[test]
fn member_plan_rejects_an_incomplete_declared_profile() {
    let everything = repository("everything", "fedora-44-everything-x86_64", 100);
    let error = plan_profile_sources("fedora-44", vec![everything])
        .err()
        .expect("missing updates must fail");
    assert!(
        error
            .to_string()
            .contains("source membership is incomplete")
    );
    assert!(error.to_string().contains("fedora-44-updates-x86_64"));
}

#[test]
fn candidate_cleanup_removes_only_the_exact_canonical_run() {
    let root = tempfile::tempdir().unwrap();
    let run_id = "00000000-0000-4000-8000-000000000001";
    let candidate = root.path().join(run_id);
    let unrelated = root.path().join("keep-me");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(candidate.join("partial"), b"candidate").unwrap();
    std::fs::create_dir(&unrelated).unwrap();

    cleanup_candidate_run(root.path(), run_id).unwrap();

    assert!(!candidate.exists());
    assert!(unrelated.exists());
}

#[test]
fn candidate_cleanup_rejects_noncanonical_or_path_like_identity() {
    let root = tempfile::tempdir().unwrap();
    for invalid in [
        "../00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-00000000000A",
        "not-a-run",
    ] {
        assert!(cleanup_candidate_run(root.path(), invalid).is_err());
    }
}

fn reusable_manifest() -> ProfileRevisionV2 {
    let members = vec![
        ProfileSourceMemberV2 {
            ordinal: 0,
            role: ProfileSourceRole::Updates,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-44-updates-x86_64".to_string(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            precedence: 110,
            required: true,
            source_snapshot_sha256: "a".repeat(64),
        },
        ProfileSourceMemberV2 {
            ordinal: 1,
            role: ProfileSourceRole::Base,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-44-everything-x86_64".to_string(),
            stream: SourceStreamV1 {
                kind: SourceStreamKindV1::Release,
                identity: "44".to_string(),
            },
            precedence: 100,
            required: true,
            source_snapshot_sha256: "b".repeat(64),
        },
    ];
    ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile: "fedora-44".to_string(),
        projection_version: PROFILE_CATALOG_PROJECTION_VERSION,
        catalog: CatalogArtifactV1 {
            sha256: "c".repeat(64),
            size: 4096,
        },
        logical_digest_sha256: "d".repeat(64),
        counts: CatalogCountsV1 {
            source_evidence: members.len() as u64,
            ..CatalogCountsV1::default()
        },
        members,
    }
}

#[test]
fn profile_reuse_requires_the_complete_member_and_projection_contract() {
    let manifest = reusable_manifest();
    assert!(profile_revision_matches_contract(
        &manifest,
        "fedora-44",
        &manifest.members
    ));

    let mut changed_member = manifest.members.clone();
    changed_member[0].source_snapshot_sha256 = "e".repeat(64);
    assert!(!profile_revision_matches_contract(
        &manifest,
        "fedora-44",
        &changed_member
    ));

    let mut changed_projection = manifest.clone();
    changed_projection.projection_version += 1;
    assert!(!profile_revision_matches_contract(
        &changed_projection,
        "fedora-44",
        &changed_projection.members
    ));
}

#[test]
fn exact_reuse_skips_profile_candidate_construction() {
    let fixture = ActiveCatalogFixture::new();
    let profile = "fedora-44";
    let revision = fixture.candidate(profile, 1, Vec::new());
    let selection = ProfileRevisionSelection {
        source_profile: profile.to_string(),
        profile_revision_sha256: revision,
    };
    let reusable = fixture
        .authority()
        .open_selected_profile(&selection)
        .expect("open exact reusable profile");
    let members = reusable.manifest().members.clone();
    let conn = fixture.connection();
    let sources = members
        .iter()
        .map(|member| {
            let manifest_json = conn
                .query_row(
                    "SELECT manifest_json FROM remi_catalog_resources
                     WHERE resource_sha256 = ?1 AND resource_kind = 'source_snapshot'",
                    [&member.source_snapshot_sha256],
                    |row| row.get::<_, String>(0),
                )
                .expect("read exact source manifest");
            let manifest: SourceSnapshotV1 =
                serde_json::from_str(&manifest_json).expect("parse exact source manifest");
            let path = fixture
                .catalog_dir()
                .join("sources")
                .join(&member.source_snapshot_sha256);
            let reader =
                verify_source_catalog_bundle(&path, &manifest).expect("reopen exact source bundle");
            VerifiedStagedSourceCatalog {
                staged: StagedSourceCatalog {
                    ordinal: member.ordinal,
                    role: member.role,
                    precedence: member.precedence,
                    required: member.required,
                    manifest,
                    path,
                    artifact: StagedSourceArtifact::DurableReuse,
                },
                reader,
            }
        })
        .collect();
    let candidate_root = tempfile::tempdir().expect("create candidate root");
    let candidate_run_dir = candidate_root.path().join("run");
    std::fs::create_dir(&candidate_run_dir).expect("create candidate run");
    let staged = stage_profile_candidate(
        StagedProfileSources {
            profile: profile.to_string(),
            members,
            sources,
            candidate_run_dir: candidate_run_dir.clone(),
            scratch_admission: Arc::new(CatalogScratchCoordinator::default()),
        },
        Some(reusable),
    )
    .expect("reuse exact profile");

    assert!(matches!(staged.artifact, StagedProfileArtifact::Reused(_)));
    assert!(!candidate_run_dir.join("profile").exists());
    let published = publish_staged_profile(staged, fixture.catalog_dir())
        .expect("retain exact registered source and profile publications");
    assert_eq!(published.sources.len(), published.manifest.members.len());
    assert!(published.sources.iter().all(|source| {
        source.path
            == fixture
                .catalog_dir()
                .join("sources")
                .join(source.manifest.manifest_sha256().unwrap())
    }));
    assert!(!candidate_run_dir.join("profile").exists());
}

#[test]
fn registered_source_selection_resolves_every_exact_candidate_member() {
    let fixture = ActiveCatalogFixture::new();
    let profile = "fedora-44";
    let revision = fixture.candidate(profile, 1, Vec::new());
    let selection = ProfileRevisionSelection {
        source_profile: profile.to_string(),
        profile_revision_sha256: revision,
    };

    let reusable = fixture
        .authority()
        .inspect_source_reuse_for_selection(&selection)
        .expect("resolve registered source candidates");
    assert!(!reusable.is_empty());
    for (ordinal, source) in reusable {
        assert_eq!(source.manifest().source_profile, profile);
        assert_eq!(
            source.bundle_path(),
            fixture
                .catalog_dir()
                .join("sources")
                .join(source.manifest().manifest_sha256().unwrap())
        );
        assert!(!source.manifest().repository_identity.is_empty());
        assert!(source.manifest().counts.source_evidence > 0);
        assert!(ordinal < 2);
    }
}

#[derive(Default)]
struct OverlapState {
    active: usize,
    released: bool,
}

struct OverlapGate {
    target: usize,
    state: Mutex<OverlapState>,
    changed: Condvar,
}

impl OverlapGate {
    fn new(target: usize) -> Self {
        Self {
            target,
            state: Mutex::new(OverlapState::default()),
            changed: Condvar::new(),
        }
    }

    fn meet(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.active += 1;
        if state.active == self.target {
            state.released = true;
            self.changed.notify_all();
        }
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(2), |state| !state.released)
            .unwrap();
        let released = state.released;
        state.active -= 1;
        anyhow::ensure!(
            released,
            "blocking source pipelines did not reach {}-way overlap",
            self.target
        );
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_pipelines_use_bounded_independent_blocking_workers() {
    let gate = Arc::new(OverlapGate::new(4));
    let counters = Arc::new(SourceWorkerCounters::default());
    let outputs = run_bounded_source_jobs_with((0..8).collect(), 4, {
        let gate = Arc::clone(&gate);
        let counters = Arc::clone(&counters);
        move |job| {
            let gate = Arc::clone(&gate);
            let counters = Arc::clone(&counters);
            async move {
                run_source_pipeline_on_blocking_worker(
                    move || {
                        gate.meet()?;
                        Ok(job)
                    },
                    counters,
                )
                .await
            }
        }
    })
    .await
    .expect("run bounded source workers");

    assert_eq!(outputs.len(), 8);
    assert_eq!(counters.peak(), 4);
}

struct ActiveJob(Arc<AtomicUsize>);

impl Drop for ActiveJob {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn source_failure_stops_queued_work_and_drains_started_jobs() {
    let launched = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let result = run_bounded_source_jobs_with((0..6).collect(), 2, {
        let launched = Arc::clone(&launched);
        let active = Arc::clone(&active);
        move |job| {
            let launched = Arc::clone(&launched);
            let active = Arc::clone(&active);
            async move {
                launched.fetch_add(1, Ordering::SeqCst);
                active.fetch_add(1, Ordering::SeqCst);
                let _active = ActiveJob(active);
                if job == 0 {
                    anyhow::bail!("injected source failure");
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
                Ok(job)
            }
        }
    })
    .await;

    assert!(result.unwrap_err().to_string().contains("injected"));
    assert_eq!(launched.load(Ordering::SeqCst), 2);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn zero_source_worker_bound_is_rejected() {
    let error =
        run_bounded_source_jobs_with(vec![0], 0, |job| async move { Ok::<_, anyhow::Error>(job) })
            .await
            .unwrap_err();
    assert!(error.to_string().contains("at least one"));
}
