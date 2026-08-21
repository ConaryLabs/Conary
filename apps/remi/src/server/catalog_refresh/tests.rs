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
    let updates = repository("aaa-display-name", "fedora-updates-x86_64", 20);
    let everything = repository("zzz-display-name", "fedora-everything-x86_64", 10);
    let forward =
        plan_profile_sources("fedora-44", vec![everything.clone(), updates.clone()]).unwrap();
    let reversed = plan_profile_sources("fedora-44", vec![updates, everything]).unwrap();

    let identities = |plans: &[ProfileSourcePlan]| {
        plans
            .iter()
            .map(|plan| {
                (
                    plan.ordinal,
                    plan.priority,
                    plan.repository.repository_identity.clone().unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identities(&forward), identities(&reversed));
    assert_eq!(
        identities(&forward),
        vec![
            (0, 20, "fedora-updates-x86_64".to_string()),
            (1, 10, "fedora-everything-x86_64".to_string()),
        ]
    );
}

#[test]
fn member_plan_rejects_mixed_profiles_before_fetch() {
    let mut mixed = repository("mixed", "fedora-updates-x86_64", 20);
    mixed.source_profile = Some("fedora-45".to_string());
    let error = plan_profile_sources("fedora-44", vec![mixed])
        .err()
        .expect("mixed profile must fail");
    assert!(error.to_string().contains("cannot plan"));
}
