// src/commands/install/dep_mode.rs
//! Dependency handling mode for package installation

use std::fmt;

use conary_core::model::parser::ConvergenceIntent;

/// Controls how Conary handles dependencies during install and update.
///
/// - `Satisfy`: Dependencies already on the system satisfy requirements (default)
/// - `Adopt`: Auto-adopt system dependencies as AdoptedTrack
/// - `Takeover`: Download CCS from Remi, fully own all dependencies
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum DepMode {
    #[default]
    Satisfy,
    Adopt,
    Takeover,
}

impl DepMode {
    /// Derive the default dep mode from the active source policy convergence intent.
    ///
    /// When the user has not explicitly specified `--dep-mode`, this function
    /// provides a profile-driven default that aligns with their declared
    /// convergence intent:
    ///
    /// - `TrackOnly` -> `Satisfy` (minimal disruption, track-only)
    /// - `CasBacked` -> `Adopt` (auto-adopt so content enters CAS)
    /// - `FullOwnership` -> `Takeover` (download and fully own everything)
    ///
    /// If no convergence intent is set, the default remains `Satisfy`.
    pub fn from_convergence_intent(intent: &ConvergenceIntent) -> Self {
        match intent {
            ConvergenceIntent::TrackOnly => Self::Satisfy,
            ConvergenceIntent::CasBacked => Self::Adopt,
            ConvergenceIntent::FullOwnership => Self::Takeover,
        }
    }
}

impl fmt::Display for DepMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Satisfy => write!(f, "satisfy"),
            Self::Adopt => write!(f, "adopt"),
            Self::Takeover => write!(f, "takeover"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn test_dep_mode_from_str() {
        assert_eq!(
            DepMode::from_str("satisfy", false).unwrap(),
            DepMode::Satisfy
        );
        assert_eq!(DepMode::from_str("adopt", false).unwrap(), DepMode::Adopt);
        assert_eq!(
            DepMode::from_str("takeover", false).unwrap(),
            DepMode::Takeover
        );
        assert!(DepMode::from_str("invalid", false).is_err());
    }

    #[test]
    fn test_dep_mode_display() {
        assert_eq!(DepMode::Satisfy.to_string(), "satisfy");
        assert_eq!(DepMode::Adopt.to_string(), "adopt");
        assert_eq!(DepMode::Takeover.to_string(), "takeover");
    }

    #[test]
    fn test_dep_mode_default() {
        assert_eq!(DepMode::default(), DepMode::Satisfy);
    }

    #[test]
    fn test_dep_mode_from_convergence_track_only() {
        assert_eq!(
            DepMode::from_convergence_intent(&ConvergenceIntent::TrackOnly),
            DepMode::Satisfy
        );
    }

    #[test]
    fn test_dep_mode_from_convergence_cas_backed() {
        assert_eq!(
            DepMode::from_convergence_intent(&ConvergenceIntent::CasBacked),
            DepMode::Adopt
        );
    }

    #[test]
    fn test_dep_mode_from_convergence_full_ownership() {
        assert_eq!(
            DepMode::from_convergence_intent(&ConvergenceIntent::FullOwnership),
            DepMode::Takeover
        );
    }
}
