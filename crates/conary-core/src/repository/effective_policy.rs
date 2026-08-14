// conary-core/src/repository/effective_policy.rs

//! Shared runtime source-policy loading.

use super::resolution_policy::{DependencyMixingPolicy, RequestScope, ResolutionPolicy};
use crate::db::models::Repository;
use crate::error::{Error, Result};
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSourcePolicy {
    pub resolution: ResolutionPolicy,
}

pub fn load_effective_policy(
    conn: &Connection,
    scope: RequestScope,
) -> Result<EffectiveSourcePolicy> {
    let primary_source_identity = match &scope {
        RequestScope::SourceIdentity(source_identity) => {
            super::resolution_policy::validate_source_identity(
                source_identity,
                "request source identity",
            )
            .map_err(Error::ConfigError)?;
            Some(source_identity.clone())
        }
        RequestScope::Repository(name) => {
            let repository = Repository::find_by_name(conn, name)?.ok_or_else(|| {
                Error::ConfigError(format!("request scope names unknown repository '{name}'"))
            })?;
            repository.resolution_source_identity()?.map(str::to_string)
        }
        RequestScope::Any => None,
    };

    let mut resolution = ResolutionPolicy::new()
        .with_scope(scope)
        .with_mixing(DependencyMixingPolicy::Strict);
    resolution.set_primary_source_identity(primary_source_identity);

    Ok(EffectiveSourcePolicy { resolution })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn unscoped_policy_has_no_ambient_source_authority() {
        let (_tmp, conn) = create_test_db();
        let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
        assert_eq!(policy.resolution.primary_source_identity(), None);
    }

    #[test]
    fn repository_scope_requires_an_existing_repository() {
        let (_tmp, conn) = create_test_db();

        let error = load_effective_policy(&conn, RequestScope::Repository("missing".to_string()))
            .unwrap_err();
        assert!(error.to_string().contains("unknown repository 'missing'"));
    }

    #[test]
    fn static_repository_scope_is_exact_without_a_distro_profile() {
        let (_tmp, conn) = create_test_db();
        let mut repository = Repository::new(
            "native-ccs".to_string(),
            "https://example.invalid/conary".to_string(),
        );
        repository.default_strategy = Some("static".to_string());
        repository.insert(&conn).unwrap();

        let policy =
            load_effective_policy(&conn, RequestScope::Repository(repository.name)).unwrap();

        assert_eq!(policy.resolution.primary_source_identity(), None);
        assert_eq!(
            policy.resolution.request_scope,
            RequestScope::Repository("native-ccs".to_string())
        );
    }

    #[test]
    fn named_static_feed_projects_its_exact_source_identity() {
        let (_tmp, conn) = create_test_db();
        let mut repository = Repository::new(
            "arch-ccs".to_string(),
            "https://example.invalid/arch".to_string(),
        );
        repository.default_strategy = Some("static".to_string());
        repository.source_profile = Some("arch".to_string());
        repository.insert(&conn).unwrap();

        let policy =
            load_effective_policy(&conn, RequestScope::Repository(repository.name)).unwrap();

        assert_eq!(policy.resolution.primary_source_identity(), Some("arch"));
    }

    #[test]
    fn arbitrary_exact_source_identity_does_not_consult_a_catalog() {
        let (_tmp, conn) = create_test_db();
        let policy = load_effective_policy(
            &conn,
            RequestScope::SourceIdentity("cachyos:rolling:x86_64".to_string()),
        )
        .unwrap();
        assert_eq!(
            policy.resolution.primary_source_identity(),
            Some("cachyos:rolling:x86_64")
        );
    }
}
