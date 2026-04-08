// conary-core/src/resolver/provider/mod.rs

//! Bridge between Conary's data model and resolvo's SAT solver.
//!
//! Implements resolvo's `DependencyProvider` and `Interner` traits via
//! `ConaryProvider`, which maps Conary packages (troves + repo packages)
//! to resolvo's abstract solvable/name/version-set model.
//!
//! Solvables are `PackageIdentity` instances carrying full provenance.

mod loading;
pub(crate) mod matching;
mod traits;
pub mod types;

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use resolvo::{
    ConditionalRequirement, NameId, SolvableId, StringId, VersionSetId, VersionSetUnionId,
};
use tracing::error;

use crate::db::models::{
    DependencyEntry, ProvideEntry, RepologyCacheEntry, RepositoryProvide, Trove,
    generate_capability_variations,
};
use crate::error::{Error, Result};
use crate::repository::LatestSignal;
use crate::repository::resolution_policy::{ResolutionPolicy, SelectionMode};
use crate::repository::versioning::VersionScheme;
use crate::resolver::identity::PackageIdentity;
use crate::resolver::provides_index::ProvidesIndex;

use loading::{
    dep_entry_to_solver_dep, escape_like, find_repo_package_by_id, load_repo_dependency_requests,
    load_repo_provided_capabilities, parse_stored_version_scheme, repo_package_provides_capability,
};
pub(crate) use matching::constraint_matches_package;
pub use types::{ConaryConstraint, SolverDep};

/// Bridge between Conary's data model and resolvo's abstract solver interface.
///
/// Owns interning pools for names, solvables, version sets, and strings,
/// and queries Conary's DB on demand to feed the solver.
pub struct ConaryProvider<'db> {
    // --- Interning pools ---
    pub(super) names: Vec<String>,
    pub(super) name_to_id: HashMap<String, NameId>,

    pub(super) solvables: Vec<PackageIdentity>,

    /// Each version set is (name_id, constraint).
    pub(super) version_sets: Vec<(NameId, ConaryConstraint)>,

    /// Cache for deduplicating version sets by (name_id, constraint).
    pub(super) version_set_cache: HashMap<(u32, ConaryConstraint), VersionSetId>,

    /// Each version set union is a list of `VersionSetId`s (OR alternatives).
    pub(super) version_set_unions: Vec<Vec<VersionSetId>>,

    /// Reverse index: package name -> indices into `solvables` vec.
    /// Enables O(1) lookup instead of linear scan when finding solvables by name.
    name_to_solvable_indices: HashMap<String, Vec<usize>>,

    /// Set of repo_package_ids already loaded as solvables.
    /// Enables O(1) duplicate check instead of linear scan.
    loaded_repo_package_ids: HashSet<i64>,

    /// Reverse index: version set ID list -> union ID.
    /// Enables O(1) lookup instead of linear scan in `find_union_id`.
    pub(super) union_id_index: HashMap<Vec<VersionSetId>, VersionSetUnionId>,

    pub(super) strings: Vec<String>,

    /// Pre-loaded dependencies for each solvable, keyed by `SolvableId` index.
    pub(super) dependencies: HashMap<u32, Vec<SolverDep>>,

    /// Index of capability name -> list of (trove_id, optional version) providers.
    /// Built by `load_removal_data()` from already-loaded solvable provides.
    removal_provides_index: HashMap<String, Vec<(i64, Option<String>)>>,

    /// Reverse map from trove_id to package name, for quick lookup during removal.
    trove_id_to_name: HashMap<i64, String>,

    /// Unfiltered dependencies for each solvable, including virtual provides
    /// (soname, perl, python, etc.) that `dependencies` strips out.
    /// Keyed by `SolvableId` index.
    removal_deps: HashMap<u32, Vec<SolverDep>>,

    /// distro_name -> Vec<equivalent distro_name> for canonical cross-distro resolution.
    /// Pre-loaded as a HashMap for O(1) lookup in the hot path.
    canonical_equivalents: HashMap<String, Vec<String>>,

    /// Pre-built capability-to-provider index (modeled after libsolv's whatprovides).
    /// Built once at resolution start via `build_provides_index()`.
    provides_index: Option<ProvidesIndex>,

    /// Source-selection policy that should influence SAT candidate ordering.
    policy: ResolutionPolicy,

    /// Cached positive latest-signal keys keyed by canonical_id + distro.
    latest_positive_keys: HashSet<(i64, String)>,

    // --- Data source ---
    pub(super) conn: &'db rusqlite::Connection,
}

impl<'db> ConaryProvider<'db> {
    /// Create a new provider backed by the given database connection.
    pub fn new(conn: &'db rusqlite::Connection) -> Self {
        Self::new_with_policy(conn, ResolutionPolicy::new())
    }

    /// Create a new provider with an explicit source-selection policy.
    pub fn new_with_policy(conn: &'db rusqlite::Connection, policy: ResolutionPolicy) -> Self {
        Self {
            names: Vec::new(),
            name_to_id: HashMap::new(),
            solvables: Vec::new(),
            version_sets: Vec::new(),
            version_set_cache: HashMap::new(),
            version_set_unions: Vec::new(),
            name_to_solvable_indices: HashMap::new(),
            loaded_repo_package_ids: HashSet::new(),
            union_id_index: HashMap::new(),
            strings: Vec::new(),
            dependencies: HashMap::new(),
            removal_provides_index: HashMap::new(),
            trove_id_to_name: HashMap::new(),
            removal_deps: HashMap::new(),
            canonical_equivalents: HashMap::new(),
            provides_index: None,
            policy,
            latest_positive_keys: HashSet::new(),
            conn,
        }
    }

    /// Convert a `usize` pool length to a `u32` index, returning
    /// `Error::PoolOverflow` if the pool exceeds `u32::MAX` entries.
    fn pool_u32(len: usize, pool_name: &str) -> Result<u32> {
        u32::try_from(len).map_err(|_| {
            crate::error::Error::PoolOverflow(format!(
                "{pool_name} pool exceeds u32::MAX entries ({len})"
            ))
        })
    }

    fn solvable_id_from_index(&self, index: usize, context: &str) -> Option<SolvableId> {
        match u32::try_from(index) {
            Ok(index) => Some(SolvableId(index)),
            Err(_) => {
                error!("resolver solvable pool overflow while {context}: index={index}");
                None
            }
        }
    }

    fn removal_deps_index(&self, index: usize) -> Result<u32> {
        u32::try_from(index).map_err(|_| {
            Error::ResolutionError(format!(
                "resolver solvable pool overflow while indexing removal dependencies: {index}"
            ))
        })
    }

    /// Intern a package name, returning its `NameId`.
    pub fn intern_name(&mut self, name: &str) -> Result<NameId> {
        if let Some(&id) = self.name_to_id.get(name) {
            return Ok(id);
        }
        let id = NameId(Self::pool_u32(self.names.len(), "name")?);
        let owned = name.to_string();
        self.names.push(owned.clone());
        self.name_to_id.insert(owned, id);
        Ok(id)
    }

    /// Intern a version constraint for a given name, deduplicating via cache.
    pub fn intern_version_set(
        &mut self,
        name_id: NameId,
        constraint: crate::version::VersionConstraint,
    ) -> Result<VersionSetId> {
        let constraint = ConaryConstraint::Legacy(constraint);
        let cache_key = (name_id.0, constraint.clone());
        if let Some(&existing) = self.version_set_cache.get(&cache_key) {
            return Ok(existing);
        }
        let id = VersionSetId(Self::pool_u32(self.version_sets.len(), "version_set")?);
        self.version_sets.push((name_id, constraint));
        self.version_set_cache.insert(cache_key, id);
        Ok(id)
    }

    pub fn intern_repo_version_set(
        &mut self,
        name_id: NameId,
        scheme: VersionScheme,
        constraint: crate::repository::versioning::RepoVersionConstraint,
        raw: Option<String>,
    ) -> Result<VersionSetId> {
        let constraint = ConaryConstraint::Repository {
            scheme,
            constraint,
            raw,
        };
        let cache_key = (name_id.0, constraint.clone());
        if let Some(&existing) = self.version_set_cache.get(&cache_key) {
            return Ok(existing);
        }
        let id = VersionSetId(Self::pool_u32(self.version_sets.len(), "version_set")?);
        self.version_sets.push((name_id, constraint));
        self.version_set_cache.insert(cache_key, id);
        Ok(id)
    }

    /// Intern a version set union (OR-group), returning its `VersionSetUnionId`.
    pub fn intern_version_set_union(
        &mut self,
        sets: Vec<VersionSetId>,
    ) -> Result<VersionSetUnionId> {
        let id = VersionSetUnionId(Self::pool_u32(
            self.version_set_unions.len(),
            "version_set_union",
        )?);
        self.union_id_index.insert(sets.clone(), id);
        self.version_set_unions.push(sets);
        Ok(id)
    }

    /// Intern a display string, returning its `StringId`.
    pub fn intern_string(&mut self, s: &str) -> Result<StringId> {
        let id = StringId(Self::pool_u32(self.strings.len(), "string")?);
        self.strings.push(s.to_string());
        Ok(id)
    }

    /// Register a solvable (package candidate) and return its `SolvableId`.
    pub fn add_solvable(&mut self, pkg: PackageIdentity) -> Result<SolvableId> {
        let idx = self.solvables.len();
        let id = SolvableId(Self::pool_u32(idx, "solvable")?);
        // Update name-to-solvable index for O(1) lookup by name.
        self.name_to_solvable_indices
            .entry(pkg.name.clone())
            .or_default()
            .push(idx);
        // Track repo_package_id for O(1) duplicate detection.
        if let Some(repo_id) = pkg.repo_package_id {
            self.loaded_repo_package_ids.insert(repo_id);
        }
        self.solvables.push(pkg);
        Ok(id)
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
            let effective_scheme = scheme.unwrap_or(VersionScheme::Rpm);

            let provided_capabilities: Vec<(String, Option<String>)> = if let Some(tid) = trove_id {
                ProvideEntry::find_by_trove(self.conn, tid)?
                    .into_iter()
                    .map(|provide| (provide.capability, provide.version))
                    .collect()
            } else {
                Vec::new()
            };

            // Intern name for side effect (ensures this name is known to the solver)
            let _name_id = self.intern_name(&trove.name)?;

            let pkg = PackageIdentity {
                repo_package_id: None,
                name: trove.name.clone(),
                version: trove.version.clone(),
                architecture: trove.architecture.clone(),
                version_scheme: effective_scheme,
                repository_id: trove.installed_from_repository_id.unwrap_or(0),
                repository_name: String::new(),
                repository_distro: trove.source_distro.clone(),
                repository_priority: 0,
                canonical_id: None,
                canonical_name: None,
                installed_trove_id: trove_id,
                provided_capabilities,
            };
            let solvable_id = self.add_solvable(pkg)?;

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
    /// ALL viable candidates (all versions from all repos) so the SAT solver
    /// can backtrack through multiple versions.
    pub fn load_repo_packages_for_names(&mut self, names: &[String]) -> Result<()> {
        use crate::repository::selector::{PackageSelector, SelectionOptions};
        use crate::repository::versioning::infer_version_scheme;

        let options = SelectionOptions::default();
        for name in names {
            // Skip if we already have a repo package for this name (O(1) index lookup).
            let already_has_repo = self
                .name_to_solvable_indices
                .get(name.as_str())
                .is_some_and(|indices| {
                    indices
                        .iter()
                        .any(|&i| self.solvables[i].repo_package_id.is_some())
                });
            if already_has_repo {
                continue;
            }

            // Load ALL candidates (all versions from all repos), not just the
            // best one. The SAT solver needs multiple candidates to backtrack.
            let mut candidates =
                PackageSelector::search_packages(self.conn, name, &options).unwrap_or_default();

            // Always include virtual-provide providers alongside exact-name
            // candidates so the solver can consider both. Filtering and ranking
            // determine which are preferred.
            let virtual_providers = self.find_repo_providers(name)?;
            candidates.extend(virtual_providers);

            for pkg_with_repo in candidates {
                // O(1) duplicate check via loaded_repo_package_ids set.
                if pkg_with_repo
                    .package
                    .id
                    .is_some_and(|id| self.loaded_repo_package_ids.contains(&id))
                {
                    continue;
                }

                let scheme =
                    infer_version_scheme(&pkg_with_repo.repository).unwrap_or(VersionScheme::Rpm);

                let _name_id = self.intern_name(&pkg_with_repo.package.name)?;

                let provided_capabilities = load_repo_provided_capabilities(
                    self.conn,
                    &pkg_with_repo.package,
                    &pkg_with_repo.repository,
                )?;

                let pkg = PackageIdentity {
                    repo_package_id: pkg_with_repo.package.id,
                    name: pkg_with_repo.package.name.clone(),
                    version: pkg_with_repo.package.version.clone(),
                    architecture: pkg_with_repo.package.architecture.clone(),
                    version_scheme: scheme,
                    repository_id: pkg_with_repo.repository.id.unwrap_or(0),
                    repository_name: pkg_with_repo.repository.name.clone(),
                    repository_distro: pkg_with_repo
                        .package
                        .distro
                        .clone()
                        .or(pkg_with_repo.repository.default_strategy_distro.clone()),
                    repository_priority: pkg_with_repo.repository.priority,
                    canonical_id: pkg_with_repo.package.canonical_id,
                    canonical_name: None,
                    installed_trove_id: None,
                    provided_capabilities,
                };
                let solvable_id = self.add_solvable(pkg)?;

                let sub_deps = load_repo_dependency_requests(
                    self.conn,
                    &pkg_with_repo.package,
                    &pkg_with_repo.repository,
                )?;
                self.dependencies.insert(solvable_id.0, sub_deps);
            }
        }

        self.refresh_latest_signal_cache()?;
        Ok(())
    }

    /// Build the `ProvidesIndex` from the database.
    ///
    /// Should be called once after `load_installed_packages()` and before
    /// resolution begins. The index enables O(1) capability-to-provider
    /// lookups, replacing per-dep DB queries.
    pub fn build_provides_index(&mut self) -> Result<()> {
        self.provides_index = Some(ProvidesIndex::build(self.conn)?);
        Ok(())
    }

    /// Look up which real packages provide a virtual capability.
    fn resolve_virtual_provide(&self, capability: &str) -> Vec<String> {
        let mut providers = Vec::new();

        // Use the pre-built ProvidesIndex when available (O(1) lookup).
        if let Some(ref index) = self.provides_index {
            for entry in index.find_providers(capability) {
                if let Some(repo_pkg_id) = entry.repo_package_id
                    && let Ok(Some(pkg)) = find_repo_package_by_id(self.conn, repo_pkg_id)
                    && !providers.contains(&pkg.name)
                {
                    providers.push(pkg.name.clone());
                }
                if let Some(trove_id) = entry.installed_trove_id
                    && let Ok(Some(trove)) = Trove::find_by_id(self.conn, trove_id)
                    && !providers.contains(&trove.name)
                {
                    providers.push(trove.name.clone());
                }
                // AppStream cross-distro provides: resolve canonical_id to
                // package names via repository_packages.canonical_id
                if let Some(cid) = entry.canonical_id
                    && let Ok(mut stmt) = self.conn.prepare(
                        "SELECT DISTINCT rp.name FROM repository_packages rp
                         JOIN repositories r ON rp.repository_id = r.id
                         WHERE rp.canonical_id = ?1 AND r.enabled = 1",
                    )
                    && let Ok(rows) = stmt.query_map([cid], |row| row.get::<_, String>(0))
                {
                    for name in rows.flatten() {
                        if !providers.contains(&name) {
                            providers.push(name);
                        }
                    }
                }
            }
            return providers;
        }

        // Fallback: per-dep DB queries when index is not built.
        if let Ok(provides) = ProvideEntry::find_all_by_capability(self.conn, capability) {
            for provide in provides {
                if let Ok(Some(trove)) = Trove::find_by_id(self.conn, provide.trove_id)
                    && !providers.contains(&trove.name)
                {
                    providers.push(trove.name.clone());
                }
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

        // Also check AppStream cross-distro provides via ProvidesIndex.
        // These have canonical_id but no direct repo_package_id, so we
        // resolve canonical_id -> repository_packages to get real packages.
        if let Some(ref index) = self.provides_index {
            for entry in index.find_providers(capability) {
                if let Some(cid) = entry.canonical_id {
                    let mut cid_stmt = self.conn.prepare(
                        "SELECT rp.id FROM repository_packages rp
                         JOIN repositories r ON rp.repository_id = r.id
                         WHERE rp.canonical_id = ?1 AND r.enabled = 1",
                    )?;
                    let pkg_ids: Vec<i64> = cid_stmt
                        .query_map([cid], |row| row.get(0))?
                        .flatten()
                        .collect();
                    for pkg_id in pkg_ids {
                        if let Some(pkg) = find_repo_package_by_id(self.conn, pkg_id)?
                            && let Some(repo) = crate::db::models::Repository::find_by_id(
                                self.conn,
                                pkg.repository_id,
                            )?
                        {
                            let already = providers.iter().any(|p| p.package.id == pkg.id);
                            if !already && repo.enabled {
                                providers.push(PackageWithRepo {
                                    package: pkg,
                                    repository: repo,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(providers)
    }

    /// Get the solvable package at a given index.
    pub fn get_solvable(&self, id: SolvableId) -> &PackageIdentity {
        &self.solvables[id.0 as usize]
    }

    pub(super) fn has_positive_latest_signal(&self, pkg: &PackageIdentity) -> bool {
        if self.policy.selection_mode != SelectionMode::Latest {
            return false;
        }

        let Some(canonical_id) = pkg.canonical_id else {
            return false;
        };
        let Some(distro) = pkg.repository_distro.as_ref() else {
            return false;
        };

        self.latest_positive_keys
            .contains(&(canonical_id, distro.clone()))
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
        self.new_dependency_names(&HashSet::new())
    }

    /// Collect dependency names not already in `known`, avoiding redundant allocations.
    pub fn new_dependency_names(&self, known: &HashSet<String>) -> Vec<String> {
        let mut seen = HashSet::new();
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
    pub fn intern_all_dependency_version_sets(&mut self) -> Result<()> {
        // Temporarily take ownership to avoid borrow conflict with self.intern_*()
        let all_deps = std::mem::take(&mut self.dependencies);

        for deps in all_deps.values() {
            for dep in deps {
                match dep {
                    SolverDep::Single(dep_name, constraint) => {
                        self.intern_constraint(dep_name, constraint)?;
                    }
                    SolverDep::OrGroup(alternatives) => {
                        let mut vs_ids = Vec::new();
                        for (dep_name, constraint) in alternatives {
                            self.intern_constraint(dep_name, constraint)?;
                            // Collect the interned version set IDs for the union
                            let name_id = self.intern_name(dep_name)?;
                            let cache_key = (name_id.0, constraint.clone());
                            if let Some(&vs_id) = self.version_set_cache.get(&cache_key) {
                                vs_ids.push(vs_id);
                            }
                        }
                        if vs_ids.len() > 1 {
                            self.intern_version_set_union(vs_ids)?;
                        }
                    }
                }
            }
        }

        // Restore the dependencies map
        self.dependencies = all_deps;
        Ok(())
    }

    /// Intern a single constraint, creating a version set for it.
    fn intern_constraint(&mut self, dep_name: &str, constraint: &ConaryConstraint) -> Result<()> {
        let name_id = self.intern_name(dep_name)?;
        match constraint {
            ConaryConstraint::Legacy(constraint) => {
                self.intern_version_set(name_id, constraint.clone())?;
            }
            ConaryConstraint::Repository {
                scheme,
                constraint,
                raw,
            } => {
                self.intern_repo_version_set(name_id, *scheme, constraint.clone(), raw.clone())?;
            }
        }
        Ok(())
    }

    /// Look up a single (name, constraint) pair as a `ConditionalRequirement`.
    pub(super) fn lookup_requirement(
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
    pub(super) fn find_union_id(&self, vs_ids: &[VersionSetId]) -> Option<VersionSetUnionId> {
        self.union_id_index.get(vs_ids).copied()
    }

    /// Find all solvables that match a given package name.
    pub(super) fn solvables_for_name(&self, name_id: NameId) -> Vec<SolvableId> {
        let name = &self.names[name_id.0 as usize];
        self.name_to_solvable_indices
            .get(name)
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|&i| {
                        self.solvable_id_from_index(i, "collecting candidates by name")
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn solvables_for_provide(&self, capability: &str) -> Vec<SolvableId> {
        self.solvables
            .iter()
            .enumerate()
            .filter(|(_, solvable)| {
                solvable
                    .provided_capabilities
                    .iter()
                    .any(|(provided, _version)| provided == capability)
            })
            .filter_map(|(i, _)| self.solvable_id_from_index(i, "collecting capability providers"))
            .collect()
    }

    /// Find the installed solvable for a name, if any.
    pub(super) fn installed_solvable_for_name(&self, name_id: NameId) -> Option<SolvableId> {
        let name = &self.names[name_id.0 as usize];
        self.name_to_solvable_indices.get(name).and_then(|indices| {
            indices
                .iter()
                .find(|&&i| self.solvables[i].installed_trove_id.is_some())
                .and_then(|&i| self.solvable_id_from_index(i, "selecting installed solvable"))
        })
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
            if let Some(tid) = solvable.installed_trove_id {
                self.trove_id_to_name.insert(tid, solvable.name.clone());
            }
        }

        // 2. Build removal_provides_index from already-loaded provided_capabilities.
        for solvable in &self.solvables {
            let Some(tid) = solvable.installed_trove_id else {
                continue;
            };
            for (capability, prov_version) in &solvable.provided_capabilities {
                // Index exact capability name.
                self.removal_provides_index
                    .entry(capability.clone())
                    .or_default()
                    .push((tid, prov_version.clone()));

                // Also index variations so that fuzzy lookups hit.
                for variation in generate_capability_variations(capability) {
                    self.removal_provides_index
                        .entry(variation)
                        .or_default()
                        .push((tid, prov_version.clone()));
                }
            }
        }

        // 3. Load UNFILTERED dependencies for each installed solvable.
        //    Same logic as `load_installed_packages` but WITHOUT the
        //    `.filter(|d| !ProvideEntry::is_virtual_provide(...))`.
        //    Batch-load all deps in one query instead of N per-solvable queries.
        let removal_trove_ids: Vec<i64> = self
            .solvables
            .iter()
            .filter_map(|s| s.installed_trove_id)
            .collect();
        let all_removal_deps = DependencyEntry::find_by_troves(self.conn, &removal_trove_ids)?;

        for (idx, solvable) in self.solvables.iter().enumerate() {
            let Some(tid) = solvable.installed_trove_id else {
                continue;
            };

            let empty = Vec::new();
            let deps = all_removal_deps.get(&tid).unwrap_or(&empty);
            let dep_list: Vec<SolverDep> = deps
                .iter()
                .map(|d| dep_entry_to_solver_dep(d, solvable.version_scheme))
                .collect();

            let sid_index = self.removal_deps_index(idx)?;
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
        let mut seen_ids = HashSet::new();

        // Exact match.
        if let Some(providers) = self.removal_provides_index.get(capability) {
            for &(tid, ref ver) in providers {
                if seen_ids.insert(tid) {
                    results.push((tid, ver.clone()));
                }
            }
        }

        // If no exact match, try variations of the dep name.
        if results.is_empty() {
            for variation in generate_capability_variations(capability) {
                if let Some(providers) = self.removal_provides_index.get(&variation) {
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

    fn refresh_latest_signal_cache(&mut self) -> Result<()> {
        self.latest_positive_keys.clear();

        if self.policy.selection_mode != SelectionMode::Latest {
            return Ok(());
        }

        let mut distros_by_canonical: HashMap<i64, HashSet<String>> = HashMap::new();
        for pkg in &self.solvables {
            let Some(canonical_id) = pkg.canonical_id else {
                continue;
            };
            let Some(distro) = pkg.repository_distro.as_ref() else {
                continue;
            };

            distros_by_canonical
                .entry(canonical_id)
                .or_default()
                .insert(distro.clone());
        }

        let now = Utc::now();
        for (canonical_id, distros) in distros_by_canonical {
            let distro_list = distros.into_iter().collect::<Vec<_>>();
            let rows = RepologyCacheEntry::find_for_canonical_and_distros(
                self.conn,
                canonical_id,
                &distro_list,
            )?;
            for row in rows {
                let status = row.status.as_deref().unwrap_or("");
                let signal = LatestSignal::from_repology(
                    status,
                    row.version.as_deref(),
                    &row.fetched_at,
                    now,
                )?;
                if signal.is_positive() {
                    self.latest_positive_keys.insert((canonical_id, row.distro));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::{
        CanonicalPackage, RepologyCacheEntry, Repository, RepositoryPackage, RepositoryProvide,
        RepositoryRequirement,
    };
    use crate::repository::resolution_policy::{ResolutionPolicy, SelectionMode};
    use crate::repository::versioning::RepoVersionConstraint;
    use crate::version::VersionConstraint;
    use futures::executor::block_on;
    use resolvo::{DependencyProvider, SolverCache};

    fn setup_test_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        db::init(&db_path).unwrap();
        let conn = db::open(&db_path).unwrap();
        (temp_dir, conn)
    }

    /// Helper to build a test `PackageIdentity` for installed packages.
    fn installed_identity(
        name: &str,
        version: &str,
        scheme: VersionScheme,
        trove_id: Option<i64>,
    ) -> PackageIdentity {
        PackageIdentity {
            repo_package_id: None,
            name: name.to_string(),
            version: version.to_string(),
            architecture: None,
            version_scheme: scheme,
            repository_id: 0,
            repository_name: String::new(),
            repository_distro: None,
            repository_priority: 0,
            canonical_id: None,
            canonical_name: None,
            installed_trove_id: trove_id,
            provided_capabilities: Vec::new(),
        }
    }

    /// Helper to build a test `PackageIdentity` for repo packages.
    fn repo_identity(
        name: &str,
        version: &str,
        scheme: VersionScheme,
        repo_package_id: Option<i64>,
    ) -> PackageIdentity {
        PackageIdentity {
            repo_package_id,
            name: name.to_string(),
            version: version.to_string(),
            architecture: None,
            version_scheme: scheme,
            repository_id: 0,
            repository_name: String::new(),
            repository_distro: None,
            repository_priority: 0,
            canonical_id: None,
            canonical_name: None,
            installed_trove_id: None,
            provided_capabilities: Vec::new(),
        }
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
            name == "kernel-core-uname-r" && *version == Some("6.19.6-200.fc43.x86_64".to_string())
        }));
    }

    #[test]
    fn test_intern_name_roundtrip() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let id1 = provider.intern_name("nginx").unwrap();
        let id2 = provider.intern_name("nginx").unwrap();
        let id3 = provider.intern_name("curl").unwrap();

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert_eq!(provider.names[id1.0 as usize], "nginx");
        assert_eq!(provider.names[id3.0 as usize], "curl");
    }

    #[test]
    fn test_version_set_filtering() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let name_id = provider.intern_name("lib").unwrap();
        let constraint = VersionConstraint::parse(">= 2.0.0").unwrap();
        let vs_id = provider.intern_version_set(name_id, constraint).unwrap();

        // Add candidates at different versions
        let s1 = provider
            .add_solvable(installed_identity("lib", "1.0.0", VersionScheme::Rpm, None))
            .unwrap();
        let s2 = provider
            .add_solvable(installed_identity("lib", "2.0.0", VersionScheme::Rpm, None))
            .unwrap();
        let s3 = provider
            .add_solvable(installed_identity("lib", "3.0.0", VersionScheme::Rpm, None))
            .unwrap();

        let candidates = [s1, s2, s3];

        // Test the filtering logic directly
        let (_, ref constraint) = provider.version_sets[vs_id.0 as usize];
        let matching: Vec<SolvableId> = candidates
            .iter()
            .copied()
            .filter(|&sid| {
                let pkg = &provider.solvables[sid.0 as usize];
                constraint_matches_package(constraint, &pkg.version, pkg.version_scheme)
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

        let name_id = provider.intern_name("nginx").unwrap();

        // Add an installed version
        let installed = provider
            .add_solvable(installed_identity(
                "nginx",
                "1.0.0",
                VersionScheme::Rpm,
                Some(42),
            ))
            .unwrap();

        // Add a repo version
        let _repo = provider
            .add_solvable(repo_identity(
                "nginx",
                "2.0.0",
                VersionScheme::Rpm,
                Some(100),
            ))
            .unwrap();

        // Test candidates lookup logic directly
        let candidates = provider.solvables_for_name(name_id);
        let favored = provider.installed_solvable_for_name(name_id);

        assert_eq!(candidates.len(), 2);
        assert_eq!(favored, Some(installed));
    }

    #[test]
    fn test_solvable_id_from_index_overflow_returns_none() {
        let (_dir, conn) = setup_test_db();
        let provider = ConaryProvider::new(&conn);

        assert!(
            provider
                .solvable_id_from_index(usize::MAX, "test overflow")
                .is_none()
        );
    }

    #[test]
    fn test_removal_deps_index_overflow_returns_error() {
        let (_dir, conn) = setup_test_db();
        let provider = ConaryProvider::new(&conn);

        let err = provider
            .removal_deps_index(usize::MAX)
            .expect_err("overflow should become an error instead of panicking");
        assert!(err.to_string().contains("solvable"));
    }

    #[test]
    fn test_sort_candidates_version_descending() {
        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let s1 = provider
            .add_solvable(installed_identity("pkg", "1.0.0", VersionScheme::Rpm, None))
            .unwrap();
        let s2 = provider
            .add_solvable(installed_identity("pkg", "3.0.0", VersionScheme::Rpm, None))
            .unwrap();
        let s3 = provider
            .add_solvable(installed_identity(
                "pkg",
                "2.0.0",
                VersionScheme::Rpm,
                Some(1),
            ))
            .unwrap();

        // Test sort logic directly
        let mut solvables = [s1, s2, s3];
        solvables.sort_by(|a, b| {
            let pkg_a = &provider.solvables[a.0 as usize];
            let pkg_b = &provider.solvables[b.0 as usize];
            if let Some(version_cmp) = matching::compare_package_versions_desc(
                &pkg_a.version,
                pkg_a.version_scheme,
                &pkg_b.version,
                pkg_b.version_scheme,
            ) && version_cmp != std::cmp::Ordering::Equal
            {
                return version_cmp;
            }
            let a_installed = pkg_a.installed_trove_id.is_some();
            let b_installed = pkg_b.installed_trove_id.is_some();
            b_installed.cmp(&a_installed)
        });

        // Should be: 3.0.0 (repo), 2.0.0 (installed), 1.0.0 (repo)
        assert_eq!(solvables[0], s2); // 3.0.0
        assert_eq!(solvables[1], s3); // 2.0.0 (installed)
        assert_eq!(solvables[2], s1); // 1.0.0
    }

    #[test]
    fn sort_candidates_prefers_latest_signal_when_policy_requests_it() {
        let (_dir, conn) = setup_test_db();

        let mut canonical = CanonicalPackage::new("python".to_string(), "package".to_string());
        let canonical_id = canonical.insert(&conn).unwrap();

        let mut fedora_repo = Repository::new(
            "fedora-remi".to_string(),
            "https://example.invalid".to_string(),
        );
        fedora_repo.priority = 20;
        fedora_repo.default_strategy_distro = Some("fedora".to_string());
        let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

        let mut arch_repo = Repository::new(
            "arch-core".to_string(),
            "https://example.invalid".to_string(),
        );
        arch_repo.priority = 5;
        arch_repo.default_strategy_distro = Some("arch".to_string());
        let arch_repo_id = arch_repo.insert(&conn).unwrap();

        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id)
             VALUES (?1, 'python', '3.12.2-1.fc43', 'sha256:fedora', 1, 'https://example.invalid/python-fedora.rpm', ?2)",
            rusqlite::params![fedora_repo_id, canonical_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, canonical_id)
             VALUES (?1, 'python', '3.13.0-1', 'sha256:arch', 1, 'https://example.invalid/python-arch.pkg.tar.zst', ?2)",
            rusqlite::params![arch_repo_id, canonical_id],
        )
        .unwrap();

        RepologyCacheEntry::insert_or_replace(
            &conn,
            &RepologyCacheEntry {
                project_name: "python".into(),
                distro: "fedora".into(),
                distro_name: "python".into(),
                version: Some("3.12.2".into()),
                status: Some("outdated".into()),
                fetched_at: "2026-04-07T00:00:00Z".into(),
            },
        )
        .unwrap();
        RepologyCacheEntry::insert_or_replace(
            &conn,
            &RepologyCacheEntry {
                project_name: "python".into(),
                distro: "arch".into(),
                distro_name: "python".into(),
                version: Some("3.13.0".into()),
                status: Some("newest".into()),
                fetched_at: "2026-04-07T00:00:00Z".into(),
            },
        )
        .unwrap();

        let mut provider = ConaryProvider::new_with_policy(
            &conn,
            ResolutionPolicy::new().with_selection_mode(SelectionMode::Latest),
        );
        provider
            .load_repo_packages_for_names(&["python".to_string()])
            .unwrap();

        let name_id = provider.intern_name("python").unwrap();
        let mut solvables = provider.solvables_for_name(name_id);
        assert_eq!(solvables.len(), 2);

        let cache = SolverCache::new(provider);
        block_on(cache.provider().sort_candidates(&cache, &mut solvables));

        assert_eq!(
            cache.provider().get_solvable(solvables[0]).repository_name,
            "arch-core"
        );
    }

    #[test]
    fn test_display_methods() {
        use resolvo::Interner;

        let (_dir, conn) = setup_test_db();
        let mut provider = ConaryProvider::new(&conn);

        let name_id = provider.intern_name("nginx").unwrap();
        let vs_id = provider
            .intern_version_set(name_id, VersionConstraint::parse(">= 1.0.0").unwrap())
            .unwrap();
        let sid = provider
            .add_solvable(installed_identity(
                "nginx",
                "1.24.0",
                VersionScheme::Rpm,
                None,
            ))
            .unwrap();
        let str_id = provider.intern_string("test string").unwrap();

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

        let capability_name = provider.intern_name("kernel-modules-core-uname-r").unwrap();
        let version_set = provider
            .intern_version_set(
                capability_name,
                VersionConstraint::parse("= 6.19.6").unwrap(),
            )
            .unwrap();

        let mut identity = installed_identity(
            "kernel-modules-core",
            "6.19.6-200.fc43",
            VersionScheme::Rpm,
            None,
        );
        identity.repo_package_id = Some(42);
        identity.provided_capabilities = vec![(
            "kernel-modules-core-uname-r".to_string(),
            Some("6.19.6".to_string()),
        )];
        let candidate = provider.add_solvable(identity).unwrap();

        let requested_name = &provider.names[capability_name.0 as usize];
        let (_, constraint) = &provider.version_sets[version_set.0 as usize];
        let matching: Vec<SolvableId> = [candidate]
            .into_iter()
            .filter(|sid| {
                let pkg = &provider.solvables[sid.0 as usize];
                let provided_version = pkg
                    .provided_capabilities
                    .iter()
                    .find(|(capability, _version)| capability == requested_name)
                    .and_then(|(_capability, version)| version.as_deref());
                let Some(pv) = provided_version else {
                    return false;
                };
                matching::constraint_matches_provide(
                    constraint,
                    Some(pv),
                    pkg.version_scheme,
                    &pkg.version,
                    pkg.version_scheme,
                )
            })
            .collect();
        assert_eq!(matching, vec![candidate]);
    }

    #[test]
    fn or_group_loading_from_requirement_groups() {
        use crate::db::models::RepositoryRequirementGroup;

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
        use crate::db::models::RepositoryRequirementGroup;

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

        // The provide version should be the raw string
        let (_, provide_version) = pkg_solvable
            .provided_capabilities
            .iter()
            .find(|(cap, _)| cap == "libc6")
            .unwrap();
        assert_eq!(*provide_version, Some("2.39-0ubuntu2".to_string()),);
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
        assert_eq!(*provide_version, Some("5.2.037".to_string()),);
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

        // Version should use Debian scheme
        let solvable = provider
            .solvables
            .iter()
            .find(|s| s.name == "libc6")
            .unwrap();
        assert_eq!(solvable.version, "2.39-0ubuntu2");
        assert_eq!(solvable.version_scheme, VersionScheme::Debian);

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
        assert_eq!(solvable.version, "2.39-1");
        assert_eq!(solvable.version_scheme, VersionScheme::Arch);

        // Arch constraint should match the installed version
        let constraint = ConaryConstraint::Repository {
            scheme: VersionScheme::Arch,
            constraint: RepoVersionConstraint::GreaterOrEqual("2.39".to_string()),
            raw: Some(">= 2.39".to_string()),
        };
        assert!(constraint_matches_package(
            &constraint,
            &solvable.version,
            solvable.version_scheme,
        ));
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

        // Should default to RPM scheme
        assert_eq!(solvable.version_scheme, VersionScheme::Rpm);

        // Legacy VersionConstraint should still work
        let constraint = ConaryConstraint::Legacy(VersionConstraint::parse(">= 5.2.0").unwrap());
        assert!(constraint_matches_package(
            &constraint,
            &solvable.version,
            solvable.version_scheme,
        ));
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
