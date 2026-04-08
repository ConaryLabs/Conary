// conary-core/src/model/state.rs

//! System state capture and representation.
//!
//! This module provides functionality to capture the current state of
//! installed packages from the Conary database.

use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use crate::db::models::{DistroPin, settings};
use crate::model::parser::SourcePinConfig;
use crate::repository::resolution_policy::SelectionMode;
use crate::repository::{SETTINGS_KEY_ALLOWED_DISTROS, SETTINGS_KEY_SELECTION_MODE};

use super::{ModelError, ModelResult};
/// Represents the current state of the system
#[derive(Debug, Clone)]
pub struct SystemState {
    /// Currently installed packages (name -> list of installed instances).
    /// A Vec is used because multilib systems can have the same package
    /// installed for multiple architectures (e.g. glibc.x86_64 + glibc.i686).
    pub installed: HashMap<String, Vec<InstalledPackage>>,

    /// Explicitly installed packages (not just dependencies)
    pub explicit: HashSet<String>,

    /// Pinned packages
    pub pinned: HashSet<String>,

    /// Effective source pin mirrored from runtime compatibility state.
    pub source_pin: Option<SourcePinConfig>,

    /// Persisted selection mode mirrored from runtime compatibility state.
    pub selection_mode: Option<SelectionMode>,

    /// Persisted distro allowlist mirrored from runtime compatibility state.
    pub allowed_distros: Vec<String>,
}

/// Information about an installed package
#[derive(Debug, Clone)]
pub struct InstalledPackage {
    /// Package name
    pub name: String,

    /// Installed version
    pub version: String,

    /// Architecture
    pub architecture: Option<String>,

    /// Whether this was explicitly installed
    pub explicit: bool,

    /// Whether this instance is version-pinned
    pub pinned: bool,

    /// Source label/repository
    pub label: Option<String>,
}

impl SystemState {
    /// Create an empty system state
    pub fn new() -> Self {
        Self {
            installed: HashMap::new(),
            explicit: HashSet::new(),
            pinned: HashSet::new(),
            source_pin: None,
            selection_mode: None,
            allowed_distros: Vec::new(),
        }
    }

    /// Check if a package is installed (any architecture)
    pub fn is_installed(&self, package: &str) -> bool {
        self.installed.get(package).is_some_and(|v| !v.is_empty())
    }

    /// Get the primary installed instance of a package.
    /// On multilib systems, returns the first (typically native-arch) entry.
    pub fn get_package(&self, package: &str) -> Option<&InstalledPackage> {
        self.installed.get(package).and_then(|v| v.first())
    }

    /// Get installed version of a package (primary instance)
    pub fn get_version(&self, package: &str) -> Option<&str> {
        self.get_package(package).map(|p| p.version.as_str())
    }

    /// Add an installed package, auto-maintaining explicit/pinned indices
    pub fn add_package(&mut self, name: String, pkg: InstalledPackage) {
        if pkg.explicit {
            self.explicit.insert(name.clone());
        }
        if pkg.pinned {
            self.pinned.insert(name.clone());
        }
        self.installed.entry(name).or_default().push(pkg);
    }

    /// Get all installed instances of a package (multi-arch aware)
    pub fn get_all_instances(&self, package: &str) -> &[InstalledPackage] {
        self.installed
            .get(package)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Check if a package was explicitly installed (any architecture)
    pub fn is_explicit(&self, package: &str) -> bool {
        self.explicit.contains(package)
    }

    /// Check if a specific architecture of a package was explicitly installed
    pub fn is_explicit_arch(&self, package: &str, arch: &str) -> bool {
        self.get_all_instances(package)
            .iter()
            .any(|p| p.explicit && p.architecture.as_deref() == Some(arch))
    }

    /// Check if a package is pinned (any architecture)
    pub fn is_pinned(&self, package: &str) -> bool {
        self.pinned.contains(package)
    }

    /// Get all installed package names (deduplicated across architectures)
    pub fn installed_packages(&self) -> impl Iterator<Item = &str> {
        self.installed.keys().map(|s| s.as_str())
    }

    /// Get count of installed packages (unique names, not instances)
    pub fn package_count(&self) -> usize {
        self.installed.len()
    }

    /// Get total count of installed instances (including multi-arch)
    pub fn instance_count(&self) -> usize {
        self.installed.values().map(|v| v.len()).sum()
    }
}

impl Default for SystemState {
    fn default() -> Self {
        Self::new()
    }
}

/// Capture the current system state from the database
pub fn capture_current_state(conn: &Connection) -> ModelResult<SystemState> {
    let mut state = SystemState::new();

    // Query all installed troves (packages)
    let mut stmt = conn
        .prepare(
            r#"
            SELECT
                t.name,
                t.version,
                t.architecture,
                t.install_reason,
                t.pinned,
                l.repository || '@' || l.namespace || ':' || l.tag as label_name
            FROM troves t
            LEFT JOIN labels l ON t.label_id = l.id
            WHERE t.type = 'package'
            ORDER BY t.name
            "#,
        )
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,               // name
                row.get::<_, String>(1)?,               // version
                row.get::<_, Option<String>>(2)?,       // architecture
                row.get::<_, Option<String>>(3)?,       // install_reason
                row.get::<_, bool>(4).unwrap_or(false), // pinned
                row.get::<_, Option<String>>(5)?,       // label_name
            ))
        })
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?;

    for row in rows {
        let (name, version, architecture, install_reason, pinned, label) =
            row.map_err(|e| ModelError::DatabaseError(e.to_string()))?;

        let explicit = install_reason.as_deref() == Some("explicit");

        let pkg = InstalledPackage {
            name: name.clone(),
            version,
            architecture,
            explicit,
            pinned,
            label,
        };

        // add_package auto-maintains explicit/pinned indices
        state.add_package(name, pkg);
    }

    state.source_pin = DistroPin::get_current(conn)
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
        .map(|pin| pin.as_source_pin());

    state.selection_mode = settings::get(conn, SETTINGS_KEY_SELECTION_MODE)
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
        .as_deref()
        .map(|raw| match raw {
            "policy" => Ok(SelectionMode::Policy),
            "latest" => Ok(SelectionMode::Latest),
            other => Err(ModelError::InvalidSourcePolicy(format!(
                "Unknown selection mode '{}'",
                other
            ))),
        })
        .transpose()?;

    state.allowed_distros = settings::get(conn, SETTINGS_KEY_ALLOWED_DISTROS)
        .map_err(|e| ModelError::DatabaseError(e.to_string()))?
        .map(|raw| {
            serde_json::from_str::<Vec<String>>(&raw)
                .map_err(|e| ModelError::DatabaseError(e.to_string()))
        })
        .transpose()?
        .unwrap_or_default();

    Ok(state)
}

/// Create a SystemModel from the current system state (for `model snapshot`)
pub fn snapshot_to_model(state: &SystemState) -> super::SystemModel {
    let mut model = super::SystemModel::new();

    // Add explicitly installed packages -- check per-instance explicitness
    // so multi-arch installs where only one arch is explicit are captured correctly
    for (name, instances) in &state.installed {
        if instances.iter().any(|p| p.explicit) {
            model.config.install.push(name.clone());
        }
    }
    model.config.install.sort();

    // Add pinned packages -- use per-instance pinned flag and pick the version
    // from the first pinned instance (all arches of the same package should
    // share the same version, but we're explicit about which one we use)
    for (name, instances) in &state.installed {
        if let Some(pinned_instance) = instances.iter().find(|p| p.pinned) {
            model
                .pin
                .insert(name.clone(), pinned_instance.version.clone());
        }
    }

    model.system.pin = state.source_pin.clone();
    model.system.selection_mode = state.selection_mode.map(|mode| match mode {
        SelectionMode::Policy => "policy".to_string(),
        SelectionMode::Latest => "latest".to_string(),
    });
    model.system.allowed_distros = state.allowed_distros.clone();

    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::DistroPin;
    use crate::db::models::settings;
    use crate::db::testing::create_test_db;
    use crate::repository::{SETTINGS_KEY_ALLOWED_DISTROS, SETTINGS_KEY_SELECTION_MODE};

    #[test]
    fn test_empty_state() {
        let state = SystemState::new();
        assert_eq!(state.package_count(), 0);
        assert!(!state.is_installed("nginx"));
    }

    #[test]
    fn test_state_operations() {
        let mut state = SystemState::new();

        state.add_package(
            "nginx".to_string(),
            InstalledPackage {
                name: "nginx".to_string(),
                version: "1.24.0".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                pinned: false,
                label: Some("fedora@f41:stable".to_string()),
            },
        );

        assert!(state.is_installed("nginx"));
        assert!(state.is_explicit("nginx"));
        assert_eq!(state.get_version("nginx"), Some("1.24.0"));
        assert_eq!(state.package_count(), 1);
    }

    #[test]
    fn test_snapshot_to_model() {
        let mut state = SystemState::new();

        state.add_package(
            "nginx".to_string(),
            InstalledPackage {
                name: "nginx".to_string(),
                version: "1.24.0".to_string(),
                architecture: None,
                explicit: true,
                pinned: true,
                label: None,
            },
        );

        let model = snapshot_to_model(&state);
        assert!(model.config.install.contains(&"nginx".to_string()));
        assert_eq!(model.pin.get("nginx"), Some(&"1.24.0".to_string()));
    }

    #[test]
    fn test_multi_arch_state() {
        let mut state = SystemState::new();

        // Simulate multilib: glibc.x86_64 (explicit) + glibc.i686 (dependency)
        state.add_package(
            "glibc".to_string(),
            InstalledPackage {
                name: "glibc".to_string(),
                version: "2.38".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                pinned: false,
                label: None,
            },
        );
        state.add_package(
            "glibc".to_string(),
            InstalledPackage {
                name: "glibc".to_string(),
                version: "2.38".to_string(),
                architecture: Some("i686".to_string()),
                explicit: false,
                pinned: false,
                label: None,
            },
        );

        // Both instances preserved (not collapsed)
        assert_eq!(state.package_count(), 1); // 1 unique name
        assert_eq!(state.instance_count(), 2); // 2 instances
        assert!(state.is_installed("glibc"));

        // Per-instance data accessible
        let instances = state.get_all_instances("glibc");
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].architecture.as_deref(), Some("x86_64"));
        assert_eq!(instances[1].architecture.as_deref(), Some("i686"));

        // Name-level explicit (at least one arch is explicit)
        assert!(state.is_explicit("glibc"));

        // Per-arch explicit
        assert!(state.is_explicit_arch("glibc", "x86_64"));
        assert!(!state.is_explicit_arch("glibc", "i686"));

        // Snapshot preserves explicitness correctly
        let model = snapshot_to_model(&state);
        assert!(model.config.install.contains(&"glibc".to_string()));
    }

    #[test]
    fn test_snapshot_to_model_captures_compatibility_distro_pin() {
        let (_temp, conn) = create_test_db();
        DistroPin::set(&conn, "arch", "strict").unwrap();

        let state = capture_current_state(&conn).unwrap();
        let model = snapshot_to_model(&state);

        let effective_pin = model.system.effective_pin().unwrap();
        assert_eq!(effective_pin.distro, "arch");
        assert_eq!(effective_pin.strength.as_deref(), Some("strict"));
    }

    #[test]
    fn source_policy_snapshot_includes_selection_mode_from_settings() {
        let (_temp, conn) = create_test_db();
        settings::set(&conn, SETTINGS_KEY_SELECTION_MODE, "policy").unwrap();

        let state = capture_current_state(&conn).unwrap();
        let model = snapshot_to_model(&state);

        assert_eq!(model.system.selection_mode.as_deref(), Some("policy"));
    }

    #[test]
    fn source_policy_snapshot_includes_allowed_distros_from_settings() {
        let (_temp, conn) = create_test_db();
        settings::set(&conn, SETTINGS_KEY_ALLOWED_DISTROS, "[\"arch\"]").unwrap();

        let state = capture_current_state(&conn).unwrap();
        let model = snapshot_to_model(&state);

        assert_eq!(model.system.allowed_distros, vec!["arch".to_string()]);
    }
}
