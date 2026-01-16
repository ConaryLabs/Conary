// src/model/state.rs

//! System state capture and representation.
//!
//! This module provides functionality to capture the current state of
//! installed packages from the Conary database.

use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

use super::{ModelError, ModelResult};

/// Represents the current state of the system
#[derive(Debug, Clone)]
pub struct SystemState {
    /// Currently installed packages (name -> version)
    pub installed: HashMap<String, InstalledPackage>,

    /// Explicitly installed packages (not just dependencies)
    pub explicit: HashSet<String>,

    /// Pinned packages
    pub pinned: HashSet<String>,
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
        }
    }

    /// Check if a package is installed
    pub fn is_installed(&self, package: &str) -> bool {
        self.installed.contains_key(package)
    }

    /// Get installed version of a package
    pub fn get_version(&self, package: &str) -> Option<&str> {
        self.installed.get(package).map(|p| p.version.as_str())
    }

    /// Check if a package was explicitly installed
    pub fn is_explicit(&self, package: &str) -> bool {
        self.explicit.contains(package)
    }

    /// Check if a package is pinned
    pub fn is_pinned(&self, package: &str) -> bool {
        self.pinned.contains(package)
    }

    /// Get all installed package names
    pub fn installed_packages(&self) -> impl Iterator<Item = &str> {
        self.installed.keys().map(|s| s.as_str())
    }

    /// Get count of installed packages
    pub fn package_count(&self) -> usize {
        self.installed.len()
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
                row.get::<_, String>(0)?,                    // name
                row.get::<_, String>(1)?,                    // version
                row.get::<_, Option<String>>(2)?,            // architecture
                row.get::<_, Option<String>>(3)?,            // install_reason
                row.get::<_, bool>(4).unwrap_or(false),      // pinned
                row.get::<_, Option<String>>(5)?,            // label_name
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
            label,
        };

        if explicit {
            state.explicit.insert(name.clone());
        }

        if pinned {
            state.pinned.insert(name.clone());
        }

        state.installed.insert(name, pkg);
    }

    Ok(state)
}

/// Create a SystemModel from the current system state (for `model snapshot`)
pub fn snapshot_to_model(state: &SystemState) -> super::SystemModel {
    let mut model = super::SystemModel::new();

    // Add explicitly installed packages
    model.config.install = state
        .explicit
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    model.config.install.sort();

    // Add pinned packages with their current versions
    for name in &state.pinned {
        if let Some(pkg) = state.installed.get(name) {
            model.pin.insert(name.clone(), pkg.version.clone());
        }
    }

    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_state() {
        let state = SystemState::new();
        assert_eq!(state.package_count(), 0);
        assert!(!state.is_installed("nginx"));
    }

    #[test]
    fn test_state_operations() {
        let mut state = SystemState::new();

        state.installed.insert(
            "nginx".to_string(),
            InstalledPackage {
                name: "nginx".to_string(),
                version: "1.24.0".to_string(),
                architecture: Some("x86_64".to_string()),
                explicit: true,
                label: Some("fedora@f41:stable".to_string()),
            },
        );
        state.explicit.insert("nginx".to_string());

        assert!(state.is_installed("nginx"));
        assert!(state.is_explicit("nginx"));
        assert_eq!(state.get_version("nginx"), Some("1.24.0"));
        assert_eq!(state.package_count(), 1);
    }

    #[test]
    fn test_snapshot_to_model() {
        let mut state = SystemState::new();

        state.installed.insert(
            "nginx".to_string(),
            InstalledPackage {
                name: "nginx".to_string(),
                version: "1.24.0".to_string(),
                architecture: None,
                explicit: true,
                label: None,
            },
        );
        state.explicit.insert("nginx".to_string());
        state.pinned.insert("nginx".to_string());

        let model = snapshot_to_model(&state);
        assert!(model.config.install.contains(&"nginx".to_string()));
        assert_eq!(model.pin.get("nginx"), Some(&"1.24.0".to_string()));
    }
}
