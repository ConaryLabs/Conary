// src/commands/install/ownership_mode.rs
//! Recorded ownership handling for package installation and update.

use std::fmt;

use conary_core::model::parser::ConvergenceIntent;

/// Controls whether an install or update preserves an adopted package's
/// recorded external ownership or explicitly transfers it to Conary.
///
/// Dependency satisfaction itself is always decided by Conary's installed
/// provider graph and configured repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum OwnershipMode {
    #[default]
    Preserve,
    Takeover,
}

impl OwnershipMode {
    /// Derive the default ownership mode from source-policy convergence intent.
    ///
    /// When the user has not explicitly specified `--ownership`, this function
    /// provides a profile-driven default that aligns with their declared
    /// convergence intent:
    ///
    /// - `TrackOnly` / `CasBacked` -> `Preserve`
    /// - `FullOwnership` -> `Takeover`
    pub fn from_convergence_intent(intent: &ConvergenceIntent) -> Self {
        match intent {
            ConvergenceIntent::TrackOnly | ConvergenceIntent::CasBacked => Self::Preserve,
            ConvergenceIntent::FullOwnership => Self::Takeover,
        }
    }
}

impl fmt::Display for OwnershipMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preserve => write!(f, "preserve"),
            Self::Takeover => write!(f, "takeover"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn ownership_mode_from_str() {
        assert_eq!(
            OwnershipMode::from_str("preserve", false).unwrap(),
            OwnershipMode::Preserve
        );
        assert_eq!(
            OwnershipMode::from_str("takeover", false).unwrap(),
            OwnershipMode::Takeover
        );
        assert!(OwnershipMode::from_str("satisfy", false).is_err());
        assert!(OwnershipMode::from_str("adopt", false).is_err());
        assert!(OwnershipMode::from_str("invalid", false).is_err());
    }

    #[test]
    fn ownership_mode_display() {
        assert_eq!(OwnershipMode::Preserve.to_string(), "preserve");
        assert_eq!(OwnershipMode::Takeover.to_string(), "takeover");
    }

    #[test]
    fn ownership_mode_default() {
        assert_eq!(OwnershipMode::default(), OwnershipMode::Preserve);
    }

    #[test]
    fn ownership_mode_from_convergence_track_only() {
        assert_eq!(
            OwnershipMode::from_convergence_intent(&ConvergenceIntent::TrackOnly),
            OwnershipMode::Preserve
        );
    }

    #[test]
    fn ownership_mode_from_convergence_cas_backed() {
        assert_eq!(
            OwnershipMode::from_convergence_intent(&ConvergenceIntent::CasBacked),
            OwnershipMode::Preserve
        );
    }

    #[test]
    fn preview_default_convergence_preserves_recorded_ownership() {
        assert_eq!(
            OwnershipMode::from_convergence_intent(&ConvergenceIntent::default()),
            OwnershipMode::Preserve
        );
    }

    #[test]
    fn ownership_mode_from_convergence_full_ownership() {
        assert_eq!(
            OwnershipMode::from_convergence_intent(&ConvergenceIntent::FullOwnership),
            OwnershipMode::Takeover
        );
    }
}
