// conary-core/src/resolver/provider/tests/installed_preference.rs

use super::*;

fn with_provide(mut identity: PackageIdentity, name: &str) -> PackageIdentity {
    identity.provided_capabilities = vec![ProvidedCapability {
        kind: crate::repository::dependency_model::RepositoryCapabilityKind::File,
        name: name.to_string(),
        version: None,
        version_relation: None,
        version_scheme: VersionScheme::Rpm,
        architecture_qualifier: ProvideArchitectureQualifier::Implicit,
    }];
    identity
}

#[test]
fn installed_virtual_provider_is_favored_over_its_repository_copy() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);
    let capability = provider.intern_name("/usr/bin").unwrap();
    let installed = provider
        .add_solvable(with_provide(
            installed_identity("filesystem", "3.18-52.fc44", VersionScheme::Rpm, Some(7)),
            "/usr/bin",
        ))
        .unwrap();
    let mut repository = repo_identity("filesystem", "3.18-52.fc44", VersionScheme::Rpm, Some(8));
    repository.repository_priority = 100;
    provider
        .add_solvable(with_provide(repository, "/usr/bin"))
        .unwrap();

    let candidates = block_on(provider.get_candidates(capability)).expect("candidates");

    assert_eq!(candidates.favored, Some(installed));
}

#[test]
fn exact_explicit_root_is_not_replaced_by_differently_named_installed_provider() {
    let (_dir, conn) = setup_test_db();
    let mut provider = root_provider(&conn, &["requested"]);
    let requested = provider.intern_name("requested").unwrap();
    provider
        .add_solvable(with_provide(
            installed_identity("compat", "1", VersionScheme::Rpm, Some(7)),
            "requested",
        ))
        .unwrap();
    provider
        .add_solvable(repo_identity("requested", "1", VersionScheme::Rpm, Some(8)))
        .unwrap();

    let candidates = block_on(provider.get_candidates(requested)).expect("candidates");

    assert_eq!(candidates.favored, None);
}
