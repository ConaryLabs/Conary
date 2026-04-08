// conary-core/src/repository/effective_policy.rs

//! Shared runtime source-policy loading.

use super::dependency_model::RepositoryDependencyFlavor;
use super::resolution_policy::{
    DependencyMixingPolicy, RequestScope, ResolutionPolicy, SelectionMode,
};
use crate::db::models::{DistroPin, settings};
use crate::error::{Error, Result};
use rusqlite::Connection;

pub const SETTINGS_KEY_SELECTION_MODE: &str = "source.selection-mode";
pub const SETTINGS_KEY_ALLOWED_DISTROS: &str = "source.allowed-distros";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSourcePolicy {
    pub resolution: ResolutionPolicy,
    pub primary_flavor: Option<RepositoryDependencyFlavor>,
}

pub fn load_effective_policy(
    conn: &Connection,
    scope: RequestScope,
) -> Result<EffectiveSourcePolicy> {
    let pin = DistroPin::get_current(conn)?;
    let mixing = pin
        .as_ref()
        .map(|pin| mixing_policy_from_string(&pin.mixing_policy))
        .unwrap_or(DependencyMixingPolicy::Strict);
    let primary_flavor = pin
        .as_ref()
        .and_then(|pin| distro_name_to_flavor(&pin.distro));

    let selection_mode = settings::get(conn, SETTINGS_KEY_SELECTION_MODE)?
        .as_deref()
        .map(parse_selection_mode)
        .transpose()?
        .unwrap_or(SelectionMode::Policy);

    let allowed_distros = settings::get(conn, SETTINGS_KEY_ALLOWED_DISTROS)?
        .map(|raw| serde_json::from_str::<Vec<String>>(&raw))
        .transpose()?
        .unwrap_or_default();

    Ok(EffectiveSourcePolicy {
        resolution: ResolutionPolicy::new()
            .with_scope(scope)
            .with_mixing(mixing)
            .with_selection_mode(selection_mode)
            .with_allowed_distros(allowed_distros),
        primary_flavor,
    })
}

fn parse_selection_mode(raw: &str) -> Result<SelectionMode> {
    match raw {
        "policy" => Ok(SelectionMode::Policy),
        "latest" => Ok(SelectionMode::Latest),
        other => Err(Error::ConfigError(format!(
            "invalid selection mode '{}'",
            other
        ))),
    }
}

fn mixing_policy_from_string(raw: &str) -> DependencyMixingPolicy {
    match raw {
        "guarded" => DependencyMixingPolicy::Guarded,
        "permissive" => DependencyMixingPolicy::Permissive,
        _ => DependencyMixingPolicy::Strict,
    }
}

fn distro_name_to_flavor(distro: &str) -> Option<RepositoryDependencyFlavor> {
    let distro = distro.to_lowercase();
    if distro.contains("fedora")
        || distro.contains("rhel")
        || distro.contains("centos")
        || distro.contains("suse")
    {
        Some(RepositoryDependencyFlavor::Rpm)
    } else if distro.contains("ubuntu") || distro.contains("debian") || distro.contains("mint") {
        Some(RepositoryDependencyFlavor::Deb)
    } else if distro.contains("arch") || distro.contains("manjaro") {
        Some(RepositoryDependencyFlavor::Arch)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    #[test]
    fn effective_policy_loads_default_selection_mode_when_setting_missing() {
        let (_tmp, conn) = create_test_db();
        let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
        assert_eq!(policy.resolution.selection_mode, SelectionMode::Policy);
    }

    #[test]
    fn effective_policy_loads_latest_selection_mode_from_settings() {
        let (_tmp, conn) = create_test_db();
        settings::set(&conn, SETTINGS_KEY_SELECTION_MODE, "latest").unwrap();
        let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
        assert_eq!(policy.resolution.selection_mode, SelectionMode::Latest);
    }

    #[test]
    fn effective_policy_loads_allowed_distros_from_settings() {
        let (_tmp, conn) = create_test_db();
        settings::set(&conn, SETTINGS_KEY_ALLOWED_DISTROS, "[\"arch\",\"fedora-43\"]").unwrap();
        let policy = load_effective_policy(&conn, RequestScope::Any).unwrap();
        assert_eq!(
            policy.resolution.allowed_distros.as_slice(),
            ["arch".to_string(), "fedora-43".to_string()]
        );
    }
}
