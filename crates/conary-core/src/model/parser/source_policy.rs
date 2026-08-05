// conary-core/src/model/parser/source_policy.rs

//! Source selection policy for system models.

use serde::{Deserialize, Serialize};

use crate::repository::resolution_policy::DependencyMixingPolicy;

/// Source pin configuration for package sourcing preferences
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "SourcePinConfigSerde")]
pub struct SourcePinConfig {
    /// Exact supported public distro profile ID.
    pub distro: String,

    /// Typed dependency mixing behavior.
    pub strength: DependencyMixingPolicy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcePinConfigSerde {
    distro: String,
    strength: DependencyMixingPolicy,
}

impl TryFrom<SourcePinConfigSerde> for SourcePinConfig {
    type Error = String;

    fn try_from(raw: SourcePinConfigSerde) -> Result<Self, Self::Error> {
        if crate::repository::supported_profiles::profile_by_public_id(&raw.distro).is_none() {
            return Err(format!(
                "unsupported public distro profile '{}'",
                raw.distro
            ));
        }
        Ok(Self {
            distro: raw.distro,
            strength: raw.strength,
        })
    }
}

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
#[serde(try_from = "SystemConfigSerde")]
pub struct SystemConfig {
    /// Allowed distros for package sourcing
    #[serde(default)]
    pub allowed_distros: Vec<String>,

    /// Explicit source pin in the richer policy shape
    #[serde(default)]
    pub pin: Option<SourcePinConfig>,

    /// Convergence intent: how aggressively to migrate packages toward
    /// Conary-managed state when the preferred source set changes.
    #[serde(default)]
    pub convergence: ConvergenceIntent,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemConfigSerde {
    #[serde(default)]
    allowed_distros: Vec<String>,
    #[serde(default)]
    pin: Option<SourcePinConfig>,
    #[serde(default)]
    convergence: ConvergenceIntent,
}

impl TryFrom<SystemConfigSerde> for SystemConfig {
    type Error = String;

    fn try_from(raw: SystemConfigSerde) -> Result<Self, Self::Error> {
        let mut seen = std::collections::HashSet::with_capacity(raw.allowed_distros.len());
        for profile in &raw.allowed_distros {
            if crate::repository::supported_profiles::profile_by_public_id(profile).is_none() {
                return Err(format!(
                    "unsupported public distro profile '{profile}' in system.allowed_distros"
                ));
            }
            if !seen.insert(profile) {
                return Err(format!(
                    "duplicate public distro profile '{profile}' in system.allowed_distros"
                ));
            }
        }

        Ok(Self {
            allowed_distros: raw.allowed_distros,
            pin: raw.pin,
            convergence: raw.convergence,
        })
    }
}

impl SystemConfig {
    /// Return the explicit source pin.
    pub fn effective_pin(&self) -> Option<SourcePinConfig> {
        self.pin.clone()
    }

    /// Check whether the source policy has been explicitly configured.
    ///
    /// Returns `true` if any of the following are set to non-default values:
    /// - `pin` (explicit source pin)
    /// - `convergence` differs from the preview default (`CasBacked`)
    /// - `allowed_distros` is non-empty
    ///
    /// When this returns `false`, the system is running with default source
    /// policy and the user may benefit from a configuration hint.
    pub fn is_source_policy_configured(&self) -> bool {
        self.pin.is_some()
            || self.convergence != ConvergenceIntent::default()
            || !self.allowed_distros.is_empty()
    }
}
