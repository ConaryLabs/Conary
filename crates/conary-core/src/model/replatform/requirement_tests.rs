// conary-core/src/model/replatform/requirement_tests.rs

use super::*;
use crate::db::models::RepositoryRequirementGroup as DbRequirementGroup;
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementExpression,
};

#[test]
fn replatform_dependency_preflight_preserves_or_group_semantics() {
    let (_temp, conn) = create_test_db();
    let mut repository = Repository::new(
        "arch-core".to_string(),
        "https://example.test/arch".to_string(),
    );
    repository.default_strategy_distro = Some("arch".to_string());
    let repository_id = repository.insert(&conn).unwrap();

    let mut target = RepositoryPackage::new(
        repository_id,
        "consumer".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:consumer".to_string(),
        123,
        "https://example.test/arch/consumer.pkg.tar.zst".to_string(),
    );
    target.architecture = Some("x86_64".to_string());
    let target_id = target.insert(&conn).unwrap();

    let mut fallback = RepositoryPackage::new(
        repository_id,
        "fallback-provider".to_string(),
        "1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:fallback".to_string(),
        123,
        "https://example.test/arch/fallback-provider.pkg.tar.zst".to_string(),
    );
    fallback.architecture = Some("x86_64".to_string());
    fallback.insert(&conn).unwrap();

    let clauses = ["missing-provider", "fallback-provider"]
        .into_iter()
        .map(|name| RepositoryRequirementClause {
            name: name.to_string(),
            capability_kind: Some(RepositoryCapabilityKind::PackageName),
            version_constraint: None,
            native_text: Some(name.to_string()),
        })
        .collect::<Vec<_>>();
    let expression = RepositoryRequirementExpression::Or(
        clauses
            .iter()
            .cloned()
            .map(RepositoryRequirementExpression::Atom)
            .collect(),
    );
    let mut group = DbRequirementGroup::new(
        target_id,
        "depends".to_string(),
        "hard".to_string(),
        serde_json::to_string(&expression).unwrap(),
    );
    group.native_text = Some("missing-provider | fallback-provider".to_string());
    let group_id = group.insert(&conn).unwrap();
    for clause in clauses {
        let mut atom = RepositoryRequirement::new(
            target_id,
            group_id,
            clause.name,
            clause.version_constraint,
            "package".to_string(),
            "runtime".to_string(),
            clause.native_text,
        );
        atom.insert(&conn).unwrap();
    }

    let unresolved =
        unresolved_target_dependencies(&conn, Some("arch-core"), Some(target_id), Some("x86_64"))
            .unwrap();

    assert!(
        unresolved.is_empty(),
        "one satisfied OR branch must satisfy the exact requirement group"
    );
}
