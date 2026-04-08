// conary-core/src/model/diff.rs

//! Diff computation between system model and current state.
//!
//! This module computes the difference between the desired state
//! (as specified in a system model) and the current state (as
//! captured from the database).

use std::collections::HashSet;

use rusqlite::Connection;

use super::parser::{SourcePinConfig, SystemModel};
use super::state::SystemState;
use super::{ResolvedModel, resolve_includes, resolve_includes_with_options};
use crate::repository::resolution_policy::SelectionMode;

/// An action to take to reach the desired state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffAction {
    /// Set or update the effective source pin
    SetSourcePin {
        distro: String,
        strength: Option<String>,
    },

    /// Clear the effective source pin
    ClearSourcePin,

    /// Set or update the persisted selection mode mirror.
    SetSelectionMode { mode: SelectionMode },

    /// Clear the persisted selection mode mirror.
    ClearSelectionMode,

    /// Set or update the persisted allowed-distros mirror.
    SetAllowedDistros { distros: Vec<String> },

    /// Clear the persisted allowed-distros mirror.
    ClearAllowedDistros,

    /// Replace an installed package with a target-distro implementation during replatforming
    ReplatformReplace {
        package: String,
        current_distro: Option<String>,
        target_distro: String,
        current_version: String,
        current_architecture: Option<String>,
        target_version: String,
        architecture: Option<String>,
        target_repository: Option<String>,
        target_repository_package_id: Option<i64>,
    },

    /// Install a new package
    Install {
        package: String,
        /// Pinned version, if any
        pin: Option<String>,
        /// Whether this is an optional package
        optional: bool,
    },

    /// Remove a package
    Remove {
        package: String,
        /// Current installed version
        current_version: String,
        /// Architectures being removed (empty if arch-agnostic)
        architectures: Vec<String>,
    },

    /// Update a package to match pin constraint
    Update {
        package: String,
        current_version: String,
        target_version: String,
    },

    /// Pin a package (mark as pinned, possibly update)
    Pin { package: String, pattern: String },

    /// Unpin a package
    Unpin { package: String },

    /// Mark a package as explicitly installed (was a dependency)
    MarkExplicit { package: String },

    /// Mark a package as dependency (was explicit)
    MarkDependency { package: String },

    /// Build and install a derived package
    BuildDerived {
        /// Name of the derived package
        name: String,
        /// Parent package name
        parent: String,
        /// Whether the parent needs to be installed first
        needs_parent: bool,
    },

    /// Rebuild a stale derived package (parent was updated)
    RebuildDerived {
        /// Name of the derived package
        name: String,
        /// Parent package name
        parent: String,
    },
}

/// Rough replatform scope estimate derived from affinity data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplatformEstimate {
    pub target_distro: String,
    pub aligned_packages: i64,
    pub packages_to_realign: i64,
    pub total_packages: i64,
}

/// Structured summary data for command/reporting layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDiffSummary {
    pub installs: usize,
    pub removes: usize,
    pub source_policy_changes: usize,
    pub other_changes: usize,
    pub warnings: usize,
    pub replatform_pending_packages: Option<i64>,
    pub planned_package_convergence: Option<usize>,
    pub visible_realignment_candidates: Option<usize>,
}

/// Shared status for source-policy driven replatforming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplatformStatus {
    PolicyOnlyPending,
    PendingWithEstimate(ReplatformEstimate),
    PackageConvergencePlanned { structural_changes: usize },
}

impl DiffAction {
    /// Get the package name this action affects
    pub fn package(&self) -> &str {
        match self {
            DiffAction::SetSourcePin { distro, .. } => distro,
            DiffAction::ClearSourcePin => "<source-policy>",
            DiffAction::SetSelectionMode { .. } => "<source-policy>",
            DiffAction::ClearSelectionMode => "<source-policy>",
            DiffAction::SetAllowedDistros { .. } => "<source-policy>",
            DiffAction::ClearAllowedDistros => "<source-policy>",
            DiffAction::ReplatformReplace { package, .. } => package,
            DiffAction::Install { package, .. } => package,
            DiffAction::Remove { package, .. } => package,
            DiffAction::Update { package, .. } => package,
            DiffAction::Pin { package, .. } => package,
            DiffAction::Unpin { package } => package,
            DiffAction::MarkExplicit { package } => package,
            DiffAction::MarkDependency { package } => package,
            DiffAction::BuildDerived { name, .. } => name,
            DiffAction::RebuildDerived { name, .. } => name,
        }
    }

    /// Check if this is a structural change (install/remove)
    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            DiffAction::ReplatformReplace { .. }
                | DiffAction::Install { .. }
                | DiffAction::Remove { .. }
        )
    }

    /// Get a human-readable description
    pub fn description(&self) -> String {
        match self {
            DiffAction::SetSourcePin { distro, strength } => match strength {
                Some(strength) => format!("Set source pin to {} ({})", distro, strength),
                None => format!("Set source pin to {}", distro),
            },
            DiffAction::ClearSourcePin => "Clear source pin".to_string(),
            DiffAction::SetSelectionMode { mode } => match mode {
                SelectionMode::Policy => "Set selection mode to policy".to_string(),
                SelectionMode::Latest => "Set selection mode to latest".to_string(),
            },
            DiffAction::ClearSelectionMode => "Clear selection mode".to_string(),
            DiffAction::SetAllowedDistros { distros } => {
                format!("Set allowed distros to {}", distros.join(", "))
            }
            DiffAction::ClearAllowedDistros => "Clear allowed distros".to_string(),
            DiffAction::ReplatformReplace {
                package,
                current_distro,
                target_distro,
                current_version,
                current_architecture,
                target_version,
                architecture,
                target_repository,
                target_repository_package_id,
            } => {
                let current = current_distro.as_deref().unwrap_or("unknown source");
                let mut desc = format!(
                    "Replatform {} ({} -> {} {} -> {})",
                    package, current, target_distro, current_version, target_version
                );
                if let Some(current_arch) = current_architecture {
                    desc.push_str(&format!(" from [{}]", current_arch));
                }
                if let Some(arch) = architecture {
                    desc.push_str(&format!(" [{}]", arch));
                }
                if let Some(repo) = target_repository {
                    desc.push_str(&format!(" via {}", repo));
                }
                if let Some(id) = target_repository_package_id {
                    desc.push_str(&format!(" [repo-pkg:{}]", id));
                }
                desc
            }
            DiffAction::Install {
                package,
                pin,
                optional,
            } => {
                let mut desc = format!("Install {}", package);
                if let Some(v) = pin {
                    desc.push_str(&format!(" (pinned to {})", v));
                }
                if *optional {
                    desc.push_str(" [optional]");
                }
                desc
            }
            DiffAction::Remove {
                package,
                current_version,
                architectures,
            } => {
                if architectures.is_empty() {
                    format!("Remove {} ({})", package, current_version)
                } else {
                    format!(
                        "Remove {} ({}) [{}]",
                        package,
                        current_version,
                        architectures.join(", ")
                    )
                }
            }
            DiffAction::Update {
                package,
                current_version,
                target_version,
            } => {
                format!(
                    "Update {} ({} -> {})",
                    package, current_version, target_version
                )
            }
            DiffAction::Pin { package, pattern } => {
                format!("Pin {} to {}", package, pattern)
            }
            DiffAction::Unpin { package } => {
                format!("Unpin {}", package)
            }
            DiffAction::MarkExplicit { package } => {
                format!("Mark {} as explicitly installed", package)
            }
            DiffAction::MarkDependency { package } => {
                format!("Mark {} as dependency", package)
            }
            DiffAction::BuildDerived {
                name,
                parent,
                needs_parent,
            } => {
                if *needs_parent {
                    format!(
                        "Build derived '{}' from '{}' (will install parent first)",
                        name, parent
                    )
                } else {
                    format!("Build derived '{}' from '{}'", name, parent)
                }
            }
            DiffAction::RebuildDerived { name, parent } => {
                format!(
                    "Rebuild derived '{}' (parent '{}' was updated)",
                    name, parent
                )
            }
        }
    }
}

/// The result of computing a diff between model and state
#[derive(Debug, Clone)]
pub struct ModelDiff {
    /// Actions to take
    pub actions: Vec<DiffAction>,

    /// Packages that would be removed (for dependency resolution)
    pub to_remove: HashSet<String>,

    /// Packages that would be installed
    pub to_install: HashSet<String>,

    /// Warnings generated during diff
    pub warnings: Vec<String>,

    /// Rough replatform scope estimate for source-policy transitions, when available.
    pub replatform_estimate: Option<ReplatformEstimate>,

    /// Visible package-level realignment candidates for the active target distro.
    pub visible_realignment_candidates: Option<super::replatform::VisibleRealignmentCandidates>,

    /// Concrete conservative same-name realignment proposals for the active target distro.
    pub visible_realignment_proposals: Option<Vec<super::replatform::VisibleRealignmentProposal>>,
}

impl ModelDiff {
    /// Create an empty diff
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
            to_remove: HashSet::new(),
            to_install: HashSet::new(),
            warnings: Vec::new(),
            replatform_estimate: None,
            visible_realignment_candidates: None,
            visible_realignment_proposals: None,
        }
    }

    /// Check if no changes are needed
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Get count of structural changes (installs + removes)
    pub fn structural_change_count(&self) -> usize {
        self.actions.iter().filter(|a| a.is_structural()).count()
    }

    /// Get all packages to install
    pub fn packages_to_install(&self) -> Vec<&str> {
        self.actions
            .iter()
            .filter_map(|a| match a {
                DiffAction::Install { package, .. } => Some(package.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get all packages to remove
    pub fn packages_to_remove(&self) -> Vec<&str> {
        self.actions
            .iter()
            .filter_map(|a| match a {
                DiffAction::Remove { package, .. } => Some(package.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Add an action
    fn add_action(&mut self, action: DiffAction) {
        match &action {
            DiffAction::Install { package, .. } => {
                self.to_install.insert(package.clone());
            }
            DiffAction::Remove { package, .. } => {
                self.to_remove.insert(package.clone());
            }
            _ => {}
        }
        self.actions.push(action);
    }

    fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    pub fn has_source_policy_changes(&self) -> bool {
        self.actions.iter().any(|action| {
            matches!(
                action,
                DiffAction::SetSourcePin { .. }
                    | DiffAction::ClearSourcePin
                    | DiffAction::SetSelectionMode { .. }
                    | DiffAction::ClearSelectionMode
                    | DiffAction::SetAllowedDistros { .. }
                    | DiffAction::ClearAllowedDistros
            )
        })
    }

    pub fn replatform_status(&self) -> Option<ReplatformStatus> {
        if !self.has_source_policy_changes() {
            return None;
        }

        let structural_changes = self.structural_change_count();
        if structural_changes > 0 {
            return Some(ReplatformStatus::PackageConvergencePlanned { structural_changes });
        }

        if let Some(estimate) = &self.replatform_estimate {
            return Some(ReplatformStatus::PendingWithEstimate(estimate.clone()));
        }

        Some(ReplatformStatus::PolicyOnlyPending)
    }

    pub fn source_policy_change_count(&self) -> usize {
        self.actions
            .iter()
            .filter(|action| {
                matches!(
                    action,
                    DiffAction::SetSourcePin { .. }
                        | DiffAction::ClearSourcePin
                        | DiffAction::SetSelectionMode { .. }
                        | DiffAction::ClearSelectionMode
                        | DiffAction::SetAllowedDistros { .. }
                        | DiffAction::ClearAllowedDistros
                )
            })
            .count()
    }

    pub fn other_change_count(&self) -> usize {
        self.actions
            .iter()
            .filter(|action| {
                !action.is_structural()
                    && !matches!(
                        action,
                        DiffAction::SetSourcePin { .. }
                            | DiffAction::ClearSourcePin
                            | DiffAction::SetSelectionMode { .. }
                            | DiffAction::ClearSelectionMode
                            | DiffAction::SetAllowedDistros { .. }
                            | DiffAction::ClearAllowedDistros
                    )
            })
            .count()
    }

    pub fn summary(&self) -> ModelDiffSummary {
        let replatform_status = self.replatform_status();
        ModelDiffSummary {
            installs: self.packages_to_install().len(),
            removes: self.packages_to_remove().len(),
            source_policy_changes: self.source_policy_change_count(),
            other_changes: self.other_change_count(),
            warnings: self.warnings.len(),
            replatform_pending_packages: match &replatform_status {
                Some(ReplatformStatus::PendingWithEstimate(estimate)) => {
                    Some(estimate.packages_to_realign)
                }
                _ => None,
            },
            planned_package_convergence: match replatform_status {
                Some(ReplatformStatus::PackageConvergencePlanned { structural_changes }) => {
                    Some(structural_changes)
                }
                _ => None,
            },
            visible_realignment_candidates: self
                .visible_realignment_candidates
                .as_ref()
                .map(|c| c.candidate_count),
        }
    }
}

impl Default for ModelDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the diff between a model and current state, resolving any includes first
///
/// This is the preferred entry point when the model may contain `[include]` directives.
/// It resolves all includes and then computes the diff against the resolved model.
pub async fn compute_diff_with_includes(
    model: &SystemModel,
    state: &SystemState,
    conn: &Connection,
) -> super::ModelResult<ModelDiff> {
    // Resolve includes if present
    let resolved = resolve_includes(model, conn).await?;
    Ok(compute_diff_from_resolved(&resolved, model, state))
}

/// Compute diff resolving includes with offline mode
///
/// When `offline` is true, only cached remote collections are used.
pub async fn compute_diff_with_includes_offline(
    model: &SystemModel,
    state: &SystemState,
    conn: &Connection,
    offline: bool,
) -> super::ModelResult<ModelDiff> {
    let resolved = resolve_includes_with_options(model, conn, offline).await?;
    Ok(compute_diff_from_resolved(&resolved, model, state))
}

/// Compute the diff from a pre-resolved model
///
/// This is used internally after resolving includes. The original model
/// is still needed for derived package definitions.
pub fn compute_diff_from_resolved(
    resolved: &ResolvedModel,
    original: &SystemModel,
    state: &SystemState,
) -> ModelDiff {
    compute_diff_inner(
        &resolved.install,
        &resolved.optionals,
        &resolved.exclude,
        &resolved.pins,
        &original.derive,
        original.system.effective_pin(),
        original.system.runtime_selection_mode_mirror(),
        original.system.allowed_distros.clone(),
        state,
    )
}

/// Compute the diff between a model and current state
///
/// Note: This does not resolve includes. Use `compute_diff_with_includes`
/// when the model may contain `[include]` directives.
pub fn compute_diff(model: &SystemModel, state: &SystemState) -> ModelDiff {
    compute_diff_inner(
        &model.config.install,
        &model.optional.packages,
        &model.config.exclude,
        &model.pin,
        &model.derive,
        model.system.effective_pin(),
        model.system.runtime_selection_mode_mirror(),
        model.system.allowed_distros.clone(),
        state,
    )
}

/// Shared implementation for computing model-vs-state diffs.
///
/// Both `compute_diff` and `compute_diff_from_resolved` delegate here
/// after extracting the relevant data from their respective model types.
#[allow(clippy::too_many_arguments)]
fn compute_diff_inner(
    install: &[String],
    optionals: &[String],
    exclude: &[String],
    pins: &std::collections::HashMap<String, String>,
    derive: &[super::parser::DerivedPackage],
    desired_source_pin: Option<SourcePinConfig>,
    desired_selection_mode: Option<SelectionMode>,
    desired_allowed_distros: Vec<String>,
    state: &SystemState,
) -> ModelDiff {
    let mut diff = ModelDiff::new();

    let model_packages: HashSet<&str> = install.iter().map(|s| s.as_str()).collect();

    let model_optional: HashSet<&str> = optionals.iter().map(|s| s.as_str()).collect();

    let model_excluded: HashSet<&str> = exclude.iter().map(|s| s.as_str()).collect();

    match (&state.source_pin, desired_source_pin.as_ref()) {
        (Some(current), Some(desired)) if current != desired => {
            diff.add_action(DiffAction::SetSourcePin {
                distro: desired.distro.clone(),
                strength: desired.strength.clone(),
            });
        }
        (None, Some(desired)) => {
            diff.add_action(DiffAction::SetSourcePin {
                distro: desired.distro.clone(),
                strength: desired.strength.clone(),
            });
        }
        (Some(_), None) => {
            diff.add_action(DiffAction::ClearSourcePin);
        }
        _ => {}
    }

    match (state.selection_mode, desired_selection_mode) {
        (Some(current), Some(desired)) if current != desired => {
            diff.add_action(DiffAction::SetSelectionMode { mode: desired });
        }
        (None, Some(desired)) => {
            diff.add_action(DiffAction::SetSelectionMode { mode: desired });
        }
        (Some(_), None) => {
            diff.add_action(DiffAction::ClearSelectionMode);
        }
        _ => {}
    }

    match (
        state.allowed_distros.as_slice(),
        desired_allowed_distros.as_slice(),
    ) {
        (current, desired) if current == desired => {}
        (_, []) => diff.add_action(DiffAction::ClearAllowedDistros),
        _ => diff.add_action(DiffAction::SetAllowedDistros {
            distros: desired_allowed_distros,
        }),
    }

    // Check what needs to be installed
    for package in &model_packages {
        if !state.is_installed(package) {
            diff.add_action(DiffAction::Install {
                package: package.to_string(),
                pin: pins.get(*package).cloned(),
                optional: false,
            });
        } else if !state.is_explicit(package) {
            // Package is installed but as a dependency - mark as explicit
            diff.add_action(DiffAction::MarkExplicit {
                package: package.to_string(),
            });
        }
    }

    // Check optional packages
    for package in &model_optional {
        if !state.is_installed(package) && !model_packages.contains(package) {
            diff.add_action(DiffAction::Install {
                package: package.to_string(),
                pin: pins.get(*package).cloned(),
                optional: true,
            });
        }
    }

    // Check what needs to be removed
    // Only remove explicitly installed packages that are not in the model
    for package in state.installed_packages() {
        if model_packages.contains(package) || model_optional.contains(package) {
            continue;
        }

        if !state.is_explicit(package) {
            continue;
        }

        if model_excluded.contains(package) {
            if let Some(pkg) = state.get_package(package) {
                let architectures: Vec<String> = state
                    .get_all_instances(package)
                    .iter()
                    .filter_map(|p| p.architecture.clone())
                    .collect();
                diff.add_action(DiffAction::Remove {
                    package: package.to_string(),
                    current_version: pkg.version.clone(),
                    architectures,
                });
            }
            continue;
        }

        // Package was explicit but is not in model - demote to dependency
        // This allows autoremove to clean up if nothing depends on it
        diff.add_action(DiffAction::MarkDependency {
            package: package.to_string(),
        });
    }

    // Check excluded packages that are installed as dependencies
    // (explicit installs were already handled in the removal loop above)
    for package in &model_excluded {
        if state.is_installed(package)
            && !state.is_explicit(package)
            && let Some(pkg) = state.get_package(package)
        {
            let architectures: Vec<String> = state
                .get_all_instances(package)
                .iter()
                .filter_map(|p| p.architecture.clone())
                .collect();
            diff.add_action(DiffAction::Remove {
                package: package.to_string(),
                current_version: pkg.version.clone(),
                architectures,
            });
        }
    }

    // Check pins
    for (package, pattern) in pins {
        if state.is_installed(package) && !state.is_pinned(package) {
            diff.add_action(DiffAction::Pin {
                package: package.clone(),
                pattern: pattern.clone(),
            });
        }
    }

    // Check for packages that should be unpinned
    for package in state.pinned.iter() {
        if !pins.contains_key(package) {
            diff.add_action(DiffAction::Unpin {
                package: package.clone(),
            });
        }
    }

    // Check derived packages
    for derived in derive {
        let derived_installed = state.is_installed(&derived.name);
        let parent_installed = state.is_installed(&derived.from);

        if !derived_installed {
            diff.add_action(DiffAction::BuildDerived {
                name: derived.name.clone(),
                parent: derived.from.clone(),
                needs_parent: !parent_installed,
            });

            if !parent_installed && !model_packages.contains(derived.from.as_str()) {
                diff.add_action(DiffAction::Install {
                    package: derived.from.clone(),
                    pin: pins.get(&derived.from).cloned(),
                    optional: false,
                });
            }
        }
    }

    if diff.has_source_policy_changes() && diff.structural_change_count() == 0 {
        diff.add_warning(
            "Source policy changed, but package realignment is still pending. Applying this model will update preferred sources now and leave blocked or unresolved replacements for later review.",
        );
    }

    diff
}

/// Options for applying a diff
#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// Dry run - don't actually make changes
    pub dry_run: bool,

    /// Skip optional packages
    pub skip_optional: bool,

    /// Force remove packages not in model (instead of demoting)
    pub strict: bool,

    /// Run autoremove after applying
    pub autoremove: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            skip_optional: false,
            strict: false,
            autoremove: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::InstalledPackage;
    use super::*;

    fn make_state_with_packages(packages: &[(&str, &str, bool)]) -> SystemState {
        let mut state = SystemState::new();
        for (name, version, explicit) in packages {
            state.add_package(
                name.to_string(),
                InstalledPackage {
                    name: name.to_string(),
                    version: version.to_string(),
                    architecture: None,
                    explicit: *explicit,
                    pinned: false,
                    label: None,
                },
            );
        }
        state
    }

    #[test]
    fn test_empty_diff() {
        let model = SystemModel::new();
        let state = SystemState::new();
        let diff = compute_diff(&model, &state);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_install_needed() {
        let mut model = SystemModel::new();
        model.config.install = vec!["nginx".to_string()];

        let state = SystemState::new();
        let diff = compute_diff(&model, &state);

        assert_eq!(diff.packages_to_install(), vec!["nginx"]);
    }

    #[test]
    fn test_already_installed() {
        let mut model = SystemModel::new();
        model.config.install = vec!["nginx".to_string()];

        let state = make_state_with_packages(&[("nginx", "1.24.0", true)]);
        let diff = compute_diff(&model, &state);

        assert!(diff.is_empty());
    }

    #[test]
    fn test_demote_to_dependency() {
        let model = SystemModel::new(); // Empty install list

        let state = make_state_with_packages(&[("nginx", "1.24.0", true)]);
        let diff = compute_diff(&model, &state);

        // Should demote to dependency, not remove
        assert!(diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::MarkDependency { package } if package == "nginx"
        )));
    }

    #[test]
    fn test_excluded_package_removed() {
        let mut model = SystemModel::new();
        model.config.exclude = vec!["sendmail".to_string()];

        let state = make_state_with_packages(&[("sendmail", "1.0.0", true)]);
        let diff = compute_diff(&model, &state);

        assert!(diff.packages_to_remove().contains(&"sendmail"));
    }

    #[test]
    fn test_mark_explicit() {
        let mut model = SystemModel::new();
        model.config.install = vec!["nginx".to_string()];

        // nginx is installed but as a dependency
        let state = make_state_with_packages(&[("nginx", "1.24.0", false)]);
        let diff = compute_diff(&model, &state);

        assert!(diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::MarkExplicit { package } if package == "nginx"
        )));
    }

    #[test]
    fn test_optional_package() {
        let mut model = SystemModel::new();
        model.optional.packages = vec!["nginx-geoip".to_string()];

        let state = SystemState::new();
        let diff = compute_diff(&model, &state);

        assert!(diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::Install { package, optional, .. }
            if package == "nginx-geoip" && *optional
        )));
    }

    #[test]
    fn test_derived_package() {
        use super::super::parser::DerivedPackage;

        let mut model = SystemModel::new();
        model.derive = vec![DerivedPackage {
            name: "nginx-custom".to_string(),
            from: "nginx".to_string(),
            version: "inherit".to_string(),
            patches: vec![],
            override_files: std::collections::HashMap::new(),
        }];

        // Parent not installed
        let state = SystemState::new();
        let diff = compute_diff(&model, &state);

        // Should have BuildDerived action with needs_parent = true
        assert!(diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::BuildDerived { name, parent, needs_parent }
            if name == "nginx-custom" && parent == "nginx" && *needs_parent
        )));

        // Should also install the parent
        assert!(diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::Install { package, .. } if package == "nginx"
        )));
    }

    #[test]
    fn test_derived_package_with_parent_installed() {
        use super::super::parser::DerivedPackage;

        let mut model = SystemModel::new();
        model.derive = vec![DerivedPackage {
            name: "nginx-custom".to_string(),
            from: "nginx".to_string(),
            version: "inherit".to_string(),
            patches: vec![],
            override_files: std::collections::HashMap::new(),
        }];

        // Parent already installed
        let state = make_state_with_packages(&[("nginx", "1.24.0", true)]);
        let diff = compute_diff(&model, &state);

        // Should have BuildDerived action with needs_parent = false
        assert!(diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::BuildDerived { name, parent, needs_parent }
            if name == "nginx-custom" && parent == "nginx" && !*needs_parent
        )));

        // Should NOT install parent again
        assert!(!diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::Install { package, .. } if package == "nginx"
        )));
    }

    #[test]
    fn test_excluded_package_no_duplicate_remove() {
        // Regression test: excluded packages that are explicit should produce
        // exactly one Remove action, not two.
        let mut model = SystemModel::new();
        model.config.exclude = vec!["sendmail".to_string()];

        let state = make_state_with_packages(&[("sendmail", "1.0.0", true)]);
        let diff = compute_diff(&model, &state);

        let remove_count = diff
            .actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    DiffAction::Remove { package, .. } if package == "sendmail"
                )
            })
            .count();
        assert_eq!(
            remove_count, 1,
            "Expected exactly one Remove action for excluded package"
        );
    }

    #[test]
    fn test_excluded_dependency_package_removed() {
        // An excluded package that is installed as a dependency (non-explicit)
        // should still be removed via the excluded-packages loop.
        let mut model = SystemModel::new();
        model.config.exclude = vec!["sendmail".to_string()];

        // sendmail installed as a dependency (explicit=false)
        let state = make_state_with_packages(&[("sendmail", "1.0.0", false)]);
        let diff = compute_diff(&model, &state);

        assert!(diff.packages_to_remove().contains(&"sendmail"));
        let remove_count = diff
            .actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    DiffAction::Remove { package, .. } if package == "sendmail"
                )
            })
            .count();
        assert_eq!(
            remove_count, 1,
            "Expected exactly one Remove action for excluded dependency"
        );
    }

    #[test]
    fn test_source_pin_change_is_diff_action() {
        let mut model = SystemModel::new();
        model.system.pin = Some(super::super::parser::SourcePinConfig {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let mut state = SystemState::new();
        state.source_pin = Some(super::super::parser::SourcePinConfig {
            distro: "fedora-43".to_string(),
            strength: Some("guarded".to_string()),
        });

        let diff = compute_diff(&model, &state);

        assert!(diff.actions.iter().any(|a| matches!(
            a,
            DiffAction::SetSourcePin { distro, strength }
            if distro == "arch" && strength.as_deref() == Some("strict")
        )));
    }

    #[test]
    fn test_source_pin_removal_is_diff_action() {
        let model = SystemModel::new();
        let mut state = SystemState::new();
        state.source_pin = Some(super::super::parser::SourcePinConfig {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let diff = compute_diff(&model, &state);

        assert!(
            diff.actions
                .iter()
                .any(|a| matches!(a, DiffAction::ClearSourcePin))
        );
    }

    #[test]
    fn test_source_pin_only_transition_warns_about_pending_convergence() {
        let mut model = SystemModel::new();
        model.system.pin = Some(super::super::parser::SourcePinConfig {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });

        let state = SystemState::new();
        let diff = compute_diff(&model, &state);

        assert!(diff.warnings.iter().any(|warning| {
            warning.contains("package realignment is still pending")
        }));
    }

    #[test]
    fn test_source_pin_with_package_changes_does_not_emit_pending_convergence_warning() {
        let mut model = SystemModel::new();
        model.system.pin = Some(super::super::parser::SourcePinConfig {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        model.config.install = vec!["kernel".to_string()];

        let state = SystemState::new();
        let diff = compute_diff(&model, &state);

        assert!(!diff.warnings.iter().any(|warning| {
            warning.contains("package realignment is still pending")
        }));
    }

    #[test]
    fn source_policy_diff_emits_selection_mode_change() {
        let mut model = SystemModel::new();
        model.system.selection_mode = Some("latest".to_string());

        let state = SystemState::new();
        let diff = compute_diff(&model, &state);

        assert!(
            diff.actions
                .iter()
                .any(|action| action.description().contains("selection mode"))
        );
    }

    #[test]
    fn source_policy_diff_emits_allowed_distros_change() {
        let mut model = SystemModel::new();
        model.system.allowed_distros = vec!["arch".to_string()];

        let state = SystemState::new();
        let diff = compute_diff(&model, &state);

        assert!(
            diff.actions
                .iter()
                .any(|action| action.description().contains("allowed distros"))
        );
    }

    #[test]
    fn test_has_source_policy_changes_detects_policy_actions() {
        let mut diff = ModelDiff::new();
        assert!(!diff.has_source_policy_changes());

        diff.add_action(DiffAction::ClearSourcePin);

        assert!(diff.has_source_policy_changes());
    }

    #[test]
    fn test_replatform_status_policy_only_pending_without_estimate() {
        let mut diff = ModelDiff::new();
        diff.add_action(DiffAction::ClearSourcePin);

        assert_eq!(
            diff.replatform_status(),
            Some(ReplatformStatus::PolicyOnlyPending)
        );
    }

    #[test]
    fn test_replatform_status_pending_with_estimate() {
        let mut diff = ModelDiff::new();
        diff.add_action(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.replatform_estimate = Some(ReplatformEstimate {
            target_distro: "arch".to_string(),
            aligned_packages: 10,
            packages_to_realign: 30,
            total_packages: 40,
        });

        assert_eq!(
            diff.replatform_status(),
            Some(ReplatformStatus::PendingWithEstimate(ReplatformEstimate {
                target_distro: "arch".to_string(),
                aligned_packages: 10,
                packages_to_realign: 30,
                total_packages: 40,
            }))
        );
    }

    #[test]
    fn test_replatform_status_package_convergence_planned() {
        let mut diff = ModelDiff::new();
        diff.add_action(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.add_action(DiffAction::Install {
            package: "kernel".to_string(),
            pin: None,
            optional: false,
        });

        assert_eq!(
            diff.replatform_status(),
            Some(ReplatformStatus::PackageConvergencePlanned {
                structural_changes: 1
            })
        );
    }

    #[test]
    fn test_summary_reports_replatform_pending_packages() {
        let mut diff = ModelDiff::new();
        diff.add_action(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.replatform_estimate = Some(ReplatformEstimate {
            target_distro: "arch".to_string(),
            aligned_packages: 10,
            packages_to_realign: 30,
            total_packages: 40,
        });

        let summary = diff.summary();

        assert_eq!(summary.source_policy_changes, 1);
        assert_eq!(summary.replatform_pending_packages, Some(30));
        assert_eq!(summary.planned_package_convergence, None);
        assert_eq!(summary.visible_realignment_candidates, None);
    }

    #[test]
    fn test_summary_reports_planned_package_convergence() {
        let mut diff = ModelDiff::new();
        diff.add_action(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.add_action(DiffAction::Install {
            package: "kernel".to_string(),
            pin: None,
            optional: false,
        });

        let summary = diff.summary();

        assert_eq!(summary.installs, 1);
        assert_eq!(summary.source_policy_changes, 1);
        assert_eq!(summary.planned_package_convergence, Some(1));
        assert_eq!(summary.replatform_pending_packages, None);
    }

    #[test]
    fn test_summary_reports_visible_realignment_candidates() {
        let mut diff = ModelDiff::new();
        diff.visible_realignment_candidates = Some(crate::model::VisibleRealignmentCandidates {
            target_distro: "arch".to_string(),
            candidate_count: 7,
        });

        let summary = diff.summary();

        assert_eq!(summary.visible_realignment_candidates, Some(7));
    }

    #[test]
    fn test_replatform_replace_is_structural_and_descriptive() {
        let action = DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(42),
        };

        assert!(action.is_structural());
        assert_eq!(action.package(), "vim");

        let description = action.description();
        assert!(description.contains("Replatform vim"));
        assert!(description.contains("fedora-43"));
        assert!(description.contains("arch"));
        assert!(description.contains("9.0.1 -> 9.1.0"));
        assert!(description.contains("via arch-core"));
    }

    #[test]
    fn test_replatform_replace_counts_as_planned_convergence() {
        let mut diff = ModelDiff::new();
        diff.actions.push(DiffAction::SetSourcePin {
            distro: "arch".to_string(),
            strength: Some("strict".to_string()),
        });
        diff.add_action(DiffAction::ReplatformReplace {
            package: "vim".to_string(),
            current_distro: Some("fedora-43".to_string()),
            target_distro: "arch".to_string(),
            current_version: "9.0.1".to_string(),
            current_architecture: Some("x86_64".to_string()),
            target_version: "9.1.0".to_string(),
            architecture: Some("x86_64".to_string()),
            target_repository: Some("arch-core".to_string()),
            target_repository_package_id: Some(42),
        });

        assert_eq!(
            diff.replatform_status(),
            Some(ReplatformStatus::PackageConvergencePlanned {
                structural_changes: 1
            })
        );
    }
}
