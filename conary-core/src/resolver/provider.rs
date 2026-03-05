// conary-core/src/resolver/provider.rs

//! Bridge between Conary's data model and resolvo's SAT solver.
//!
//! Implements resolvo's `DependencyProvider` and `Interner` traits via
//! `ConaryProvider`, which maps Conary packages (troves + repo packages)
//! to resolvo's abstract solvable/name/version-set model.

use std::collections::HashMap;
use std::fmt;

use resolvo::{
    Candidates, Condition, ConditionId, ConditionalRequirement, Dependencies, DependencyProvider,
    HintDependenciesAvailable, Interner, KnownDependencies, NameId, SolvableId, SolverCache,
    StringId, VersionSetId, VersionSetUnionId,
};

use crate::db::models::{DependencyEntry, ProvideEntry, Trove};
use crate::error::Result;
use crate::version::{RpmVersion, VersionConstraint};

/// A solvable package — either an installed trove or a repository candidate.
#[derive(Debug, Clone)]
pub struct ConaryPackage {
    pub name: String,
    pub version: RpmVersion,
    /// `Some` when this package is currently installed.
    pub trove_id: Option<i64>,
    /// `Some` when this package is from a repository.
    pub repo_package_id: Option<i64>,
}

/// Bridge between Conary's data model and resolvo's abstract solver interface.
///
/// Owns interning pools for names, solvables, version sets, and strings,
/// and queries Conary's DB on demand to feed the solver.
pub struct ConaryProvider<'db> {
    // --- Interning pools ---
    names: Vec<String>,
    name_to_id: HashMap<String, NameId>,

    solvables: Vec<ConaryPackage>,

    /// Each version set is (name_id, constraint).
    version_sets: Vec<(NameId, VersionConstraint)>,

    strings: Vec<String>,

    /// Pre-loaded dependencies for each solvable, keyed by `SolvableId` index.
    dependencies: HashMap<u32, Vec<(String, VersionConstraint)>>,

    // --- Data source ---
    conn: &'db rusqlite::Connection,
}

impl<'db> ConaryProvider<'db> {
    /// Create a new provider backed by the given database connection.
    pub fn new(conn: &'db rusqlite::Connection) -> Self {
        Self {
            names: Vec::new(),
            name_to_id: HashMap::new(),
            solvables: Vec::new(),
            version_sets: Vec::new(),
            strings: Vec::new(),
            dependencies: HashMap::new(),
            conn,
        }
    }

    /// Intern a package name, returning its `NameId`.
    pub fn intern_name(&mut self, name: &str) -> NameId {
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }
        let id = NameId(self.names.len() as u32);
        self.names.push(name.to_string());
        self.name_to_id.insert(name.to_string(), id);
        id
    }

    /// Intern a version constraint for a given name.
    pub fn intern_version_set(
        &mut self,
        name_id: NameId,
        constraint: VersionConstraint,
    ) -> VersionSetId {
        let id = VersionSetId(self.version_sets.len() as u32);
        self.version_sets.push((name_id, constraint));
        id
    }

    /// Intern a display string, returning its `StringId`.
    pub fn intern_string(&mut self, s: &str) -> StringId {
        let id = StringId(self.strings.len() as u32);
        self.strings.push(s.to_string());
        id
    }

    /// Register a solvable (package candidate) and return its `SolvableId`.
    pub fn add_solvable(&mut self, pkg: ConaryPackage) -> SolvableId {
        let id = SolvableId(self.solvables.len() as u32);
        self.solvables.push(pkg);
        id
    }

    /// Bulk-load all installed troves as solvables.
    pub fn load_installed_packages(&mut self) -> Result<()> {
        let troves = Trove::list_all(self.conn)?;
        for trove in troves {
            let trove_id = trove.id;
            let version = RpmVersion::parse(&trove.version)?;
            let name_id = self.intern_name(&trove.name);

            let pkg = ConaryPackage {
                name: trove.name.clone(),
                version,
                trove_id,
                repo_package_id: None,
            };
            let solvable_id = self.add_solvable(pkg);

            // Load dependencies for this installed trove
            if let Some(tid) = trove_id {
                let deps = DependencyEntry::find_by_trove(self.conn, tid)?;
                let dep_list: Vec<(String, VersionConstraint)> = deps
                    .into_iter()
                    .filter(|d| !ProvideEntry::is_virtual_provide(&d.depends_on_name))
                    .map(|d| {
                        let constraint = d
                            .version_constraint
                            .as_deref()
                            .and_then(|s| VersionConstraint::parse(s).ok())
                            .unwrap_or(VersionConstraint::Any);
                        (d.depends_on_name, constraint)
                    })
                    .collect();
                self.dependencies.insert(solvable_id.0, dep_list);
            }

            // Ensure this name is known
            let _ = name_id;
        }
        Ok(())
    }

    /// Load repository packages for a set of names as additional candidates.
    ///
    /// This queries the `repository_packages` table for each name and adds
    /// any packages not already present as solvables.
    pub fn load_repo_packages_for_names(&mut self, names: &[String]) -> Result<()> {
        use crate::repository::selector::{PackageSelector, SelectionOptions};

        let options = SelectionOptions::default();
        for name in names {
            // Skip if we already have a repo package for this name
            let already_has_repo = self
                .solvables
                .iter()
                .any(|s| s.name == *name && s.repo_package_id.is_some());
            if already_has_repo {
                continue;
            }

            if let Ok(pkg_with_repo) = PackageSelector::find_best_package(self.conn, name, &options)
            {
                let version = match RpmVersion::parse(&pkg_with_repo.package.version) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let _name_id = self.intern_name(name);

                let pkg = ConaryPackage {
                    name: name.clone(),
                    version,
                    trove_id: None,
                    repo_package_id: pkg_with_repo.package.id,
                };
                let solvable_id = self.add_solvable(pkg);

                // Parse dependencies from repo metadata
                if let Ok(sub_deps) = pkg_with_repo.package.parse_dependencies() {
                    let dep_list: Vec<(String, VersionConstraint)> = sub_deps
                        .into_iter()
                        .filter(|d| !ProvideEntry::is_virtual_provide(d))
                        .map(|d| (d, VersionConstraint::Any))
                        .collect();
                    self.dependencies.insert(solvable_id.0, dep_list);
                }
            }
        }
        Ok(())
    }

    /// Look up which real packages provide a virtual capability.
    fn resolve_virtual_provide(&self, capability: &str) -> Vec<String> {
        let mut providers = Vec::new();

        // Query the provides table for packages that provide this capability
        if let Ok(Some(provide)) = ProvideEntry::find_by_capability(self.conn, capability) {
            // Look up the trove name from the trove_id
            if let Ok(troves) = Trove::list_all(self.conn) {
                for trove in troves {
                    if trove.id == Some(provide.trove_id) {
                        providers.push(trove.name.clone());
                        break;
                    }
                }
            }
        }

        providers
    }

    /// Get the solvable package at a given index.
    pub fn get_solvable(&self, id: SolvableId) -> &ConaryPackage {
        &self.solvables[id.0 as usize]
    }

    /// Get the total number of solvables.
    pub fn solvable_count(&self) -> usize {
        self.solvables.len()
    }

    /// Get the dependency list for a solvable (if loaded).
    pub fn get_dependency_list(&self, id: SolvableId) -> Option<&[(String, VersionConstraint)]> {
        self.dependencies.get(&id.0).map(Vec::as_slice)
    }

    /// Collect all unique dependency names from loaded packages.
    pub fn dependency_names(&self) -> Vec<String> {
        let mut names = std::collections::HashSet::new();
        for dep_list in self.dependencies.values() {
            for (name, _) in dep_list {
                names.insert(name.clone());
            }
        }
        names.into_iter().collect()
    }

    /// Intern version sets for all loaded dependencies so that `get_dependencies`
    /// can find them when the solver queries.
    pub fn intern_all_dependency_version_sets(&mut self) {
        // Collect all dependencies first to avoid borrowing issues
        let all_deps: Vec<(u32, Vec<(String, VersionConstraint)>)> = self
            .dependencies
            .iter()
            .map(|(&sid, deps)| (sid, deps.clone()))
            .collect();

        for (_sid, deps) in &all_deps {
            for (dep_name, constraint) in deps {
                let name_id = self.intern_name(dep_name);
                // Check if this version set already exists
                let already_exists = self
                    .version_sets
                    .iter()
                    .any(|(nid, c)| *nid == name_id && c == constraint);
                if !already_exists {
                    self.intern_version_set(name_id, constraint.clone());
                }
            }
        }
    }

    /// Find all solvables that match a given package name.
    fn solvables_for_name(&self, name_id: NameId) -> Vec<SolvableId> {
        let name = &self.names[name_id.0 as usize];
        self.solvables
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == *name)
            .map(|(i, _)| SolvableId(i as u32))
            .collect()
    }

    /// Find the installed solvable for a name, if any.
    fn installed_solvable_for_name(&self, name_id: NameId) -> Option<SolvableId> {
        let name = &self.names[name_id.0 as usize];
        self.solvables
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == *name && s.trove_id.is_some())
            .map(|(i, _)| SolvableId(i as u32))
    }
}

// --- Display helpers ---

struct DisplayName<'a>(&'a str);
impl fmt::Display for DisplayName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct DisplaySolvable<'a> {
    name: &'a str,
    version: &'a RpmVersion,
}
impl fmt::Display for DisplaySolvable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, self.version)
    }
}

struct DisplayVersionSet<'a>(&'a VersionConstraint);
impl fmt::Display for DisplayVersionSet<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct DisplayString<'a>(&'a str);
impl fmt::Display for DisplayString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// --- Interner implementation ---

impl Interner for ConaryProvider<'_> {
    fn display_solvable(&self, solvable: SolvableId) -> impl fmt::Display + '_ {
        let pkg = &self.solvables[solvable.0 as usize];
        DisplaySolvable {
            name: &pkg.name,
            version: &pkg.version,
        }
    }

    fn display_name(&self, name: NameId) -> impl fmt::Display + '_ {
        DisplayName(&self.names[name.0 as usize])
    }

    fn display_version_set(&self, version_set: VersionSetId) -> impl fmt::Display + '_ {
        DisplayVersionSet(&self.version_sets[version_set.0 as usize].1)
    }

    fn display_string(&self, string_id: StringId) -> impl fmt::Display + '_ {
        DisplayString(&self.strings[string_id.0 as usize])
    }

    fn version_set_name(&self, version_set: VersionSetId) -> NameId {
        self.version_sets[version_set.0 as usize].0
    }

    fn solvable_name(&self, solvable: SolvableId) -> NameId {
        let pkg = &self.solvables[solvable.0 as usize];
        self.name_to_id[&pkg.name]
    }

    fn version_sets_in_union(
        &self,
        _version_set_union: VersionSetUnionId,
    ) -> impl Iterator<Item = VersionSetId> {
        // We don't use unions — return empty iterator
        std::iter::empty()
    }

    fn resolve_condition(&self, _condition: ConditionId) -> Condition {
        // We don't use conditions — this should never be called
        unreachable!("ConaryProvider does not use conditions")
    }
}

// --- DependencyProvider implementation ---

impl DependencyProvider for ConaryProvider<'_> {
    async fn filter_candidates(
        &self,
        candidates: &[SolvableId],
        version_set: VersionSetId,
        inverse: bool,
    ) -> Vec<SolvableId> {
        let (_, ref constraint) = self.version_sets[version_set.0 as usize];
        candidates
            .iter()
            .copied()
            .filter(|&sid| {
                let pkg = &self.solvables[sid.0 as usize];
                let matches = constraint.satisfies(&pkg.version);
                if inverse { !matches } else { matches }
            })
            .collect()
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        let candidates = self.solvables_for_name(name);

        if candidates.is_empty() {
            // Check if this is a virtual provide
            let name_str = &self.names[name.0 as usize];
            if ProvideEntry::is_virtual_provide(name_str) {
                let _providers = self.resolve_virtual_provide(name_str);
                // Virtual provides are resolved through real package names
                // that the solver will discover via dependencies
            }
            return None;
        }

        let favored = self.installed_solvable_for_name(name);

        Some(Candidates {
            candidates,
            favored,
            locked: None,
            hint_dependencies_available: HintDependenciesAvailable::All,
            excluded: Vec::new(),
        })
    }

    async fn sort_candidates(&self, _solver: &SolverCache<Self>, solvables: &mut [SolvableId]) {
        // Sort by version descending (newest first).
        // Installed packages sort before repo packages at the same version.
        solvables.sort_by(|a, b| {
            let pkg_a = &self.solvables[a.0 as usize];
            let pkg_b = &self.solvables[b.0 as usize];

            // Higher version first
            let version_cmp = pkg_b.version.cmp(&pkg_a.version);
            if version_cmp != std::cmp::Ordering::Equal {
                return version_cmp;
            }

            // Installed before repo at same version
            let a_installed = pkg_a.trove_id.is_some();
            let b_installed = pkg_b.trove_id.is_some();
            b_installed.cmp(&a_installed)
        });
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let mut requirements = Vec::new();

        if let Some(dep_list) = self.dependencies.get(&solvable.0) {
            for (dep_name, constraint) in dep_list {
                // Find or create the name id (we can't mutate self in async,
                // so we look up existing names only)
                if let Some(&dep_name_id) = self.name_to_id.get(dep_name) {
                    // Find a matching version set
                    let vs_id = self
                        .version_sets
                        .iter()
                        .enumerate()
                        .find(|(_, (nid, c))| *nid == dep_name_id && c == constraint)
                        .map(|(i, _)| VersionSetId(i as u32));

                    if let Some(vs_id) = vs_id {
                        requirements.push(ConditionalRequirement::from(vs_id));
                    }
                    // If no version set found, skip — the dependency wasn't
                    // pre-interned (will be handled at the sat.rs level)
                }
            }
        }

        Dependencies::Known(KnownDependencies {
            requirements,
            constrains: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn setup_test_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        db::init(&db_path).unwrap();
        let conn = db::open(&db_path).unwrap();
        (temp_dir, conn)
    }

    #[test]
    fn test_intern_name_roundtrip() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let id1 = provider.intern_name("nginx");
        let id2 = provider.intern_name("nginx");
        let id3 = provider.intern_name("curl");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert_eq!(provider.names[id1.0 as usize], "nginx");
        assert_eq!(provider.names[id3.0 as usize], "curl");
    }

    #[test]
    fn test_version_set_filtering() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let name_id = provider.intern_name("lib");
        let constraint = VersionConstraint::parse(">= 2.0.0").unwrap();
        let vs_id = provider.intern_version_set(name_id, constraint);

        // Add candidates at different versions
        let s1 = provider.add_solvable(ConaryPackage {
            name: "lib".to_string(),
            version: RpmVersion::parse("1.0.0").unwrap(),
            trove_id: None,
            repo_package_id: None,
        });
        let s2 = provider.add_solvable(ConaryPackage {
            name: "lib".to_string(),
            version: RpmVersion::parse("2.0.0").unwrap(),
            trove_id: None,
            repo_package_id: None,
        });
        let s3 = provider.add_solvable(ConaryPackage {
            name: "lib".to_string(),
            version: RpmVersion::parse("3.0.0").unwrap(),
            trove_id: None,
            repo_package_id: None,
        });

        let candidates = [s1, s2, s3];

        // Test the filtering logic directly
        let (_, ref constraint) = provider.version_sets[vs_id.0 as usize];
        let matching: Vec<SolvableId> = candidates
            .iter()
            .copied()
            .filter(|&sid| constraint.satisfies(&provider.solvables[sid.0 as usize].version))
            .collect();

        assert_eq!(matching.len(), 2);
        assert!(matching.contains(&s2));
        assert!(matching.contains(&s3));
        assert!(!matching.contains(&s1));
    }

    #[test]
    fn test_favored_installed() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let name_id = provider.intern_name("nginx");

        // Add an installed version
        let installed = provider.add_solvable(ConaryPackage {
            name: "nginx".to_string(),
            version: RpmVersion::parse("1.0.0").unwrap(),
            trove_id: Some(42),
            repo_package_id: None,
        });

        // Add a repo version
        let _repo = provider.add_solvable(ConaryPackage {
            name: "nginx".to_string(),
            version: RpmVersion::parse("2.0.0").unwrap(),
            trove_id: None,
            repo_package_id: Some(100),
        });

        // Test candidates lookup logic directly
        let candidates = provider.solvables_for_name(name_id);
        let favored = provider.installed_solvable_for_name(name_id);

        assert_eq!(candidates.len(), 2);
        assert_eq!(favored, Some(installed));
    }

    #[test]
    fn test_sort_candidates_version_descending() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let s1 = provider.add_solvable(ConaryPackage {
            name: "pkg".to_string(),
            version: RpmVersion::parse("1.0.0").unwrap(),
            trove_id: None,
            repo_package_id: None,
        });
        let s2 = provider.add_solvable(ConaryPackage {
            name: "pkg".to_string(),
            version: RpmVersion::parse("3.0.0").unwrap(),
            trove_id: None,
            repo_package_id: None,
        });
        let s3 = provider.add_solvable(ConaryPackage {
            name: "pkg".to_string(),
            version: RpmVersion::parse("2.0.0").unwrap(),
            trove_id: Some(1), // installed
            repo_package_id: None,
        });

        // Test sort logic directly
        let mut solvables = [s1, s2, s3];
        solvables.sort_by(|a, b| {
            let pkg_a = &provider.solvables[a.0 as usize];
            let pkg_b = &provider.solvables[b.0 as usize];
            let version_cmp = pkg_b.version.cmp(&pkg_a.version);
            if version_cmp != std::cmp::Ordering::Equal {
                return version_cmp;
            }
            let a_installed = pkg_a.trove_id.is_some();
            let b_installed = pkg_b.trove_id.is_some();
            b_installed.cmp(&a_installed)
        });

        // Should be: 3.0.0 (repo), 2.0.0 (installed), 1.0.0 (repo)
        assert_eq!(solvables[0], s2); // 3.0.0
        assert_eq!(solvables[1], s3); // 2.0.0 (installed)
        assert_eq!(solvables[2], s1); // 1.0.0
    }

    #[test]
    fn test_display_methods() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let name_id = provider.intern_name("nginx");
        let vs_id =
            provider.intern_version_set(name_id, VersionConstraint::parse(">= 1.0.0").unwrap());
        let sid = provider.add_solvable(ConaryPackage {
            name: "nginx".to_string(),
            version: RpmVersion::parse("1.24.0").unwrap(),
            trove_id: None,
            repo_package_id: None,
        });
        let str_id = provider.intern_string("test string");

        assert_eq!(provider.display_name(name_id).to_string(), "nginx");
        assert_eq!(provider.display_solvable(sid).to_string(), "nginx=1.24.0");
        assert_eq!(provider.display_version_set(vs_id).to_string(), ">= 1.0.0");
        assert_eq!(provider.display_string(str_id).to_string(), "test string");
        assert_eq!(provider.version_set_name(vs_id), name_id);
        assert_eq!(provider.solvable_name(sid), name_id);
    }
}
