// apps/remi/src/server/catalog_refresh/tests.rs

use super::*;
use conary_core::db::models::{
    NativeSourceEcosystem, NativeSourceStream, RepositoryPolicyScope, RepositorySourcePolicy,
    RepositoryUpdateMode,
};
use conary_core::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

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
