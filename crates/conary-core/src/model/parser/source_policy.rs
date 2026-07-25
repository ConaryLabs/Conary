// conary-core/src/model/parser/source_policy.rs

//! Source selection and package override policy for system models.

use crate::repository::resolution_policy::SelectionMode;
use serde::{Deserialize, Serialize};

fn default_source_profile() -> Option<String> {
    Some("balanced/latest-anywhere".to_string())
}

const DEFAULT_SOURCE_PROFILE: &str = "balanced/latest-anywhere";
const CONSERVATIVE_POLICY_PROFILE: &str = "conservative/policy-first";

pub(super) fn selection_mode_from_profile(profile: &str) -> Option<SelectionMode> {
    match profile {
        DEFAULT_SOURCE_PROFILE => Some(SelectionMode::Latest),
        CONSERVATIVE_POLICY_PROFILE => Some(SelectionMode::Policy),
        _ => None,
    }
}

pub(super) fn selection_mode_from_string(mode: &str) -> Option<SelectionMode> {
    match mode {
        "policy" => Some(SelectionMode::Policy),
        "latest" => Some(SelectionMode::Latest),
        _ => None,
    }
}

/// Source pin configuration for package sourcing preferences
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourcePinConfig {
    /// Preferred distro to pin to (e.g., "arch", "ubuntu-noble")
    pub distro: String,

    /// Pin strength / mixing behavior (e.g., "strict", "guarded", "hard")
    #[serde(default)]
    pub strength: Option<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "SystemConfigSerde")]
pub struct SystemConfig {
    /// Source selection profile (default: balanced/latest-anywhere)
    pub profile: Option<String>,

    /// Explicit ranking preference override.
    #[serde(default)]
    pub selection_mode: Option<String>,

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

    /// Whether the profile was explicitly present in the parsed model.
    #[serde(skip)]
    pub(super) profile_explicit: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemConfigSerde {
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    selection_mode: Option<String>,
    #[serde(default)]
    allowed_distros: Vec<String>,
    #[serde(default)]
    pin: Option<SourcePinConfig>,
    #[serde(default)]
    convergence: ConvergenceIntent,
}

impl From<SystemConfigSerde> for SystemConfig {
    fn from(raw: SystemConfigSerde) -> Self {
        let profile_explicit = raw.profile.is_some();
        Self {
            profile: raw.profile.or_else(default_source_profile),
            selection_mode: raw.selection_mode,
            allowed_distros: raw.allowed_distros,
            pin: raw.pin,
            convergence: raw.convergence,
            profile_explicit,
        }
    }
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            profile: default_source_profile(),
            selection_mode: None,
            allowed_distros: Vec::new(),
            pin: None,
            convergence: ConvergenceIntent::default(),
            profile_explicit: false,
        }
    }
}

impl SystemConfig {
    /// Return the effective selection mode, preferring explicit override over
    /// profile-derived defaults.
    pub fn effective_selection_mode(&self) -> Option<SelectionMode> {
        self.selection_mode
            .as_deref()
            .and_then(selection_mode_from_string)
            .or_else(|| {
                self.profile
                    .as_deref()
                    .and_then(selection_mode_from_profile)
            })
    }

    /// Return the selection-mode value that should be mirrored into runtime state.
    ///
    /// Implicit default profiles do not count as an explicit runtime override.
    pub fn runtime_selection_mode_mirror(&self) -> Option<SelectionMode> {
        if self.selection_mode.is_some() {
            self.selection_mode
                .as_deref()
                .and_then(selection_mode_from_string)
        } else if self.profile_explicit {
            self.profile
                .as_deref()
                .and_then(selection_mode_from_profile)
        } else {
            None
        }
    }

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
        let profile_is_non_default = self
            .profile
            .as_deref()
            .is_some_and(|profile| profile != DEFAULT_SOURCE_PROFILE);

        self.pin.is_some()
            || self.convergence != ConvergenceIntent::default()
            || self.selection_mode.is_some()
            || !self.allowed_distros.is_empty()
            || self.profile_explicit
            || profile_is_non_default
    }
}

/// Per-package override to source from a different distro
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageOverrideConfig {
    /// Distro to source this package from (e.g., "fedora-41", "rpmfusion-41")
    pub from: String,

    /// Override scope such as exact package or package family
    #[serde(default)]
    pub scope: Option<String>,

    /// Human-readable reason for the override
    #[serde(default)]
    pub reason: Option<String>,
}
