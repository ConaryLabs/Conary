// src/commands/update/adopted_authority.rs

//! Adopted-package update authority and native package-manager guidance.

use super::super::install::{self, DepMode};
use conary_core::db::models::Trove;
use conary_core::packages::SystemPackageManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdoptedUpdateDecision {
    SkipNativeAuthority,
    QueueTakeover,
    BlockCritical,
}

pub(super) fn adopted_update_decision(
    trove: &Trove,
    dep_mode: DepMode,
    requested_dep_mode: Option<DepMode>,
) -> AdoptedUpdateDecision {
    let explicit_takeover = matches!(requested_dep_mode, Some(DepMode::Takeover));
    if dep_mode == DepMode::Takeover && explicit_takeover {
        if install::is_package_blocked(&trove.name) {
            AdoptedUpdateDecision::BlockCritical
        } else {
            AdoptedUpdateDecision::QueueTakeover
        }
    } else {
        AdoptedUpdateDecision::SkipNativeAuthority
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdoptedUpdateSkipReason {
    NativeAuthority,
    CriticalBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdoptedUpdateSkip {
    pub(super) package: String,
    pub(super) manager: SystemPackageManager,
    pub(super) reason: AdoptedUpdateSkipReason,
}

pub(super) fn native_manager_for_trove(
    trove: &Trove,
    fallback_manager: SystemPackageManager,
) -> SystemPackageManager {
    SystemPackageManager::from_version_scheme(trove.version_scheme.as_deref())
        .unwrap_or(fallback_manager)
}

pub(super) fn render_adopted_skip_sample(skips: &[&AdoptedUpdateSkip]) -> String {
    let mut sample: Vec<String> = skips
        .iter()
        .take(5)
        .map(|skip| {
            format!(
                "{} ({})",
                skip.package,
                skip.manager.update_command(&skip.package)
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
            "No Conary-managed security updates available; adopted package updates remain under native package-manager authority"
        }
        (false, true) => {
            "No Conary-managed updates available; adopted package updates remain under native package-manager authority"
        }
        (true, false) => "No security updates available",
        (false, false) => "All packages are up to date",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{InstallSource, Trove, TroveType};

    fn adopted_trove(name: &str) -> Trove {
        let mut trove = Trove::new_with_source(
            name.to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::AdoptedFull,
        );
        trove.version_scheme = Some("debian".to_string());
        trove
    }

    #[test]
    fn adopted_updates_do_not_take_over_without_explicit_takeover_mode() {
        let trove = adopted_trove("curl");

        assert_eq!(
            adopted_update_decision(&trove, DepMode::Takeover, None),
            AdoptedUpdateDecision::SkipNativeAuthority
        );
    }

    #[test]
    fn adopted_updates_take_over_only_under_explicit_takeover_mode() {
        let trove = adopted_trove("curl");

        assert_eq!(
            adopted_update_decision(&trove, DepMode::Takeover, Some(DepMode::Takeover)),
            AdoptedUpdateDecision::QueueTakeover
        );
        assert_eq!(
            adopted_update_decision(&trove, DepMode::Takeover, None),
            AdoptedUpdateDecision::SkipNativeAuthority
        );
    }

    #[test]
    fn critical_adopted_packages_are_blocked_even_under_takeover_mode() {
        let trove = adopted_trove("glibc");

        assert_eq!(
            adopted_update_decision(&trove, DepMode::Takeover, Some(DepMode::Takeover)),
            AdoptedUpdateDecision::BlockCritical
        );
    }

    #[test]
    fn adopted_updates_are_not_queued_under_satisfy_or_adopt() {
        let trove = adopted_trove("curl");

        for dep_mode in [DepMode::Satisfy, DepMode::Adopt] {
            assert_eq!(
                adopted_update_decision(&trove, dep_mode, Some(dep_mode)),
                AdoptedUpdateDecision::SkipNativeAuthority
            );
        }
    }

    #[test]
    fn adopted_update_guidance_uses_recorded_version_scheme_before_live_detection() {
        let mut trove = adopted_trove("curl");
        trove.version_scheme = Some("arch".to_string());

        assert_eq!(
            native_manager_for_trove(&trove, conary_core::packages::SystemPackageManager::Rpm),
            conary_core::packages::SystemPackageManager::Pacman
        );
    }

    #[test]
    fn adopted_update_skip_message_is_not_generic_up_to_date_text() {
        let message = no_update_message(false, true);

        assert!(!message.contains("All packages are up to date"));
        assert!(message.contains("native package-manager authority"));
    }
}
