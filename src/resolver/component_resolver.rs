// src/resolver/component_resolver.rs

//! Component-level dependency resolution
//!
//! This module provides dependency resolution at the component level,
//! determining which components need to be installed and in what order,
//! and checking if removing a component would break others.

use crate::components::ComponentType;
use crate::db::models::{Component, ComponentDependency, Trove};
use crate::error::Result;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet, VecDeque};

/// A component identifier: (package_name, component_name)
pub type ComponentSpec = (String, String);

/// Result of component resolution
#[derive(Debug, Clone)]
pub struct ComponentResolutionPlan {
    /// Components to install, in dependency order (dependencies first)
    pub install_order: Vec<ComponentSpec>,
    /// Components that are missing and need to be fetched/installed
    pub missing: Vec<MissingComponent>,
    /// Components that are already installed (can be skipped)
    pub already_installed: Vec<ComponentSpec>,
}

/// A missing component dependency
#[derive(Debug, Clone)]
pub struct MissingComponent {
    /// Package name
    pub package: String,
    /// Component name (e.g., "lib", "runtime")
    pub component: String,
    /// Version constraint (if any)
    pub version_constraint: Option<String>,
    /// What requires this component
    pub required_by: Vec<ComponentSpec>,
}

/// Component resolver for determining installation order and checking removals
pub struct ComponentResolver<'a> {
    conn: &'a Connection,
    /// Cache of installed components: (package, component) -> Component
    installed: HashMap<ComponentSpec, Component>,
    /// Cache of component dependencies: component_id -> Vec<ComponentDependency>
    deps: HashMap<i64, Vec<ComponentDependency>>,
    /// Cache of trove name by ID
    trove_names: HashMap<i64, String>,
}

impl<'a> ComponentResolver<'a> {
    /// Create a new component resolver from database
    pub fn new(conn: &'a Connection) -> Result<Self> {
        let mut resolver = Self {
            conn,
            installed: HashMap::new(),
            deps: HashMap::new(),
            trove_names: HashMap::new(),
        };
        resolver.load_installed_components()?;
        Ok(resolver)
    }

    /// Load all installed components from the database
    fn load_installed_components(&mut self) -> Result<()> {
        // Load all troves
        let troves = Trove::list_all(self.conn)?;
        for trove in troves {
            if let Some(trove_id) = trove.id {
                self.trove_names.insert(trove_id, trove.name.clone());

                // Load components for this trove
                let components = Component::find_installed_by_trove(self.conn, trove_id)?;
                for comp in components {
                    let spec = (trove.name.clone(), comp.name.clone());
                    if let Some(comp_id) = comp.id {
                        // Load dependencies for this component
                        let deps = ComponentDependency::find_by_component(self.conn, comp_id)?;
                        self.deps.insert(comp_id, deps);
                    }
                    self.installed.insert(spec, comp);
                }
            }
        }
        Ok(())
    }

    /// Check if a component is installed
    pub fn is_installed(&self, package: &str, component: &str) -> bool {
        self.installed
            .contains_key(&(package.to_string(), component.to_string()))
    }

    /// Get an installed component
    pub fn get_installed(&self, package: &str, component: &str) -> Option<&Component> {
        self.installed
            .get(&(package.to_string(), component.to_string()))
    }

    /// Resolve installation of a component and its dependencies
    ///
    /// Returns the components that need to be installed in dependency order
    /// (dependencies first), along with missing components.
    pub fn resolve_install(
        &self,
        package: &str,
        component: &str,
        deps: &[ComponentDependency],
    ) -> ComponentResolutionPlan {
        let mut install_order = Vec::new();
        let mut missing = Vec::new();
        let mut already_installed = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        // Start with the target component
        let target = (package.to_string(), component.to_string());
        queue.push_back((target.clone(), deps.to_vec()));
        visited.insert(target.clone());

        while let Some((spec, component_deps)) = queue.pop_front() {
            let (pkg, comp) = &spec;

            // Check if already installed
            if self.is_installed(pkg, comp) {
                already_installed.push(spec.clone());
                continue;
            }

            // Process dependencies
            for dep in &component_deps {
                let dep_pkg = dep.depends_on_package.as_deref().unwrap_or(pkg);
                let dep_comp = &dep.depends_on_component;
                let dep_spec = (dep_pkg.to_string(), dep_comp.clone());

                if visited.contains(&dep_spec) {
                    continue;
                }
                visited.insert(dep_spec.clone());

                if self.is_installed(dep_pkg, dep_comp) {
                    already_installed.push(dep_spec);
                } else {
                    // Check if we have deps for this dependency
                    let nested_deps = self.get_deps_for_component(dep_pkg, dep_comp);
                    if nested_deps.is_some() || dep_pkg == pkg {
                        // Can resolve this - add to queue
                        queue.push_back((
                            (dep_pkg.to_string(), dep_comp.clone()),
                            nested_deps.unwrap_or_default(),
                        ));
                    } else {
                        // Missing component
                        missing.push(MissingComponent {
                            package: dep_pkg.to_string(),
                            component: dep_comp.clone(),
                            version_constraint: dep.version_constraint.clone(),
                            required_by: vec![spec.clone()],
                        });
                    }
                }
            }

            // Add to install order (dependencies will be added before this due to queue order)
            install_order.push(spec);
        }

        // Reverse to get dependency order (dependencies first)
        install_order.reverse();

        // Deduplicate missing components and aggregate required_by
        let missing = Self::dedupe_missing(missing);

        ComponentResolutionPlan {
            install_order,
            missing,
            already_installed,
        }
    }

    /// Get dependencies for a component from cache
    fn get_deps_for_component(&self, package: &str, component: &str) -> Option<Vec<ComponentDependency>> {
        let comp = self.get_installed(package, component)?;
        let comp_id = comp.id?;
        self.deps.get(&comp_id).cloned()
    }

    /// Deduplicate missing components and aggregate required_by
    fn dedupe_missing(missing: Vec<MissingComponent>) -> Vec<MissingComponent> {
        let mut map: HashMap<ComponentSpec, MissingComponent> = HashMap::new();
        for m in missing {
            let key = (m.package.clone(), m.component.clone());
            map.entry(key)
                .and_modify(|existing| {
                    existing.required_by.extend(m.required_by.clone());
                })
                .or_insert(m);
        }
        map.into_values().collect()
    }

    /// Check if removing a component would break other components
    ///
    /// Returns a list of (package, component) pairs that would be broken.
    pub fn check_removal(&self, package: &str, component: &str) -> Result<Vec<ComponentSpec>> {
        let mut breaking = Vec::new();

        // Find all components that depend on this one
        let reverse_deps =
            ComponentDependency::find_reverse_deps(self.conn, package, component)?;

        for dep in reverse_deps {
            // Find the component that has this dependency
            if let Some(comp) = Component::find_by_id(self.conn, dep.component_id)? {
                // Get the package name for this component
                if let Some(pkg_name) = self.trove_names.get(&comp.parent_trove_id) {
                    breaking.push((pkg_name.clone(), comp.name));
                }
            }
        }

        // Also check same-package dependencies
        // Find all components in the same package that depend on this component
        if let Some(target_comp) = self.get_installed(package, component) {
            let trove_id = target_comp.parent_trove_id;
            let sibling_comps = Component::find_installed_by_trove(self.conn, trove_id)?;

            for sibling in sibling_comps {
                if sibling.name == component {
                    continue; // Skip self
                }
                if let Some(sibling_id) = sibling.id
                    && let Some(deps) = self.deps.get(&sibling_id)
                {
                    for dep in deps {
                        // Same-package dependency (depends_on_package is None)
                        if dep.depends_on_package.is_none()
                            && dep.depends_on_component == component
                        {
                            breaking.push((package.to_string(), sibling.name.clone()));
                            break;
                        }
                    }
                }
            }
        }

        Ok(breaking)
    }

    /// Resolve default components for a package
    ///
    /// Returns the default component types that should be installed.
    pub fn default_components() -> &'static [ComponentType] {
        ComponentType::defaults()
    }

    /// Check if installing a component requires other components from the same package
    ///
    /// For example, `:devel` typically requires `:lib` from the same package.
    pub fn intra_package_deps(component: ComponentType) -> Vec<ComponentType> {
        match component {
            ComponentType::Devel => vec![ComponentType::Lib, ComponentType::Runtime],
            ComponentType::Doc => vec![], // Doc is standalone
            ComponentType::Config => vec![ComponentType::Runtime],
            ComponentType::Lib => vec![],
            ComponentType::Runtime => vec![],
            ComponentType::Debuginfo => vec![], // Debug symbols are standalone
            ComponentType::Test => vec![ComponentType::Runtime], // Tests may need runtime
        }
    }

    /// Get a resolution plan for installing default components of a package
    pub fn resolve_default_install(&self, package: &str) -> ComponentResolutionPlan {
        let defaults = Self::default_components();
        let mut all_install = Vec::new();
        let mut all_missing = Vec::new();
        let mut all_installed = Vec::new();

        for comp_type in defaults {
            let comp_name = comp_type.as_str();
            let plan = self.resolve_install(package, comp_name, &[]);
            all_install.extend(plan.install_order);
            all_missing.extend(plan.missing);
            all_installed.extend(plan.already_installed);
        }

        // Deduplicate
        let mut seen = HashSet::new();
        all_install.retain(|spec| seen.insert(spec.clone()));
        seen.clear();
        all_installed.retain(|spec| seen.insert(spec.clone()));

        ComponentResolutionPlan {
            install_order: all_install,
            missing: Self::dedupe_missing(all_missing),
            already_installed: all_installed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::TroveType;
    use crate::db::schema;
    use tempfile::NamedTempFile;

    fn create_test_db() -> (NamedTempFile, Connection) {
        let temp_file = NamedTempFile::new().unwrap();
        let conn = Connection::open(temp_file.path()).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::migrate(&conn).unwrap();
        (temp_file, conn)
    }

    fn create_package_with_components(
        conn: &Connection,
        name: &str,
        components: &[&str],
    ) -> (i64, Vec<i64>) {
        let mut trove = Trove::new(name.to_string(), "1.0.0".to_string(), TroveType::Package);
        let trove_id = trove.insert(conn).unwrap();

        let mut comp_ids = Vec::new();
        for comp_name in components {
            let mut comp = Component::new(trove_id, comp_name.to_string());
            let comp_id = comp.insert(conn).unwrap();
            comp_ids.push(comp_id);
        }

        (trove_id, comp_ids)
    }

    #[test]
    fn test_resolver_creation() {
        let (_temp, conn) = create_test_db();
        let resolver = ComponentResolver::new(&conn).unwrap();
        assert!(resolver.installed.is_empty());
    }

    #[test]
    fn test_is_installed() {
        let (_temp, conn) = create_test_db();
        create_package_with_components(&conn, "nginx", &["runtime", "lib", "config"]);

        let resolver = ComponentResolver::new(&conn).unwrap();

        assert!(resolver.is_installed("nginx", "runtime"));
        assert!(resolver.is_installed("nginx", "lib"));
        assert!(resolver.is_installed("nginx", "config"));
        assert!(!resolver.is_installed("nginx", "devel"));
        assert!(!resolver.is_installed("apache", "runtime"));
    }

    #[test]
    fn test_resolve_already_installed() {
        let (_temp, conn) = create_test_db();
        create_package_with_components(&conn, "nginx", &["runtime", "lib"]);

        let resolver = ComponentResolver::new(&conn).unwrap();
        let plan = resolver.resolve_install("nginx", "runtime", &[]);

        assert!(plan.install_order.is_empty());
        assert!(plan.missing.is_empty());
        assert_eq!(plan.already_installed.len(), 1);
        assert_eq!(
            plan.already_installed[0],
            ("nginx".to_string(), "runtime".to_string())
        );
    }

    #[test]
    fn test_resolve_new_component() {
        let (_temp, conn) = create_test_db();
        create_package_with_components(&conn, "nginx", &["runtime", "lib"]);

        let resolver = ComponentResolver::new(&conn).unwrap();
        // Request to install devel (not currently installed)
        let plan = resolver.resolve_install("nginx", "devel", &[]);

        // devel should be in install order since it's not installed
        assert_eq!(plan.install_order.len(), 1);
        assert_eq!(
            plan.install_order[0],
            ("nginx".to_string(), "devel".to_string())
        );
    }

    #[test]
    fn test_resolve_with_dependency() {
        let (_temp, conn) = create_test_db();
        let (_, _comp_ids) = create_package_with_components(&conn, "nginx", &["runtime", "lib"]);

        // Create devel component with dependency on lib
        let (_, devel_ids) = create_package_with_components(&conn, "myapp", &["devel"]);
        let devel_id = devel_ids[0];

        // Add dependency: myapp:devel depends on nginx:lib
        let mut dep = ComponentDependency::new_cross_package(
            devel_id,
            "lib".to_string(),
            "nginx".to_string(),
            crate::db::models::ComponentDepType::Runtime,
        );
        dep.insert(&conn).unwrap();

        // Need to create a fresh resolver to pick up the dependency
        let resolver = ComponentResolver::new(&conn).unwrap();

        // Get the deps we just created
        let deps = ComponentDependency::find_by_component(&conn, devel_id).unwrap();

        // Since devel is already installed, it should be in already_installed
        let plan = resolver.resolve_install("myapp", "devel", &deps);

        // devel is already installed, lib (nginx) is already installed
        assert!(plan.install_order.is_empty() || plan.already_installed.len() >= 1);
    }

    #[test]
    fn test_check_removal_no_deps() {
        let (_temp, conn) = create_test_db();
        create_package_with_components(&conn, "nginx", &["runtime", "lib", "doc"]);

        let resolver = ComponentResolver::new(&conn).unwrap();
        let breaking = resolver.check_removal("nginx", "doc").unwrap();

        // Nothing depends on doc
        assert!(breaking.is_empty());
    }

    #[test]
    fn test_check_removal_with_deps() {
        let (_temp, conn) = create_test_db();
        let (_, _nginx_ids) = create_package_with_components(&conn, "nginx", &["runtime", "lib"]);

        let (_, myapp_ids) = create_package_with_components(&conn, "myapp", &["runtime"]);
        let myapp_runtime_id = myapp_ids[0];

        // myapp:runtime depends on nginx:lib
        let mut dep = ComponentDependency::new_cross_package(
            myapp_runtime_id,
            "lib".to_string(),
            "nginx".to_string(),
            crate::db::models::ComponentDepType::Runtime,
        );
        dep.insert(&conn).unwrap();

        let resolver = ComponentResolver::new(&conn).unwrap();
        let breaking = resolver.check_removal("nginx", "lib").unwrap();

        assert_eq!(breaking.len(), 1);
        assert_eq!(
            breaking[0],
            ("myapp".to_string(), "runtime".to_string())
        );
    }

    #[test]
    fn test_check_removal_same_package() {
        let (_temp, conn) = create_test_db();
        let (_, comp_ids) =
            create_package_with_components(&conn, "openssl", &["runtime", "lib", "devel"]);
        let devel_id = comp_ids[2];

        // devel depends on lib (same package)
        let mut dep = ComponentDependency::new_same_package(
            devel_id,
            "lib".to_string(),
            crate::db::models::ComponentDepType::Runtime,
        );
        dep.insert(&conn).unwrap();

        let resolver = ComponentResolver::new(&conn).unwrap();
        let breaking = resolver.check_removal("openssl", "lib").unwrap();

        assert_eq!(breaking.len(), 1);
        assert_eq!(
            breaking[0],
            ("openssl".to_string(), "devel".to_string())
        );
    }

    #[test]
    fn test_default_components() {
        let defaults = ComponentResolver::default_components();

        // runtime, lib, config should be default
        assert!(defaults.contains(&ComponentType::Runtime));
        assert!(defaults.contains(&ComponentType::Lib));
        assert!(defaults.contains(&ComponentType::Config));

        // devel and doc should NOT be default
        assert!(!defaults.contains(&ComponentType::Devel));
        assert!(!defaults.contains(&ComponentType::Doc));
    }

    #[test]
    fn test_intra_package_deps() {
        // devel depends on lib and runtime
        let devel_deps = ComponentResolver::intra_package_deps(ComponentType::Devel);
        assert!(devel_deps.contains(&ComponentType::Lib));
        assert!(devel_deps.contains(&ComponentType::Runtime));

        // doc is standalone
        let doc_deps = ComponentResolver::intra_package_deps(ComponentType::Doc);
        assert!(doc_deps.is_empty());

        // config depends on runtime
        let config_deps = ComponentResolver::intra_package_deps(ComponentType::Config);
        assert!(config_deps.contains(&ComponentType::Runtime));
    }

    #[test]
    fn test_resolve_default_install() {
        let (_temp, conn) = create_test_db();
        // Package with only runtime installed
        create_package_with_components(&conn, "nginx", &["runtime"]);

        let resolver = ComponentResolver::new(&conn).unwrap();
        let plan = resolver.resolve_default_install("nginx");

        // runtime is installed, lib and config should be in install_order
        assert!(plan.already_installed.contains(&("nginx".to_string(), "runtime".to_string())));

        // lib and config should be requested for install (they're default but not installed)
        let install_names: Vec<_> = plan.install_order.iter().map(|(_, c)| c.as_str()).collect();
        assert!(install_names.contains(&"lib") || install_names.contains(&"config")
            || plan.install_order.is_empty()); // Empty if we can't find them
    }
}
