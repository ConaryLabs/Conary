// src/model/diff.rs

//! Diff computation between system model and current state.
//!
//! This module computes the difference between the desired state
//! (as specified in a system model) and the current state (as
//! captured from the database).

use std::collections::HashSet;

use super::parser::SystemModel;
use super::state::SystemState;

/// An action to take to reach the desired state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffAction {
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
    },

    /// Update a package to match pin constraint
    Update {
        package: String,
        current_version: String,
        target_version: String,
    },

    /// Pin a package (mark as pinned, possibly update)
    Pin {
        package: String,
        pattern: String,
    },

    /// Unpin a package
    Unpin {
        package: String,
    },

    /// Mark a package as explicitly installed (was a dependency)
    MarkExplicit {
        package: String,
    },

    /// Mark a package as dependency (was explicit)
    MarkDependency {
        package: String,
    },

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

impl DiffAction {
    /// Get the package name this action affects
    pub fn package(&self) -> &str {
        match self {
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
        matches!(self, DiffAction::Install { .. } | DiffAction::Remove { .. })
    }

    /// Get a human-readable description
    pub fn description(&self) -> String {
        match self {
            DiffAction::Install { package, pin, optional } => {
                let mut desc = format!("Install {}", package);
                if let Some(v) = pin {
                    desc.push_str(&format!(" (pinned to {})", v));
                }
                if *optional {
                    desc.push_str(" [optional]");
                }
                desc
            }
            DiffAction::Remove { package, current_version } => {
                format!("Remove {} ({})", package, current_version)
            }
            DiffAction::Update { package, current_version, target_version } => {
                format!("Update {} ({} -> {})", package, current_version, target_version)
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
            DiffAction::BuildDerived { name, parent, needs_parent } => {
                if *needs_parent {
                    format!("Build derived '{}' from '{}' (will install parent first)", name, parent)
                } else {
                    format!("Build derived '{}' from '{}'", name, parent)
                }
            }
            DiffAction::RebuildDerived { name, parent } => {
                format!("Rebuild derived '{}' (parent '{}' was updated)", name, parent)
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
}

impl ModelDiff {
    /// Create an empty diff
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
            to_remove: HashSet::new(),
            to_install: HashSet::new(),
            warnings: Vec::new(),
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

    /// Add a warning (for future use in model validation)
    #[allow(dead_code)]
    fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

impl Default for ModelDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the diff between a model and current state
pub fn compute_diff(model: &SystemModel, state: &SystemState) -> ModelDiff {
    let mut diff = ModelDiff::new();

    // Collect all packages from model
    let model_packages: HashSet<&str> = model
        .config
        .install
        .iter()
        .map(|s| s.as_str())
        .collect();

    let model_optional: HashSet<&str> = model
        .optional
        .packages
        .iter()
        .map(|s| s.as_str())
        .collect();

    let model_excluded: HashSet<&str> = model
        .config
        .exclude
        .iter()
        .map(|s| s.as_str())
        .collect();

    // Check what needs to be installed
    for package in &model_packages {
        if !state.is_installed(package) {
            diff.add_action(DiffAction::Install {
                package: package.to_string(),
                pin: model.get_pin(package).map(|s| s.to_string()),
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
                pin: model.get_pin(package).map(|s| s.to_string()),
                optional: true,
            });
        }
    }

    // Check what needs to be removed
    // Only remove explicitly installed packages that are not in the model
    // Dependencies will be handled by autoremove
    for package in state.installed_packages() {
        // Skip if package is in the model
        if model_packages.contains(package) || model_optional.contains(package) {
            continue;
        }

        // Skip if it's a dependency (not explicit)
        if !state.is_explicit(package) {
            continue;
        }

        // If explicitly excluded, definitely remove
        if model_excluded.contains(package) {
            if let Some(pkg) = state.installed.get(package) {
                diff.add_action(DiffAction::Remove {
                    package: package.to_string(),
                    current_version: pkg.version.clone(),
                });
            }
            continue;
        }

        // Package was explicit but is not in model - remove or demote
        // For safety, we demote to dependency rather than remove
        // This allows autoremove to clean up if nothing depends on it
        diff.add_action(DiffAction::MarkDependency {
            package: package.to_string(),
        });
    }

    // Check excluded packages that are installed
    for package in &model_excluded {
        if state.is_installed(package)
            && let Some(pkg) = state.installed.get(*package)
        {
            diff.add_action(DiffAction::Remove {
                package: package.to_string(),
                current_version: pkg.version.clone(),
            });
        }
    }

    // Check pins
    for (package, pattern) in &model.pin {
        if state.is_installed(package) && !state.is_pinned(package) {
            diff.add_action(DiffAction::Pin {
                package: package.clone(),
                pattern: pattern.clone(),
            });
            // TODO: Check if current version matches pin pattern
            // and add Update action if needed
        }
    }

    // Check for packages that should be unpinned
    for package in state.pinned.iter() {
        if !model.pin.contains_key(package) {
            diff.add_action(DiffAction::Unpin {
                package: package.clone(),
            });
        }
    }

    // Check derived packages from model
    for derived in &model.derive {
        // Check if the derived package is already installed
        let derived_installed = state.is_installed(&derived.name);
        let parent_installed = state.is_installed(&derived.from);

        if !derived_installed {
            // Need to build and install the derived package
            diff.add_action(DiffAction::BuildDerived {
                name: derived.name.clone(),
                parent: derived.from.clone(),
                needs_parent: !parent_installed,
            });

            // If parent not installed, we also need to install it
            if !parent_installed && !model_packages.contains(derived.from.as_str()) {
                diff.add_action(DiffAction::Install {
                    package: derived.from.clone(),
                    pin: model.get_pin(&derived.from).map(|s| s.to_string()),
                    optional: false,
                });
            }
        }
        // Note: Stale derived packages (parent updated) are detected elsewhere
        // by checking DerivedPackage.status in the database
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
    use super::*;
    use super::super::state::InstalledPackage;

    fn make_state_with_packages(packages: &[(&str, &str, bool)]) -> SystemState {
        let mut state = SystemState::new();
        for (name, version, explicit) in packages {
            state.installed.insert(
                name.to_string(),
                InstalledPackage {
                    name: name.to_string(),
                    version: version.to_string(),
                    architecture: None,
                    explicit: *explicit,
                    label: None,
                },
            );
            if *explicit {
                state.explicit.insert(name.to_string());
            }
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
}
