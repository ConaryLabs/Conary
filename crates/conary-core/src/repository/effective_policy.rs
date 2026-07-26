// conary-core/src/repository/effective_policy.rs

//! Shared runtime source-policy loading.

use super::resolution_policy::{DependencyMixingPolicy, RequestScope, ResolutionPolicy};
use crate::db::models::{DistroPin, Repository, settings};
use crate::error::{Error, Result};
use rusqlite::Connection;

pub const SETTINGS_KEY_ALLOWED_DISTROS: &str = "source.allowed-distros";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSourcePolicy {
    pub resolution: ResolutionPolicy,
}

pub fn load_effective_policy(
    conn: &Connection,
    scope: RequestScope,
) -> Result<EffectiveSourcePolicy> {
    let pin = DistroPin::get_current(conn)?;
    let mixing = pin
        .as_ref()
        .map_or(DependencyMixingPolicy::Strict, |pin| pin.mixing_policy);
    let primary_profile = match &scope {
        RequestScope::DistroProfile(profile) => {
            let profile = crate::repository::supported_profiles::profile_by_public_id(profile)
                .ok_or_else(|| {
                    Error::ConfigError(format!(
                        "request scope names unsupported source profile '{profile}'"
                    ))
                })?;
            Some(profile.id().to_string())
        }
        RequestScope::Repository(name) => {
            let repository = Repository::find_by_name(conn, name)?.ok_or_else(|| {
                Error::ConfigError(format!("request scope names unknown repository '{name}'"))
            })?;
            repository
                .resolution_source_profile()?
                .map(|profile| profile.id().to_string())
        }
        RequestScope::Any => pin.as_ref().map(|pin| pin.distro.clone()),
    };

    let allowed_distros = settings::get(conn, SETTINGS_KEY_ALLOWED_DISTROS)?
        .map(|raw| serde_json::from_str::<Vec<String>>(&raw))
        .transpose()?
        .unwrap_or_default();

    let mut resolution = ResolutionPolicy::new()
        .with_scope(scope)
        .with_mixing(mixing)
        .with_allowed_distros(allowed_distros);
    resolution.set_primary_profile(primary_profile);

    Ok(EffectiveSourcePolicy { resolution })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn effective_policy_loads_allowed_distros_from_settings() {
        let (_tmp, conn) = create_test_db();
        settings::set(
            &conn,
            SETTINGS_KEY_ALLOWED_DISTROS,
            "[\"arch\",\"fedora-44\"]",
        )
        .unwrap();
        let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
        assert_eq!(
            policy.resolution.allowed_distros.as_slice(),
            ["arch".to_string(), "fedora-44".to_string()]
        );
    }

    #[test]
    fn effective_policy_carries_the_exact_persisted_profile() {
        let (_tmp, conn) = create_test_db();
        DistroPin::set(&conn, "fedora-44", DependencyMixingPolicy::Strict).unwrap();

        let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();

        assert_eq!(policy.resolution.primary_profile(), Some("fedora-44"));
    }

    #[test]
    fn repository_scope_requires_an_existing_exact_profile() {
        let (_tmp, conn) = create_test_db();

        let error = load_effective_policy(&conn, RequestScope::Repository("missing".to_string()))
            .unwrap_err();
        assert!(error.to_string().contains("unknown repository 'missing'"));

        let mut repository = Repository::new(
            "fedora".to_string(),
            "https://example.invalid/repository".to_string(),
        );
        repository.insert(&conn).unwrap();

        let error = load_effective_policy(&conn, RequestScope::Repository(repository.name.clone()))
            .unwrap_err();
        assert!(error.to_string().contains("has no exact source profile"));
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

        assert_eq!(policy.resolution.primary_profile(), None);
        assert_eq!(
            policy.resolution.request_scope,
            RequestScope::Repository("native-ccs".to_string())
        );
    }

    #[test]
    fn distro_specific_static_repository_scope_preserves_its_exact_profile() {
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

        assert_eq!(policy.resolution.primary_profile(), Some("arch"));
    }

    #[test]
    fn distro_scope_rejects_routes_and_generic_format_labels() {
        let (_tmp, conn) = create_test_db();

        for invalid in ["fedora", "rpm", "deb"] {
            let error =
                load_effective_policy(&conn, RequestScope::DistroProfile(invalid.to_string()))
                    .unwrap_err();
            assert!(
                error.to_string().contains("unsupported source profile"),
                "{invalid}: {error}"
            );
        }
    }
}
