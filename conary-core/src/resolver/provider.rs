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

use crate::db::models::{
    DependencyEntry, ProvideEntry, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    RepositoryRequirementGroup, Trove, generate_capability_variations,
};
use crate::error::Result;
use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, compare_repo_versions, infer_version_scheme,
    parse_repo_constraint, repo_version_satisfies,
};
use crate::version::{RpmVersion, VersionConstraint};

/// A solvable package — either an installed trove or a repository candidate.
#[derive(Debug, Clone)]
pub struct ConaryPackage {
    pub name: String,
    pub version: ConaryPackageVersion,
    /// `Some` when this package is currently installed.
    pub trove_id: Option<i64>,
    /// `Some` when this package is from a repository.
    pub repo_package_id: Option<i64>,
    /// Capabilities this package provides, along with optional capability versions.
    pub provided_capabilities: Vec<(String, Option<ConaryProvidedVersion>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConaryPackageVersion {
    /// Installed package with RPM version (legacy or actual RPM).
    Installed(RpmVersion),
    /// Installed package with a native (non-RPM) version scheme.
    InstalledNative { raw: String, scheme: VersionScheme },
    Repository {
        raw: String,
        scheme: Option<VersionScheme>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConaryProvidedVersion {
    Installed(RpmVersion),
    Repository { raw: String, scheme: VersionScheme },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConaryConstraint {
    Legacy(VersionConstraint),
    Repository {
        scheme: VersionScheme,
        constraint: RepoVersionConstraint,
        raw: Option<String>,
    },
}

/// A single dependency entry for the solver, which may be a simple requirement
/// or an OR-group of alternatives.
#[derive(Debug, Clone)]
pub enum SolverDep {
    /// A single dependency: (name, constraint).
    Single(String, ConaryConstraint),
    /// An OR-group: any one of the alternatives satisfies the dependency.
    /// Each alternative is (name, constraint).
    OrGroup(Vec<(String, ConaryConstraint)>),
}

impl SolverDep {
    /// Returns the name if this is a `Single` dep; `None` for `OrGroup`.
    pub fn single_name(&self) -> Option<&str> {
        match self {
            Self::Single(name, _) => Some(name),
            Self::OrGroup(_) => None,
        }
    }

    /// Returns (name, constraint) if this is a `Single` dep.
    pub fn as_single(&self) -> Option<(&str, &ConaryConstraint)> {
        match self {
            Self::Single(name, constraint) => Some((name, constraint)),
            Self::OrGroup(_) => None,
        }
    }
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
    version_sets: Vec<(NameId, ConaryConstraint)>,

    /// Cache for deduplicating version sets by (name_id, constraint).
    version_set_cache: HashMap<(u32, ConaryConstraint), VersionSetId>,

    /// Each version set union is a list of `VersionSetId`s (OR alternatives).
    version_set_unions: Vec<Vec<VersionSetId>>,

    strings: Vec<String>,

    /// Pre-loaded dependencies for each solvable, keyed by `SolvableId` index.
    dependencies: HashMap<u32, Vec<SolverDep>>,

    /// Index of capability name -> list of (trove_id, optional version) providers.
    /// Built by `load_removal_data()` from already-loaded solvable provides.
    provides_index: HashMap<String, Vec<(i64, Option<String>)>>,

    /// Reverse map from trove_id to package name, for quick lookup during removal.
    trove_id_to_name: HashMap<i64, String>,

    /// Unfiltered dependencies for each solvable, including virtual provides
    /// (soname, perl, python, etc.) that `dependencies` strips out.
    /// Keyed by `SolvableId` index.
    removal_deps: HashMap<u32, Vec<SolverDep>>,

    /// distro_name -> Vec<equivalent distro_name> for canonical cross-distro resolution.
    /// Pre-loaded as a HashMap for O(1) lookup in the hot path.
    canonical_equivalents: HashMap<String, Vec<String>>,

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
            version_set_cache: HashMap::new(),
            version_set_unions: Vec::new(),
            strings: Vec::new(),
            dependencies: HashMap::new(),
            provides_index: HashMap::new(),
            trove_id_to_name: HashMap::new(),
            removal_deps: HashMap::new(),
            canonical_equivalents: HashMap::new(),
            conn,
        }
    }

    /// Intern a package name, returning its `NameId`.
    pub fn intern_name(&mut self, name: &str) -> NameId {
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }
        let id = NameId(u32::try_from(self.names.len()).expect("resolver name pool overflow"));
        let owned = name.to_string();
        self.names.push(owned.clone());
        self.name_to_id.insert(owned, id);
        id
    }

    /// Intern a version constraint for a given name, deduplicating via cache.
    pub fn intern_version_set(
        &mut self,
        name_id: NameId,
        constraint: VersionConstraint,
    ) -> VersionSetId {
        let constraint = ConaryConstraint::Legacy(constraint);
        let cache_key = (name_id.0, constraint.clone());
        if let Some(&existing) = self.version_set_cache.get(&cache_key) {
            return existing;
        }
        let id = VersionSetId(
            u32::try_from(self.version_sets.len()).expect("resolver version set pool overflow"),
        );
        self.version_sets.push((name_id, constraint));
        self.version_set_cache.insert(cache_key, id);
        id
    }

    pub fn intern_repo_version_set(
        &mut self,
        name_id: NameId,
        scheme: VersionScheme,
        constraint: RepoVersionConstraint,
        raw: Option<String>,
    ) -> VersionSetId {
        let constraint = ConaryConstraint::Repository {
            scheme,
            constraint,
            raw,
        };
        let cache_key = (name_id.0, constraint.clone());
        if let Some(&existing) = self.version_set_cache.get(&cache_key) {
            return existing;
        }
        let id = VersionSetId(
            u32::try_from(self.version_sets.len()).expect("resolver version set pool overflow"),
        );
        self.version_sets.push((name_id, constraint));
        self.version_set_cache.insert(cache_key, id);
        id
    }

    /// Intern a version set union (OR-group), returning its `VersionSetUnionId`.
    pub fn intern_version_set_union(&mut self, sets: Vec<VersionSetId>) -> VersionSetUnionId {
        let id = VersionSetUnionId(
            u32::try_from(self.version_set_unions.len())
                .expect("resolver version set union pool overflow"),
        );
        self.version_set_unions.push(sets);
        id
    }

    /// Intern a display string, returning its `StringId`.
    pub fn intern_string(&mut self, s: &str) -> StringId {
        let id =
            StringId(u32::try_from(self.strings.len()).expect("resolver string pool overflow"));
        self.strings.push(s.to_string());
        id
    }

    /// Register a solvable (package candidate) and return its `SolvableId`.
    pub fn add_solvable(&mut self, pkg: ConaryPackage) -> SolvableId {
        let id = SolvableId(
            u32::try_from(self.solvables.len()).expect("resolver solvable pool overflow"),
        );
        self.solvables.push(pkg);
        id
    }

    /// Bulk-load all installed troves as solvables.
    pub fn load_installed_packages(&mut self) -> Result<()> {
        let troves = Trove::list_all(self.conn)?;

        // Batch-load all dependencies in one query instead of N per-trove queries
        let trove_ids: Vec<i64> = troves.iter().filter_map(|t| t.id).collect();
        let all_deps = DependencyEntry::find_by_troves(self.conn, &trove_ids)?;

        for trove in troves {
            let trove_id = trove.id;
            let scheme = parse_stored_version_scheme(trove.version_scheme.as_deref());

            let version = match scheme {
                Some(VersionScheme::Rpm) | None => {
                    // RPM or legacy (no stored scheme): use RpmVersion
                    ConaryPackageVersion::Installed(RpmVersion::parse(&trove.version)?)
                }
                Some(native_scheme) => {
                    // Debian or Arch: use native scheme
                    ConaryPackageVersion::InstalledNative {
                        raw: trove.version.clone(),
                        scheme: native_scheme,
                    }
                }
            };

            let effective_scheme = scheme.unwrap_or(VersionScheme::Rpm);
            let provided_capabilities = if let Some(tid) = trove_id {
                ProvideEntry::find_by_trove(self.conn, tid)?
                    .into_iter()
                    .map(|provide| {
                        let prov_version =
                            provide
                                .version
                                .as_deref()
                                .and_then(|value| match effective_scheme {
                                    VersionScheme::Rpm => RpmVersion::parse(value)
                                        .ok()
                                        .map(ConaryProvidedVersion::Installed),
                                    _ => Some(ConaryProvidedVersion::Repository {
                                        raw: value.to_string(),
                                        scheme: effective_scheme,
                                    }),
                                });
                        (provide.capability, prov_version)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            // Intern name for side effect (ensures this name is known to the solver)
            let _name_id = self.intern_name(&trove.name);

            let pkg = ConaryPackage {
                name: trove.name.clone(),
                version,
                trove_id,
                repo_package_id: None,
                provided_capabilities,
            };
            let solvable_id = self.add_solvable(pkg);

            // Use batch-loaded dependencies
            if let Some(tid) = trove_id {
                let empty = Vec::new();
                let deps = all_deps.get(&tid).unwrap_or(&empty);
                let dep_list: Vec<SolverDep> = deps
                    .iter()
                    .filter(|d| !ProvideEntry::is_virtual_provide(&d.depends_on_name))
                    .map(|d| dep_entry_to_solver_dep(d, effective_scheme))
                    .collect();
                self.dependencies.insert(solvable_id.0, dep_list);
            }
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

            let mut candidates = Vec::new();
            if let Ok(pkg_with_repo) = PackageSelector::find_best_package(self.conn, name, &options)
            {
                candidates.push(pkg_with_repo);
            } else {
                for pkg_with_repo in self.find_repo_providers(name)? {
                    candidates.push(pkg_with_repo);
                }
            }

            for pkg_with_repo in candidates {
                if self
                    .solvables
                    .iter()
                    .any(|s| s.repo_package_id == pkg_with_repo.package.id)
                {
                    continue;
                }

                let scheme = infer_version_scheme(&pkg_with_repo.repository);

                let _name_id = self.intern_name(&pkg_with_repo.package.name);

                let pkg = ConaryPackage {
                    name: pkg_with_repo.package.name.clone(),
                    version: ConaryPackageVersion::Repository {
                        raw: pkg_with_repo.package.version.clone(),
                        scheme,
                    },
                    trove_id: None,
                    repo_package_id: pkg_with_repo.package.id,
                    provided_capabilities: load_repo_provided_capabilities(
                        self.conn,
                        &pkg_with_repo.package,
                        &pkg_with_repo.repository,
                    )?,
                };
                let solvable_id = self.add_solvable(pkg);

                let sub_deps = load_repo_dependency_requests(
                    self.conn,
                    &pkg_with_repo.package,
                    &pkg_with_repo.repository,
                )?;
                self.dependencies.insert(solvable_id.0, sub_deps);
            }
        }
        Ok(())
    }

    /// Look up which real packages provide a virtual capability.
    fn resolve_virtual_provide(&self, capability: &str) -> Vec<String> {
        let mut providers = Vec::new();

        // Query the provides table for packages that provide this capability
        if let Ok(Some(provide)) = ProvideEntry::find_by_capability(self.conn, capability) {
            // Direct O(1) lookup by trove_id instead of scanning all troves
            if let Ok(Some(trove)) = Trove::find_by_id(self.conn, provide.trove_id) {
                providers.push(trove.name.clone());
            }
        }

        providers
    }

    fn find_repo_providers(
        &self,
        capability: &str,
    ) -> Result<Vec<crate::repository::selector::PackageWithRepo>> {
        use crate::repository::selector::{PackageSelector, PackageWithRepo, SelectionOptions};

        let mut providers = Vec::<PackageWithRepo>::new();
        let normalized = RepositoryProvide::find_by_capability(self.conn, capability)?;
        if !normalized.is_empty() {
            for provide in normalized {
                let Some(pkg) = find_repo_package_by_id(self.conn, provide.repository_package_id)?
                else {
                    continue;
                };
                let Some(repo) =
                    crate::db::models::Repository::find_by_id(self.conn, pkg.repository_id)?
                else {
                    continue;
                };
                if !repo.enabled {
                    continue;
                }
                providers.push(PackageWithRepo {
                    package: pkg,
                    repository: repo,
                });
            }
            return Ok(providers);
        }

        let pattern = format!("%{}%", escape_like(capability));
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT name FROM repository_packages
             WHERE metadata LIKE ?1 ESCAPE '\\'
             ORDER BY LENGTH(name), name",
        )?;
        let rows = stmt.query_map([pattern], |row| row.get::<_, String>(0))?;
        for row in rows {
            let name = row?;
            if let Ok(pkg_with_repo) =
                PackageSelector::find_best_package(self.conn, &name, &SelectionOptions::default())
                && repo_package_provides_capability(self.conn, &pkg_with_repo.package, capability)?
            {
                providers.push(pkg_with_repo);
            }
        }
        Ok(providers)
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
    pub fn get_dependency_list(&self, id: SolvableId) -> Option<&[SolverDep]> {
        self.dependencies.get(&id.0).map(Vec::as_slice)
    }

    /// Collect all unique dependency names from loaded packages.
    pub fn dependency_names(&self) -> Vec<String> {
        self.new_dependency_names(&std::collections::HashSet::new())
    }

    /// Collect dependency names not already in `known`, avoiding redundant allocations.
    pub fn new_dependency_names(&self, known: &std::collections::HashSet<String>) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        for dep_list in self.dependencies.values() {
            for dep in dep_list {
                match dep {
                    SolverDep::Single(name, _) => {
                        if !known.contains(name.as_str()) {
                            seen.insert(name.clone());
                        }
                    }
                    SolverDep::OrGroup(alternatives) => {
                        for (name, _) in alternatives {
                            if !known.contains(name.as_str()) {
                                seen.insert(name.clone());
                            }
                        }
                    }
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Intern version sets for all loaded dependencies so that `get_dependencies`
    /// can find them when the solver queries.
    pub fn intern_all_dependency_version_sets(&mut self) {
        // Temporarily take ownership to avoid borrow conflict with self.intern_*()
        let all_deps = std::mem::take(&mut self.dependencies);

        for deps in all_deps.values() {
            for dep in deps {
                match dep {
                    SolverDep::Single(dep_name, constraint) => {
                        self.intern_constraint(dep_name, constraint);
                    }
                    SolverDep::OrGroup(alternatives) => {
                        let mut vs_ids = Vec::new();
                        for (dep_name, constraint) in alternatives {
                            self.intern_constraint(dep_name, constraint);
                            // Collect the interned version set IDs for the union
                            let name_id = self.intern_name(dep_name);
                            let cache_key = (name_id.0, constraint.clone());
                            if let Some(&vs_id) = self.version_set_cache.get(&cache_key) {
                                vs_ids.push(vs_id);
                            }
                        }
                        if vs_ids.len() > 1 {
                            self.intern_version_set_union(vs_ids);
                        }
                    }
                }
            }
        }

        // Restore the dependencies map
        self.dependencies = all_deps;
    }

    /// Intern a single constraint, creating a version set for it.
    fn intern_constraint(&mut self, dep_name: &str, constraint: &ConaryConstraint) {
        let name_id = self.intern_name(dep_name);
        match constraint {
            ConaryConstraint::Legacy(constraint) => {
                self.intern_version_set(name_id, constraint.clone());
            }
            ConaryConstraint::Repository {
                scheme,
                constraint,
                raw,
            } => {
                self.intern_repo_version_set(name_id, *scheme, constraint.clone(), raw.clone());
            }
        }
    }

    /// Look up a single (name, constraint) pair as a `ConditionalRequirement`.
    fn lookup_requirement(
        &self,
        dep_name: &str,
        constraint: &ConaryConstraint,
    ) -> Option<ConditionalRequirement> {
        let dep_name_id = self.name_to_id.get(dep_name)?;
        let cache_key = (dep_name_id.0, constraint.clone());
        let vs_id = self.version_set_cache.get(&cache_key).copied();
        if vs_id.is_none() {
            tracing::warn!(
                "Version set not interned for dependency '{}' -- skipping",
                dep_name
            );
        }
        vs_id.map(ConditionalRequirement::from)
    }

    /// Find a previously-interned union ID matching the given version set IDs.
    fn find_union_id(&self, vs_ids: &[VersionSetId]) -> Option<VersionSetUnionId> {
        self.version_set_unions
            .iter()
            .position(|sets| sets == vs_ids)
            .map(|i| VersionSetUnionId(u32::try_from(i).expect("union pool overflow")))
    }

    /// Find all solvables that match a given package name.
    fn solvables_for_name(&self, name_id: NameId) -> Vec<SolvableId> {
        let name = &self.names[name_id.0 as usize];
        self.solvables
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name == *name)
            .map(|(i, _)| SolvableId(u32::try_from(i).expect("resolver solvable pool overflow")))
            .collect()
    }

    fn solvables_for_provide(&self, capability: &str) -> Vec<SolvableId> {
        self.solvables
            .iter()
            .enumerate()
            .filter(|(_, solvable)| {
                solvable
                    .provided_capabilities
                    .iter()
                    .any(|(provided, _version)| provided == capability)
            })
            .map(|(i, _)| SolvableId(u32::try_from(i).expect("resolver solvable pool overflow")))
            .collect()
    }

    /// Find the installed solvable for a name, if any.
    fn installed_solvable_for_name(&self, name_id: NameId) -> Option<SolvableId> {
        let name = &self.names[name_id.0 as usize];
        self.solvables
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == *name && s.trove_id.is_some())
            .map(|(i, _)| SolvableId(u32::try_from(i).expect("resolver solvable pool overflow")))
    }

    /// Build the provides index, trove-id-to-name map, and unfiltered dependency
    /// list used by `solve_removal()`.
    ///
    /// Must be called after `load_installed_packages()`.  All data comes from
    /// already-loaded solvables and a single extra DB query per trove for the
    /// unfiltered dependency rows (the regular `dependencies` map strips
    /// virtual provides via `is_virtual_provide`).
    pub fn load_removal_data(&mut self) -> Result<()> {
        // 1. Build trove_id_to_name from loaded solvables.
        for solvable in &self.solvables {
            if let Some(tid) = solvable.trove_id {
                self.trove_id_to_name.insert(tid, solvable.name.clone());
            }
        }

        // 2. Build provides_index from already-loaded provided_capabilities.
        for solvable in &self.solvables {
            let Some(tid) = solvable.trove_id else {
                continue;
            };
            for (capability, prov_version) in &solvable.provided_capabilities {
                let version_str = prov_version.as_ref().map(|v| match v {
                    ConaryProvidedVersion::Installed(rpm) => rpm.to_string(),
                    ConaryProvidedVersion::Repository { raw, .. } => raw.clone(),
                });

                // Index exact capability name.
                self.provides_index
                    .entry(capability.clone())
                    .or_default()
                    .push((tid, version_str.clone()));

                // Also index variations so that fuzzy lookups hit.
                for variation in generate_capability_variations(capability) {
                    self.provides_index
                        .entry(variation)
                        .or_default()
                        .push((tid, version_str.clone()));
                }
            }
        }

        // 3. Load UNFILTERED dependencies for each installed solvable.
        //    Same logic as `load_installed_packages` but WITHOUT the
        //    `.filter(|d| !ProvideEntry::is_virtual_provide(...))`.
        //    Batch-load all deps in one query instead of N per-solvable queries.
        let removal_trove_ids: Vec<i64> =
            self.solvables.iter().filter_map(|s| s.trove_id).collect();
        let all_removal_deps = DependencyEntry::find_by_troves(self.conn, &removal_trove_ids)?;

        for (idx, solvable) in self.solvables.iter().enumerate() {
            let Some(tid) = solvable.trove_id else {
                continue;
            };
            let effective_scheme = match &solvable.version {
                ConaryPackageVersion::Installed(_) => VersionScheme::Rpm,
                ConaryPackageVersion::InstalledNative { scheme, .. } => *scheme,
                ConaryPackageVersion::Repository { scheme, .. } => {
                    scheme.unwrap_or(VersionScheme::Rpm)
                }
            };

            let empty = Vec::new();
            let deps = all_removal_deps.get(&tid).unwrap_or(&empty);
            let dep_list: Vec<SolverDep> = deps
                .iter()
                .map(|d| dep_entry_to_solver_dep(d, effective_scheme))
                .collect();

            let sid_index = u32::try_from(idx).expect("resolver solvable pool overflow");
            self.removal_deps.insert(sid_index, dep_list);
        }

        Ok(())
    }

    /// Look up which troves provide a given capability.
    ///
    /// Returns `(trove_id, optional_version)` pairs.  Tries an exact match
    /// first, then falls back to `generate_capability_variations()` and
    /// deduplicates by trove id.
    pub fn find_providers(&self, capability: &str) -> Vec<(i64, Option<String>)> {
        let mut results: Vec<(i64, Option<String>)> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // Exact match.
        if let Some(providers) = self.provides_index.get(capability) {
            for &(tid, ref ver) in providers {
                if seen_ids.insert(tid) {
                    results.push((tid, ver.clone()));
                }
            }
        }

        // If no exact match, try variations of the dep name.
        if results.is_empty() {
            for variation in generate_capability_variations(capability) {
                if let Some(providers) = self.provides_index.get(&variation) {
                    for &(tid, ref ver) in providers {
                        if seen_ids.insert(tid) {
                            results.push((tid, ver.clone()));
                        }
                    }
                }
            }
        }

        results
    }

    /// Map a trove id back to its package name.
    pub fn trove_name(&self, trove_id: i64) -> Option<&str> {
        self.trove_id_to_name.get(&trove_id).map(String::as_str)
    }

    /// Get the unfiltered dependency list for a solvable (if loaded).
    ///
    /// Unlike `get_dependency_list()` this includes virtual provides such as
    /// soname deps, perl modules, etc. that the regular list strips out.
    pub fn get_removal_dependency_list(&self, id: SolvableId) -> Option<&[SolverDep]> {
        self.removal_deps.get(&id.0).map(Vec::as_slice)
    }

    /// Load canonical equivalents from the local DB.
    ///
    /// For each distro-specific name, finds all other names that map to the
    /// same canonical package. This enables cross-distro fallback: when the
    /// solver can't find `libssl3`, it can discover `openssl` as an equivalent.
    ///
    /// The index is pre-loaded as a `HashMap` for O(1) lookups -- no DB calls
    /// happen during the solver's hot path.
    pub fn load_canonical_index(&mut self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT pi1.distro_name, pi2.distro_name
             FROM package_implementations pi1
             JOIN package_implementations pi2 ON pi1.canonical_id = pi2.canonical_id
             WHERE pi1.distro_name != pi2.distro_name",
        )?;

        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let from: String = row.get(0)?;
            let to: String = row.get(1)?;
            self.canonical_equivalents.entry(from).or_default().push(to);
        }
        Ok(())
    }

    /// Find canonical equivalents for a package name.
    ///
    /// Returns all other distro-specific names that map to the same canonical
    /// package. Returns an empty slice when no mapping exists.
    pub fn canonical_equivalents(&self, name: &str) -> &[String] {
        self.canonical_equivalents
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

fn load_repo_dependency_requests(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    repo: &crate::db::models::Repository,
) -> Result<Vec<SolverDep>> {
    let repo_scheme = infer_version_scheme(repo);
    let Some(repository_package_id) = pkg.id else {
        return Ok(pkg
            .parse_dependency_requests()?
            .into_iter()
            .map(|(name, constraint)| SolverDep::Single(name, ConaryConstraint::Legacy(constraint)))
            .collect());
    };

    // Try group-based loading first for OR and conditional support
    let groups =
        RepositoryRequirementGroup::find_by_repository_package(conn, repository_package_id)?;
    if !groups.is_empty() {
        return load_grouped_dependency_requests(conn, &groups, repo_scheme);
    }

    // Fall back to flat requirement rows (legacy data or non-grouped deps)
    let rows = RepositoryRequirement::find_by_repository_package(conn, repository_package_id)?;
    if rows.is_empty() {
        return Ok(pkg
            .parse_dependency_requests()?
            .into_iter()
            .map(|(name, constraint)| SolverDep::Single(name, ConaryConstraint::Legacy(constraint)))
            .collect());
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            let (name, constraint) = row_to_constraint(row, repo_scheme);
            SolverDep::Single(name, constraint)
        })
        .collect())
}

/// Map a dependency entry to a `SolverDep` using the given version scheme.
///
/// Shared by `load_installed_packages` and `load_removal_data` to avoid
/// duplicating the scheme-aware constraint construction.
fn dep_entry_to_solver_dep(dep: &DependencyEntry, scheme: VersionScheme) -> SolverDep {
    let constraint = match (scheme, dep.version_constraint.as_deref()) {
        (VersionScheme::Rpm, Some(s)) => {
            ConaryConstraint::Legacy(VersionConstraint::parse(s).unwrap_or(VersionConstraint::Any))
        }
        (VersionScheme::Rpm, None) => ConaryConstraint::Legacy(VersionConstraint::Any),
        (native, Some(s)) => ConaryConstraint::Repository {
            scheme: native,
            constraint: parse_repo_constraint(native, s).unwrap_or(RepoVersionConstraint::Any),
            raw: Some(s.to_string()),
        },
        (native, None) => ConaryConstraint::Repository {
            scheme: native,
            constraint: RepoVersionConstraint::Any,
            raw: None,
        },
    };
    SolverDep::Single(dep.depends_on_name.clone(), constraint)
}

/// Convert a flat requirement row to (name, constraint).
fn row_to_constraint(
    row: RepositoryRequirement,
    repo_scheme: Option<VersionScheme>,
) -> (String, ConaryConstraint) {
    let raw = row.version_constraint.clone();
    let constraint = match (repo_scheme, raw.as_deref()) {
        (Some(scheme), Some(value)) => ConaryConstraint::Repository {
            scheme,
            constraint: parse_repo_constraint(scheme, value).unwrap_or(RepoVersionConstraint::Any),
            raw,
        },
        (Some(scheme), None) => ConaryConstraint::Repository {
            scheme,
            constraint: RepoVersionConstraint::Any,
            raw: None,
        },
        (None, Some(value)) => ConaryConstraint::Legacy(
            VersionConstraint::parse(value).unwrap_or(VersionConstraint::Any),
        ),
        (None, None) => ConaryConstraint::Legacy(VersionConstraint::Any),
    };
    (row.capability, constraint)
}

/// Load dependency requests using the group-based model.
///
/// Groups with `behavior = "hard"` become solver requirements.
/// Multi-clause groups produce `SolverDep::OrGroup` (Debian OR dependencies).
/// Conditional and unsupported-rich behaviors are logged and skipped.
fn load_grouped_dependency_requests(
    conn: &rusqlite::Connection,
    groups: &[RepositoryRequirementGroup],
    repo_scheme: Option<VersionScheme>,
) -> Result<Vec<SolverDep>> {
    let mut deps = Vec::new();

    for group in groups {
        // Skip non-hard dependencies (optional, build, etc. for runtime resolution)
        if group.kind != "depends" && group.kind != "pre_depends" {
            continue;
        }

        match group.behavior.as_str() {
            "conditional" | "unsupported_rich" => {
                tracing::debug!(
                    "Skipping {} dependency (behavior={}): {:?}",
                    group.kind,
                    group.behavior,
                    group.native_text,
                );
                continue;
            }
            _ => {} // "hard" -- process normally
        }

        let Some(group_id) = group.id else {
            continue;
        };
        let clauses = RepositoryRequirement::find_by_group(conn, group_id)?;

        if clauses.is_empty() {
            continue;
        }

        if clauses.len() == 1 {
            let (name, constraint) =
                row_to_constraint(clauses.into_iter().next().unwrap(), repo_scheme);
            deps.push(SolverDep::Single(name, constraint));
        } else {
            // Multi-clause: OR-group
            let alternatives: Vec<(String, ConaryConstraint)> = clauses
                .into_iter()
                .map(|clause| row_to_constraint(clause, repo_scheme))
                .collect();
            deps.push(SolverDep::OrGroup(alternatives));
        }
    }

    Ok(deps)
}

fn load_repo_provided_capabilities(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    repo: &crate::db::models::Repository,
) -> Result<Vec<(String, Option<ConaryProvidedVersion>)>> {
    let repo_scheme = infer_version_scheme(repo);
    let Some(repository_package_id) = pkg.id else {
        return Ok(parse_repo_provides(pkg, repo_scheme));
    };

    let rows = RepositoryProvide::find_by_repository_package(conn, repository_package_id)?;
    if rows.is_empty() {
        return Ok(parse_repo_provides(pkg, repo_scheme));
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            let version = match (repo_scheme, row.version) {
                (Some(scheme), Some(raw)) => {
                    Some(ConaryProvidedVersion::Repository { raw, scheme })
                }
                _ => None,
            };
            (row.capability, version)
        })
        .collect())
}

fn find_repo_package_by_id(
    conn: &rusqlite::Connection,
    repository_package_id: i64,
) -> Result<Option<RepositoryPackage>> {
    RepositoryPackage::find_by_id(conn, repository_package_id)
}

/// Escape special characters for SQL LIKE patterns.
///
/// SQLite LIKE treats `%` and `_` as wildcards. When searching for literal
/// text we must escape them (along with the escape character itself).
fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
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
    version: &'a ConaryPackageVersion,
}
impl fmt::Display for DisplaySolvable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, self.version)
    }
}

struct DisplayVersionSet<'a>(&'a ConaryConstraint);
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
        version_set_union: VersionSetUnionId,
    ) -> impl Iterator<Item = VersionSetId> {
        self.version_set_unions
            .get(version_set_union.0 as usize)
            .cloned()
            .unwrap_or_default()
            .into_iter()
    }

    fn resolve_condition(&self, _condition: ConditionId) -> Condition {
        // ConaryProvider does not use conditions; return a permissive default
        // rather than panicking if resolvo ever calls this unexpectedly.
        Condition::Requirement(VersionSetId::default())
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
        let (name_id, ref constraint) = self.version_sets[version_set.0 as usize];
        let requested_name = &self.names[name_id.0 as usize];
        candidates
            .iter()
            .copied()
            .filter(|&sid| {
                let pkg = &self.solvables[sid.0 as usize];
                let matches = if pkg.name == *requested_name {
                    constraint_matches_package(constraint, &pkg.version)
                } else if let Some(provided_version) = pkg
                    .provided_capabilities
                    .iter()
                    .find(|(capability, _version)| capability == requested_name)
                    .and_then(|(_capability, version)| version.as_ref())
                {
                    constraint_matches_provide(constraint, Some(provided_version), &pkg.version)
                } else {
                    constraint_matches_provide(constraint, None, &pkg.version)
                };
                if inverse { !matches } else { matches }
            })
            .collect()
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        let mut candidates = self.solvables_for_name(name);

        if candidates.is_empty() {
            let name_str = &self.names[name.0 as usize];
            let providers = self.resolve_virtual_provide(name_str);
            for provider_name in &providers {
                if let Some(&provider_name_id) = self.name_to_id.get(provider_name) {
                    candidates.extend(self.solvables_for_name(provider_name_id));
                }
            }
            candidates.extend(self.solvables_for_provide(name_str));

            // Canonical fallback: check cross-distro equivalents when all
            // other lookup strategies fail. E.g. 'libssl3' -> 'openssl'.
            if candidates.is_empty() {
                for equiv in self.canonical_equivalents(name_str) {
                    if let Some(&equiv_name_id) = self.name_to_id.get(equiv) {
                        let equiv_candidates = self.solvables_for_name(equiv_name_id);
                        if !equiv_candidates.is_empty() {
                            tracing::debug!("Canonical fallback: {} -> {}", name_str, equiv);
                            candidates.extend(equiv_candidates);
                            break;
                        }
                    }
                }
            }

            if candidates.is_empty() {
                return None;
            }
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
        solvables.sort_by(|a, b| {
            let pkg_a = &self.solvables[a.0 as usize];
            let pkg_b = &self.solvables[b.0 as usize];

            if let Some(version_cmp) = compare_package_versions_desc(&pkg_a.version, &pkg_b.version)
                && version_cmp != std::cmp::Ordering::Equal
            {
                return version_cmp;
            }

            let a_installed = pkg_a.trove_id.is_some();
            let b_installed = pkg_b.trove_id.is_some();
            b_installed
                .cmp(&a_installed)
                .then_with(|| pkg_a.name.cmp(&pkg_b.name))
        });
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let mut requirements = Vec::new();

        if let Some(dep_list) = self.dependencies.get(&solvable.0) {
            for dep in dep_list {
                match dep {
                    SolverDep::Single(dep_name, constraint) => {
                        if let Some(req) = self.lookup_requirement(dep_name, constraint) {
                            requirements.push(req);
                        }
                    }
                    SolverDep::OrGroup(alternatives) => {
                        let mut vs_ids = Vec::new();
                        for (dep_name, constraint) in alternatives {
                            if let Some(&dep_name_id) = self.name_to_id.get(dep_name) {
                                let cache_key = (dep_name_id.0, constraint.clone());
                                if let Some(&vs_id) = self.version_set_cache.get(&cache_key) {
                                    vs_ids.push(vs_id);
                                }
                            }
                        }
                        if vs_ids.len() == 1 {
                            // Single-alternative OR group: emit as a simple requirement
                            requirements.push(ConditionalRequirement::from(vs_ids[0]));
                        } else if vs_ids.len() > 1 {
                            // Look up the union ID from our pool
                            if let Some(union_id) = self.find_union_id(&vs_ids) {
                                requirements.push(ConditionalRequirement::from(union_id));
                            }
                        }
                    }
                }
            }
        }

        Dependencies::Known(KnownDependencies {
            requirements,
            constrains: Vec::new(),
        })
    }
}

impl fmt::Display for ConaryPackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed(version) => write!(f, "{}", version),
            Self::InstalledNative { raw, .. } => write!(f, "{}", raw),
            Self::Repository { raw, .. } => write!(f, "{}", raw),
        }
    }
}

impl fmt::Display for ConaryConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(constraint) => write!(f, "{}", constraint),
            Self::Repository {
                raw, constraint, ..
            } => {
                if let Some(raw) = raw {
                    write!(f, "{}", raw)
                } else {
                    write!(f, "{:?}", constraint)
                }
            }
        }
    }
}

fn compare_package_versions_desc(
    a: &ConaryPackageVersion,
    b: &ConaryPackageVersion,
) -> Option<std::cmp::Ordering> {
    let ord = match (a, b) {
        (ConaryPackageVersion::Installed(a), ConaryPackageVersion::Installed(b)) => Some(b.cmp(a)),
        (
            ConaryPackageVersion::InstalledNative {
                raw: a_raw,
                scheme: a_scheme,
            },
            ConaryPackageVersion::InstalledNative {
                raw: b_raw,
                scheme: b_scheme,
            },
        ) if a_scheme == b_scheme => compare_repo_versions(*a_scheme, b_raw, a_raw),
        (
            ConaryPackageVersion::Repository {
                raw: a_raw,
                scheme: Some(a_scheme),
            },
            ConaryPackageVersion::Repository {
                raw: b_raw,
                scheme: Some(b_scheme),
            },
        ) if a_scheme == b_scheme => compare_repo_versions(*a_scheme, b_raw, a_raw),
        _ => None,
    }?;
    Some(ord)
}

pub(crate) fn constraint_matches_package(
    constraint: &ConaryConstraint,
    version: &ConaryPackageVersion,
) -> bool {
    match (constraint, version) {
        // Legacy constraint vs RPM installed
        (ConaryConstraint::Legacy(constraint), ConaryPackageVersion::Installed(version)) => {
            constraint.satisfies(version)
        }
        // Legacy `Any` matches anything
        (
            ConaryConstraint::Legacy(VersionConstraint::Any),
            ConaryPackageVersion::Repository { .. } | ConaryPackageVersion::InstalledNative { .. },
        ) => true,
        // Legacy constraint vs RPM repo package
        (
            ConaryConstraint::Legacy(constraint),
            ConaryPackageVersion::Repository {
                raw,
                scheme: Some(VersionScheme::Rpm),
            },
        ) => RpmVersion::parse(raw)
            .map(|version| constraint.satisfies(&version))
            .unwrap_or(false),
        // Native RPM constraint vs legacy RPM installed
        (
            ConaryConstraint::Repository {
                scheme: VersionScheme::Rpm,
                constraint,
                ..
            },
            ConaryPackageVersion::Installed(version),
        ) => {
            let legacy = repo_constraint_to_legacy(constraint);
            legacy.satisfies(version)
        }
        // Native constraint vs installed native (same scheme)
        (
            ConaryConstraint::Repository {
                scheme, constraint, ..
            },
            ConaryPackageVersion::InstalledNative {
                raw,
                scheme: installed_scheme,
            },
        ) if scheme == installed_scheme => repo_version_satisfies(*scheme, raw, constraint),
        // Native constraint vs repo package (same scheme)
        (
            ConaryConstraint::Repository {
                scheme, constraint, ..
            },
            ConaryPackageVersion::Repository {
                raw,
                scheme: Some(version_scheme),
            },
        ) if scheme == version_scheme => repo_version_satisfies(*scheme, raw, constraint),
        // Native `Any` constraint matches any repo version
        (
            ConaryConstraint::Repository {
                constraint: RepoVersionConstraint::Any,
                ..
            },
            ConaryPackageVersion::Repository { .. } | ConaryPackageVersion::InstalledNative { .. },
        ) => true,
        _ => false,
    }
}

fn constraint_matches_provide(
    constraint: &ConaryConstraint,
    provided_version: Option<&ConaryProvidedVersion>,
    package_version: &ConaryPackageVersion,
) -> bool {
    if let Some(version) = provided_version {
        match (constraint, version) {
            (ConaryConstraint::Legacy(constraint), ConaryProvidedVersion::Installed(version)) => {
                return constraint.satisfies(version);
            }
            (
                ConaryConstraint::Legacy(VersionConstraint::Any),
                ConaryProvidedVersion::Repository { .. },
            ) => return true,
            (
                ConaryConstraint::Legacy(constraint),
                ConaryProvidedVersion::Repository {
                    raw,
                    scheme: VersionScheme::Rpm,
                },
            ) => {
                return RpmVersion::parse(raw)
                    .map(|version| constraint.satisfies(&version))
                    .unwrap_or(false);
            }
            (
                ConaryConstraint::Repository {
                    scheme, constraint, ..
                },
                ConaryProvidedVersion::Repository {
                    raw,
                    scheme: provided_scheme,
                },
            ) if scheme == provided_scheme => {
                return repo_version_satisfies(*scheme, raw, constraint);
            }
            (
                ConaryConstraint::Repository {
                    constraint: RepoVersionConstraint::Any,
                    ..
                },
                ConaryProvidedVersion::Repository { .. },
            ) => return true,
            _ => {}
        }
    }

    constraint_matches_package(constraint, package_version)
}

/// Convert a `RepoVersionConstraint` to a legacy `VersionConstraint` for RPM
/// cross-format matching (repo RPM constraint vs installed RPM `RpmVersion`).
fn repo_constraint_to_legacy(constraint: &RepoVersionConstraint) -> VersionConstraint {
    match constraint {
        RepoVersionConstraint::Any => VersionConstraint::Any,
        RepoVersionConstraint::Exact(v) => {
            VersionConstraint::parse(&format!("= {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::GreaterThan(v) => {
            VersionConstraint::parse(&format!("> {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::GreaterOrEqual(v) => {
            VersionConstraint::parse(&format!(">= {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::LessThan(v) => {
            VersionConstraint::parse(&format!("< {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::LessOrEqual(v) => {
            VersionConstraint::parse(&format!("<= {v}")).unwrap_or(VersionConstraint::Any)
        }
        RepoVersionConstraint::NotEqual(v) => {
            VersionConstraint::parse(&format!("!= {v}")).unwrap_or(VersionConstraint::Any)
        }
    }
}

fn parse_stored_version_scheme(raw: Option<&str>) -> Option<VersionScheme> {
    match raw? {
        "rpm" => Some(VersionScheme::Rpm),
        "debian" => Some(VersionScheme::Debian),
        "arch" => Some(VersionScheme::Arch),
        _ => None,
    }
}

fn parse_repo_provides(
    pkg: &RepositoryPackage,
    scheme: Option<VersionScheme>,
) -> Vec<(String, Option<ConaryProvidedVersion>)> {
    let Some(metadata_json) = pkg.metadata.as_deref() else {
        return Vec::new();
    };
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return Vec::new();
    };
    let Some(provides) = metadata
        .get("rpm_provides")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };

    provides
        .iter()
        .filter_map(|value| value.as_str())
        .map(|entry| parse_provide_entry(entry, scheme))
        .collect()
}

fn parse_provide_entry(
    entry: &str,
    scheme: Option<VersionScheme>,
) -> (String, Option<ConaryProvidedVersion>) {
    const OPS: [&str; 5] = ["<=", ">=", "=", "<", ">"];

    for op in OPS {
        if let Some((name, version)) = entry.split_once(op) {
            let name = name.trim();
            let version = version.trim();
            if name.is_empty() || version.is_empty() {
                continue;
            }
            let parsed = match scheme {
                Some(VersionScheme::Rpm) => RpmVersion::parse(version)
                    .ok()
                    .map(ConaryProvidedVersion::Installed),
                Some(scheme) => Some(ConaryProvidedVersion::Repository {
                    raw: version.to_string(),
                    scheme,
                }),
                None => None,
            };
            return (name.to_string(), parsed);
        }
    }

    (entry.trim().to_string(), None)
}

fn repo_package_provides_capability(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    capability: &str,
) -> Result<bool> {
    let Some(repo) = crate::db::models::Repository::find_by_id(conn, pkg.repository_id)? else {
        return Ok(false);
    };
    Ok(load_repo_provided_capabilities(conn, pkg, &repo)?
        .iter()
        .any(|(provided, _version)| provided == capability))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::{
        Repository, RepositoryPackage, RepositoryProvide, RepositoryRequirement,
    };

    fn setup_test_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        db::init(&db_path).unwrap();
        let conn = db::open(&db_path).unwrap();
        (temp_dir, conn)
    }

    #[test]
    fn load_repo_packages_preserves_dependency_constraints() {
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "kernel".to_string(),
            "6.19.6-200.fc43".to_string(),
            "sha256:deadbeef".to_string(),
            1,
            "https://example.invalid/kernel.rpm".to_string(),
        );
        pkg.dependencies = Some(
            serde_json::to_string(&vec![
                "kernel-core-uname-r = 6.19.6-200.fc43.x86_64".to_string(),
                "coreutils >= 9.7".to_string(),
            ])
            .unwrap(),
        );
        pkg.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider
            .load_repo_packages_for_names(&["kernel".to_string()])
            .unwrap();

        let deps = provider
            .dependencies
            .values()
            .find(|deps| {
                deps.iter()
                    .any(|d| d.single_name() == Some("kernel-core-uname-r"))
            })
            .cloned()
            .unwrap();

        assert!(deps.iter().any(|d| {
            d.as_single().is_some_and(|(name, constraint)| {
                name == "kernel-core-uname-r"
                    && *constraint
                        == ConaryConstraint::Legacy(
                            VersionConstraint::parse("= 6.19.6-200.fc43.x86_64").unwrap(),
                        )
            })
        }));
        assert!(deps.iter().any(|d| {
            d.as_single().is_some_and(|(name, constraint)| {
                name == "coreutils"
                    && *constraint
                        == ConaryConstraint::Legacy(VersionConstraint::parse(">= 9.7").unwrap())
            })
        }));
    }

    #[test]
    fn load_repo_packages_uses_normalized_repository_requirements() {
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new("arch-core".to_string(), "https://example.invalid".into());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "ripgrep".to_string(),
            "14.1.0-1".to_string(),
            "sha256:deadbeef".to_string(),
            1,
            "https://example.invalid/ripgrep.pkg.tar.zst".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let repo_package_id = pkg.id.unwrap();

        let mut requirement = RepositoryRequirement::new(
            repo_package_id,
            "glibc".to_string(),
            Some(">= 2.39".to_string()),
            "package".to_string(),
            "runtime".to_string(),
            Some("glibc >= 2.39".to_string()),
        );
        requirement.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider
            .load_repo_packages_for_names(&["ripgrep".to_string()])
            .unwrap();

        let deps = provider
            .dependencies
            .values()
            .find(|deps| deps.iter().any(|d| d.single_name() == Some("glibc")))
            .cloned()
            .unwrap();

        assert_eq!(deps.len(), 1);
        let (name, constraint) = deps[0].as_single().expect("expected Single dep");
        assert_eq!(name, "glibc");
        assert_eq!(
            *constraint,
            ConaryConstraint::Repository {
                scheme: VersionScheme::Arch,
                constraint: RepoVersionConstraint::GreaterOrEqual("2.39".to_string()),
                raw: Some(">= 2.39".to_string()),
            }
        );
    }

    #[test]
    fn load_repo_packages_uses_normalized_repository_provides() {
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "kernel-core".to_string(),
            "6.19.6-200.fc43".to_string(),
            "sha256:deadbeef".to_string(),
            1,
            "https://example.invalid/kernel-core.rpm".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let repo_package_id = pkg.id.unwrap();

        let mut provide = RepositoryProvide::new(
            repo_package_id,
            "kernel-core-uname-r".to_string(),
            Some("6.19.6-200.fc43.x86_64".to_string()),
            "package".to_string(),
            Some("kernel-core-uname-r = 6.19.6-200.fc43.x86_64".to_string()),
        );
        provide.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider
            .load_repo_packages_for_names(&["kernel-core".to_string()])
            .unwrap();

        let loaded = provider
            .solvables
            .iter()
            .find(|pkg| pkg.name == "kernel-core" && pkg.repo_package_id == Some(repo_package_id))
            .unwrap();

        assert!(loaded.provided_capabilities.iter().any(|(name, version)| {
            name == "kernel-core-uname-r"
                && *version
                    == Some(ConaryProvidedVersion::Repository {
                        raw: "6.19.6-200.fc43.x86_64".to_string(),
                        scheme: VersionScheme::Rpm,
                    })
        }));
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
            version: ConaryPackageVersion::Installed(RpmVersion::parse("1.0.0").unwrap()),
            trove_id: None,
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });
        let s2 = provider.add_solvable(ConaryPackage {
            name: "lib".to_string(),
            version: ConaryPackageVersion::Installed(RpmVersion::parse("2.0.0").unwrap()),
            trove_id: None,
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });
        let s3 = provider.add_solvable(ConaryPackage {
            name: "lib".to_string(),
            version: ConaryPackageVersion::Installed(RpmVersion::parse("3.0.0").unwrap()),
            trove_id: None,
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });

        let candidates = [s1, s2, s3];

        // Test the filtering logic directly
        let (_, ref constraint) = provider.version_sets[vs_id.0 as usize];
        let matching: Vec<SolvableId> = candidates
            .iter()
            .copied()
            .filter(|&sid| {
                constraint_matches_package(constraint, &provider.solvables[sid.0 as usize].version)
            })
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
            version: ConaryPackageVersion::Installed(RpmVersion::parse("1.0.0").unwrap()),
            trove_id: Some(42),
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });

        // Add a repo version
        let _repo = provider.add_solvable(ConaryPackage {
            name: "nginx".to_string(),
            version: ConaryPackageVersion::Installed(RpmVersion::parse("2.0.0").unwrap()),
            trove_id: None,
            repo_package_id: Some(100),
            provided_capabilities: Vec::new(),
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
            version: ConaryPackageVersion::Installed(RpmVersion::parse("1.0.0").unwrap()),
            trove_id: None,
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });
        let s2 = provider.add_solvable(ConaryPackage {
            name: "pkg".to_string(),
            version: ConaryPackageVersion::Installed(RpmVersion::parse("3.0.0").unwrap()),
            trove_id: None,
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });
        let s3 = provider.add_solvable(ConaryPackage {
            name: "pkg".to_string(),
            version: ConaryPackageVersion::Installed(RpmVersion::parse("2.0.0").unwrap()),
            trove_id: Some(1), // installed
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });

        // Test sort logic directly
        let mut solvables = [s1, s2, s3];
        solvables.sort_by(|a, b| {
            let pkg_a = &provider.solvables[a.0 as usize];
            let pkg_b = &provider.solvables[b.0 as usize];
            if let Some(version_cmp) = compare_package_versions_desc(&pkg_a.version, &pkg_b.version)
                && version_cmp != std::cmp::Ordering::Equal
            {
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
            version: ConaryPackageVersion::Installed(RpmVersion::parse("1.24.0").unwrap()),
            trove_id: None,
            repo_package_id: None,
            provided_capabilities: Vec::new(),
        });
        let str_id = provider.intern_string("test string");

        assert_eq!(provider.display_name(name_id).to_string(), "nginx");
        assert_eq!(provider.display_solvable(sid).to_string(), "nginx=1.24.0");
        assert_eq!(provider.display_version_set(vs_id).to_string(), ">= 1.0.0");
        assert_eq!(provider.display_string(str_id).to_string(), "test string");
        assert_eq!(provider.version_set_name(vs_id), name_id);
        assert_eq!(provider.solvable_name(sid), name_id);
    }

    #[test]
    fn filter_candidates_uses_provide_version_for_virtual_capabilities() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let capability_name = provider.intern_name("kernel-modules-core-uname-r");
        let version_set = provider.intern_version_set(
            capability_name,
            VersionConstraint::parse("= 6.19.6").unwrap(),
        );
        let candidate = provider.add_solvable(ConaryPackage {
            name: "kernel-modules-core".to_string(),
            version: ConaryPackageVersion::Installed(RpmVersion::parse("6.19.6-200.fc43").unwrap()),
            trove_id: None,
            repo_package_id: Some(42),
            provided_capabilities: vec![(
                "kernel-modules-core-uname-r".to_string(),
                Some(ConaryProvidedVersion::Installed(
                    RpmVersion::parse("6.19.6").unwrap(),
                )),
            )],
        });

        let requested_name = &provider.names[capability_name.0 as usize];
        let (_, constraint) = &provider.version_sets[version_set.0 as usize];
        let matching: Vec<SolvableId> = [candidate]
            .into_iter()
            .filter(|sid| {
                let pkg = &provider.solvables[sid.0 as usize];
                let Some(provided_version) = pkg
                    .provided_capabilities
                    .iter()
                    .find(|(capability, _version)| capability == requested_name)
                    .and_then(|(_capability, version)| version.as_ref())
                else {
                    return false;
                };
                constraint_matches_provide(constraint, Some(provided_version), &pkg.version)
            })
            .collect();
        assert_eq!(matching, vec![candidate]);
    }

    #[test]
    fn or_group_loading_from_requirement_groups() {
        // Debian OR dependency: default-mta | mail-transport-agent
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new("ubuntu-main".to_string(), "https://example.invalid".into());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "bsd-mailx".to_string(),
            "8.1.2-0.20220stringe-0ubuntu1".to_string(),
            "sha256:mailx".to_string(),
            1,
            "https://example.invalid/bsd-mailx.deb".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        // Create a group for the OR dependency
        let mut group =
            RepositoryRequirementGroup::new(pkg_id, "depends".to_string(), "hard".to_string());
        group.native_text = Some("default-mta | mail-transport-agent".to_string());
        group.insert(&conn).unwrap();
        let group_id = group.id.unwrap();

        // Insert both alternatives
        let mut clause_a = RepositoryRequirement::new(
            pkg_id,
            "default-mta".to_string(),
            None,
            "package".to_string(),
            "runtime".to_string(),
            None,
        )
        .with_group(group_id);
        clause_a.insert(&conn).unwrap();

        let mut clause_b = RepositoryRequirement::new(
            pkg_id,
            "mail-transport-agent".to_string(),
            None,
            "package".to_string(),
            "runtime".to_string(),
            None,
        )
        .with_group(group_id);
        clause_b.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider
            .load_repo_packages_for_names(&["bsd-mailx".to_string()])
            .unwrap();

        // Find the dependency list for bsd-mailx
        let deps = provider.dependencies.values().next().cloned().unwrap();
        assert_eq!(deps.len(), 1, "should have exactly one (OR) dependency");

        match &deps[0] {
            SolverDep::OrGroup(alts) => {
                assert_eq!(alts.len(), 2);
                assert_eq!(alts[0].0, "default-mta");
                assert_eq!(alts[1].0, "mail-transport-agent");
            }
            SolverDep::Single(..) => panic!("expected OrGroup, got Single"),
        }
    }

    #[test]
    fn conditional_deps_skipped_with_diagnostic() {
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new("fedora-main".to_string(), "https://example.invalid".into());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "systemd".to_string(),
            "256-1.fc43".to_string(),
            "sha256:systemd".to_string(),
            1,
            "https://example.invalid/systemd.rpm".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        // A hard dependency
        let mut hard_group =
            RepositoryRequirementGroup::new(pkg_id, "depends".to_string(), "hard".to_string());
        hard_group.insert(&conn).unwrap();
        let hard_group_id = hard_group.id.unwrap();

        let mut hard_req = RepositoryRequirement::new(
            pkg_id,
            "glibc".to_string(),
            Some(">= 2.39".to_string()),
            "package".to_string(),
            "runtime".to_string(),
            None,
        )
        .with_group(hard_group_id);
        hard_req.insert(&conn).unwrap();

        // A conditional dependency (should be skipped)
        let mut cond_group = RepositoryRequirementGroup::new(
            pkg_id,
            "depends".to_string(),
            "conditional".to_string(),
        );
        cond_group.native_text = Some("(systemd-boot if efi-filesystem)".to_string());
        cond_group.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider
            .load_repo_packages_for_names(&["systemd".to_string()])
            .unwrap();

        // Only the hard dependency should appear
        let deps = provider.dependencies.values().next().cloned().unwrap();
        assert_eq!(deps.len(), 1);
        let (name, _) = deps[0].as_single().expect("expected Single dep");
        assert_eq!(name, "glibc");
    }

    #[test]
    fn debian_versioned_provide_uses_provide_version_not_package_version() {
        // Package libc6 version 2.39-0ubuntu2 provides libc6 (= 2.39-0ubuntu2)
        // A dep on libc6 (>= 2.38) should match via the provide version, not
        // attempt to parse "2.39-0ubuntu2" as RPM.
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new("ubuntu-main".to_string(), "https://example.invalid".into());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "libc6".to_string(),
            "2.39-0ubuntu2".to_string(),
            "sha256:libc6".to_string(),
            1,
            "https://example.invalid/libc6.deb".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        let mut provide = RepositoryProvide::new(
            pkg_id,
            "libc6".to_string(),
            Some("2.39-0ubuntu2".to_string()),
            "package".to_string(),
            Some("libc6 (= 2.39-0ubuntu2)".to_string()),
        );
        provide.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider
            .load_repo_packages_for_names(&["libc6".to_string()])
            .unwrap();

        let pkg_solvable = provider
            .solvables
            .iter()
            .find(|s| s.name == "libc6" && s.repo_package_id.is_some())
            .unwrap();

        // The provide version should use the Debian scheme
        let (_, provide_version) = pkg_solvable
            .provided_capabilities
            .iter()
            .find(|(cap, _)| cap == "libc6")
            .unwrap();
        assert_eq!(
            *provide_version,
            Some(ConaryProvidedVersion::Repository {
                raw: "2.39-0ubuntu2".to_string(),
                scheme: VersionScheme::Debian,
            })
        );
    }

    #[test]
    fn arch_versioned_provide_uses_native_scheme() {
        let (_dir, conn) = setup_test_db();

        let mut repo = Repository::new("arch-core".to_string(), "https://example.invalid".into());
        let repo_id = repo.insert(&conn).unwrap();

        let mut pkg = RepositoryPackage::new(
            repo_id,
            "sh".to_string(),
            "5.2.037-1".to_string(),
            "sha256:sh".to_string(),
            1,
            "https://example.invalid/sh.pkg.tar.zst".to_string(),
        );
        pkg.insert(&conn).unwrap();
        let pkg_id = pkg.id.unwrap();

        let mut provide = RepositoryProvide::new(
            pkg_id,
            "sh".to_string(),
            Some("5.2.037".to_string()),
            "package".to_string(),
            Some("sh=5.2.037".to_string()),
        );
        provide.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider
            .load_repo_packages_for_names(&["sh".to_string()])
            .unwrap();

        let pkg_solvable = provider
            .solvables
            .iter()
            .find(|s| s.name == "sh" && s.repo_package_id.is_some())
            .unwrap();

        let (_, provide_version) = pkg_solvable
            .provided_capabilities
            .iter()
            .find(|(cap, _)| cap == "sh")
            .unwrap();
        assert_eq!(
            *provide_version,
            Some(ConaryProvidedVersion::Repository {
                raw: "5.2.037".to_string(),
                scheme: VersionScheme::Arch,
            })
        );
    }

    #[test]
    fn installed_debian_package_uses_native_version_scheme() {
        use crate::db::models::{Changeset, ChangesetStatus, DependencyEntry, TroveType};

        let (_dir, conn) = setup_test_db();

        let mut changeset = Changeset::new("Install libc6".to_string());
        changeset.insert(&conn).unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();

        let mut trove = Trove::new(
            "libc6".to_string(),
            "2.39-0ubuntu2".to_string(),
            TroveType::Package,
        );
        trove.version_scheme = Some("debian".to_string());
        trove.source_distro = Some("ubuntu".to_string());
        let trove_id = trove.insert(&conn).unwrap();

        let mut dep = DependencyEntry::new(
            trove_id,
            "libgcc-s1".to_string(),
            None,
            "runtime".to_string(),
            Some(">= 3.0".to_string()),
        );
        dep.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider.load_installed_packages().unwrap();

        // Version should be InstalledNative with Debian scheme
        let solvable = provider
            .solvables
            .iter()
            .find(|s| s.name == "libc6")
            .unwrap();
        assert_eq!(
            solvable.version,
            ConaryPackageVersion::InstalledNative {
                raw: "2.39-0ubuntu2".to_string(),
                scheme: VersionScheme::Debian,
            }
        );

        // Dependencies should use Debian constraint scheme
        let deps = provider.dependencies.values().next().unwrap();
        let (name, constraint) = deps[0].as_single().unwrap();
        assert_eq!(name, "libgcc-s1");
        assert_eq!(
            *constraint,
            ConaryConstraint::Repository {
                scheme: VersionScheme::Debian,
                constraint: RepoVersionConstraint::GreaterOrEqual("3.0".to_string()),
                raw: Some(">= 3.0".to_string()),
            }
        );
    }

    #[test]
    fn installed_arch_package_in_provider_selection() {
        use crate::db::models::{Changeset, ChangesetStatus, TroveType};

        let (_dir, conn) = setup_test_db();

        let mut changeset = Changeset::new("Install glibc".to_string());
        changeset.insert(&conn).unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();

        let mut trove = Trove::new(
            "glibc".to_string(),
            "2.39-1".to_string(),
            TroveType::Package,
        );
        trove.version_scheme = Some("arch".to_string());
        trove.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider.load_installed_packages().unwrap();

        let solvable = provider
            .solvables
            .iter()
            .find(|s| s.name == "glibc")
            .unwrap();
        assert_eq!(
            solvable.version,
            ConaryPackageVersion::InstalledNative {
                raw: "2.39-1".to_string(),
                scheme: VersionScheme::Arch,
            }
        );

        // Arch constraint should match the installed version
        let constraint = ConaryConstraint::Repository {
            scheme: VersionScheme::Arch,
            constraint: RepoVersionConstraint::GreaterOrEqual("2.39".to_string()),
            raw: Some(">= 2.39".to_string()),
        };
        assert!(constraint_matches_package(&constraint, &solvable.version));
    }

    #[test]
    fn legacy_rpm_fallback_for_untyped_installed_troves() {
        use crate::db::models::{Changeset, ChangesetStatus, TroveType};

        let (_dir, conn) = setup_test_db();

        let mut changeset = Changeset::new("Install bash".to_string());
        changeset.insert(&conn).unwrap();
        changeset
            .update_status(&conn, ChangesetStatus::Applied)
            .unwrap();

        // Legacy trove: no version_scheme set
        let mut trove = Trove::new(
            "bash".to_string(),
            "5.2.21-2.fc43".to_string(),
            TroveType::Package,
        );
        trove.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider.load_installed_packages().unwrap();

        let solvable = provider
            .solvables
            .iter()
            .find(|s| s.name == "bash")
            .unwrap();

        // Should be Installed(RpmVersion), not InstalledNative
        assert!(matches!(
            solvable.version,
            ConaryPackageVersion::Installed(_)
        ));

        // Legacy VersionConstraint should still work
        let constraint = ConaryConstraint::Legacy(VersionConstraint::parse(">= 5.2.0").unwrap());
        assert!(constraint_matches_package(&constraint, &solvable.version));
    }

    #[test]
    fn canonical_index_loads_cross_distro_equivalents() {
        use crate::db::models::{CanonicalPackage, PackageImplementation};

        let (_dir, conn) = setup_test_db();

        // Create a canonical package with three distro implementations
        let mut pkg = CanonicalPackage::new("openssl".to_string(), "package".to_string());
        let can_id = pkg.insert(&conn).unwrap();

        let mut impl_fed = PackageImplementation::new(
            can_id,
            "fedora".to_string(),
            "openssl".to_string(),
            "auto".to_string(),
        );
        impl_fed.insert(&conn).unwrap();

        let mut impl_deb = PackageImplementation::new(
            can_id,
            "debian".to_string(),
            "libssl3".to_string(),
            "auto".to_string(),
        );
        impl_deb.insert(&conn).unwrap();

        let mut impl_arch = PackageImplementation::new(
            can_id,
            "arch".to_string(),
            "openssl".to_string(),
            "auto".to_string(),
        );
        impl_arch.insert(&conn).unwrap();

        let mut provider = ConaryProvider::new(&conn);
        provider.load_canonical_index().unwrap();

        // "libssl3" should map to "openssl" (both fedora and arch share the name)
        let equivs = provider.canonical_equivalents("libssl3");
        assert!(
            equivs.contains(&"openssl".to_string()),
            "libssl3 should have openssl as equivalent, got: {equivs:?}"
        );

        // "openssl" should map to "libssl3"
        let equivs = provider.canonical_equivalents("openssl");
        assert!(
            equivs.contains(&"libssl3".to_string()),
            "openssl should have libssl3 as equivalent, got: {equivs:?}"
        );

        // Unknown name returns empty
        assert!(provider.canonical_equivalents("nonexistent").is_empty());
    }

    #[test]
    fn canonical_index_empty_when_no_mappings() {
        let (_dir, conn) = setup_test_db();

        let mut provider = ConaryProvider::new(&conn);
        provider.load_canonical_index().unwrap();

        assert!(provider.canonical_equivalents("anything").is_empty());
    }
}
