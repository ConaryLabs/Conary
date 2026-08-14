// conary-core/src/model/parser/source_policy.rs

//! Source selection policy for system models.

use serde::{Deserialize, Serialize};

/// Convergence intent controls how aggressively the system should migrate
/// packages toward Conary-managed state when the source policy changes.
///
/// Each level reuses existing install-source primitives:
/// - `TrackOnly` -> `AdoptedTrack` (metadata tracking, no CAS content)
/// - `CasBacked` -> `AdoptedFull` (tracked + CAS-backed content)
/// - `FullOwnership` -> `Taken` / `Repository` (Conary fully owns the package)
///
/// Non-interactive preview flows default to `CasBacked`, so package content
/// enters CAS and can participate in generation-backed runtime output.
/// `TrackOnly` remains available as an explicit low-disruption mode when
/// operators only want visibility and dependency accounting.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConvergenceIntent {
    /// Track packages for visibility and dependency bookkeeping.
    /// Maps to `InstallSource::AdoptedTrack`.
    TrackOnly,
    /// Track packages and back content with CAS storage.
    /// Maps to `InstallSource::AdoptedFull`. Required for generation-building.
    #[default]
    CasBacked,
    /// Fully take over package ownership via Remi install or takeover.
    /// Maps to `InstallSource::Taken` or `InstallSource::Repository`.
    /// Unlocks generations, rollback, verification, provenance, and storage dedup.
    FullOwnership,
}

impl ConvergenceIntent {
    /// Return the display name used in user-facing output.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::TrackOnly => "track-only",
            Self::CasBacked => "cas-backed",
            Self::FullOwnership => "full-ownership",
        }
    }

    /// Return the target install-source value that this convergence intent
    /// maps to, expressed as its database string.
    pub fn target_install_source(&self) -> &'static str {
        match self {
            Self::TrackOnly => "adopted-track",
            Self::CasBacked => "adopted-full",
            Self::FullOwnership => "taken",
        }
    }
}

/// System-level source policy configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemConfig {
    /// Convergence intent: how aggressively to migrate packages toward
    /// Conary-managed state when the preferred source set changes.
    #[serde(default)]
    pub convergence: ConvergenceIntent,
}

impl SystemConfig {
    /// Check whether the source policy has been explicitly configured.
    ///
    /// Returns `true` if any of the following are set to non-default values:
    /// - `convergence` differs from the preview default (`CasBacked`)
    ///
    /// When this returns `false`, the system is running with default source
    /// policy and the user may benefit from a configuration hint.
    pub fn is_source_policy_configured(&self) -> bool {
        self.convergence != ConvergenceIntent::default()
    }
}
