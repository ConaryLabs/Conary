// src/commands/update/adopted_authority.rs

//! Adopted-package update authority and native package-manager guidance.

use super::super::install::OwnershipMode;
use conary_core::db::models::Trove;
use conary_core::packages::SystemPackageManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdoptedUpdateDecision {
    SkipNativeAuthority,
    QueueTakeover,
}

pub(super) fn adopted_update_decision(
    ownership: OwnershipMode,
    requested_ownership: Option<OwnershipMode>,
) -> AdoptedUpdateDecision {
    let explicit_takeover = matches!(requested_ownership, Some(OwnershipMode::Takeover));
    if ownership == OwnershipMode::Takeover && explicit_takeover {
        AdoptedUpdateDecision::QueueTakeover
    } else {
        AdoptedUpdateDecision::SkipNativeAuthority
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdoptedUpdateSkipReason {
    NativeAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdoptedUpdateSkip {
    pub(super) package: String,
    pub(super) manager: Option<SystemPackageManager>,
    pub(super) reason: AdoptedUpdateSkipReason,
}

pub(super) fn native_manager_for_trove(trove: &Trove) -> Option<SystemPackageManager> {
    SystemPackageManager::from_version_scheme(trove.version_scheme)
}

pub(super) fn render_adopted_skip_sample(skips: &[&AdoptedUpdateSkip]) -> String {
    let mut sample: Vec<String> = skips
        .iter()
        .take(5)
        .map(|skip| {
            skip.manager.map_or_else(
                || format!("{} (external owner)", skip.package),
                |manager| {
                    format!(
                        "{} ({})",
                        skip.package,
                        manager.update_command(&skip.package)
                    )
                },
            )
        })
        .collect();
    if skips.len() > 5 {
        sample.push(format!("... and {} more", skips.len() - 5));
    }
    sample.join(", ")
}

pub(super) fn no_update_message(
    security_only: bool,
    adopted_updates_skipped: bool,
) -> &'static str {
    match (security_only, adopted_updates_skipped) {
        (true, true) => {
            "No Conary-managed security updates available; adopted package updates remain under recorded external authority"
        }
        (false, true) => {
            "No Conary-managed updates available; adopted package updates remain under recorded external authority"
        }
        (true, false) => "No security updates available",
        (false, false) => "All packages are up to date",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, Trove, TroveType};
    use conary_core::repository::versioning::VersionScheme;

    fn adopted_trove(name: &str, version_scheme: VersionScheme) -> Trove {
        Trove::new_with_source(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
            version_scheme,
        )
    }

    #[test]
    fn adopted_updates_do_not_take_over_without_explicit_takeover_mode() {
        assert_eq!(
            adopted_update_decision(OwnershipMode::Takeover, None),
            AdoptedUpdateDecision::SkipNativeAuthority
        );
    }

    #[test]
    fn adopted_updates_take_over_only_under_explicit_takeover_mode() {
        assert_eq!(
            adopted_update_decision(OwnershipMode::Takeover, Some(OwnershipMode::Takeover)),
            AdoptedUpdateDecision::QueueTakeover
        );
        assert_eq!(
            adopted_update_decision(OwnershipMode::Takeover, None),
            AdoptedUpdateDecision::SkipNativeAuthority
        );
    }

    #[test]
    fn adopted_updates_are_not_queued_under_preserve() {
        assert_eq!(
            adopted_update_decision(OwnershipMode::Preserve, Some(OwnershipMode::Preserve)),
            AdoptedUpdateDecision::SkipNativeAuthority
        );
    }

    #[test]
    fn adopted_update_guidance_uses_recorded_version_scheme() {
        let trove = adopted_trove("curl", VersionScheme::Arch);

        assert_eq!(
            native_manager_for_trove(&trove),
            Some(conary_core::packages::SystemPackageManager::Pacman)
        );
    }

    #[test]
    fn conary_version_scheme_has_no_native_package_manager() {
        let trove = adopted_trove("curl", VersionScheme::Conary);

        assert_eq!(native_manager_for_trove(&trove), None);
    }

    #[test]
    fn adopted_update_skip_message_is_not_generic_up_to_date_text() {
        let message = no_update_message(false, true);

        assert!(!message.contains("All packages are up to date"));
        assert!(message.contains("recorded external authority"));
    }
}
