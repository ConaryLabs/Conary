// conary-core/src/resolver/provider/mod.rs

//! Bridge between Conary's data model and resolvo's SAT solver.
//!
//! Implements resolvo's `DependencyProvider` and `Interner` traits via
//! `ConaryProvider`, which maps Conary packages (troves + repo packages)
//! to resolvo's abstract solvable/name/version-set model.
//!
//! Solvables are `PackageIdentity` instances carrying full provenance.

mod evidence;
mod expression;
mod loading;
pub(crate) mod matching;
mod traits;
pub mod types;

use std::collections::{HashMap, HashSet};

use crate::db::models::RepositoryProvide;
use crate::error::{Error, Result};
use crate::repository::dependency_model::ProvideVersionRelation;
use crate::repository::resolution_policy::{RequestScope, ResolutionPolicy};
use crate::repository::versioning::{VersionScheme, validate_repo_version};
use crate::resolver::identity::PackageIdentity;
use crate::resolver::provides_index::{ProviderEntry, ProvidesIndex};
use resolvo::{
    Condition, ConditionalRequirement, DenseIndex, NameId, SolvableId, StringId, VersionSetId,
    VersionSetUnionId,
};

pub(crate) use loading::repository_expression_to_solver_for_architecture;
use loading::{
    find_repo_package_by_id, load_installed_dependency_requests, load_installed_relations,
    load_repo_dependency_requests, load_repo_provided_capabilities, load_repo_relations,
    relation_to_solver_dep,
};
pub(crate) use matching::constraint_matches_package;
pub use types::{ConaryConstraint, SolverDep, SolverExpression, SolverRelation};

type RemovalProvider = (i64, Option<String>, Option<ProvideVersionRelation>);
type RemovalProvidesIndex = HashMap<String, Vec<RemovalProvider>>;

/// Bridge between Conary's data model and resolvo's abstract solver interface.
///
/// Owns interning pools for names, solvables, version sets, and strings,
/// and queries Conary's DB on demand to feed the solver.
pub struct ConaryProvider<'db> {
    // --- Interning pools ---
    pub(super) names: Vec<String>,
    pub(super) name_to_id: HashMap<String, NameId>,

    pub(super) solvables: Vec<PackageIdentity>,
    pub(super) solvable_ids: Vec<SolvableId>,

    /// Each version set is (name_id, constraint).
    pub(super) version_sets: Vec<(NameId, ConaryConstraint)>,

    /// Cache for deduplicating version sets by (name_id, constraint).
    pub(super) version_set_cache: HashMap<(u32, ConaryConstraint), VersionSetId>,

    /// Each version set union is a list of `VersionSetId`s (OR alternatives).
    pub(super) version_set_unions: Vec<Vec<VersionSetId>>,

    /// Reverse index: package name -> validated IDs in the `solvables` pool.
    /// Enables O(1) lookup instead of linear scan when finding solvables by name.
    name_to_solvable_ids: HashMap<String, Vec<SolvableId>>,

    /// Set of repo_package_ids already loaded as solvables.
    /// Enables O(1) duplicate check instead of linear scan.
    loaded_repo_package_ids: HashSet<i64>,

    /// Reverse index: version set ID list -> union ID.
    /// Enables O(1) lookup instead of linear scan in `find_union_id`.
    pub(super) union_id_index: HashMap<Vec<VersionSetId>, VersionSetUnionId>,

    pub(super) strings: Vec<String>,
    missing_dependency_authority: StringId,

    /// Pre-loaded dependencies for each solvable, keyed by `SolvableId` index.
    pub(super) dependencies: HashMap<u32, Vec<SolverDep>>,

    /// Exact negative and replacement relations keyed by solver candidate.
    pub(super) relations: HashMap<u32, Vec<SolverRelation>>,

    /// Exact SAT requirements compiled from native dependency expressions.
    pub(super) compiled_dependencies: HashMap<u32, Vec<ConditionalRequirement>>,

    /// Exact persisted group behind each compiled positive version set.
    compiled_requirement_groups:
        HashMap<(u32, u32), std::collections::BTreeSet<types::RepositoryRequirementGroupIdentity>>,

    /// Boolean conditions referenced by compiled conditional requirements.
    pub(super) conditions: Vec<Condition>,

    /// Deduplication index for Boolean conditions.
    pub(super) condition_cache: HashMap<Condition, resolvo::ConditionId>,

    /// Interned names whose candidate set is governed by a same-provider
    /// capability expression rather than package-name identity.
    pub(super) provider_expression_name_ids: HashSet<u32>,

    /// Index of capability name -> list of (trove_id, optional version) providers.
    /// Built by `load_removal_data()` from already-loaded solvable provides.
    removal_provides_index: RemovalProvidesIndex,

    /// Reverse map from trove_id to package name, for quick lookup during removal.
    trove_id_to_name: HashMap<i64, String>,

    /// Unfiltered dependencies for each solvable, including virtual provides
    /// (soname, perl, python, etc.) that `dependencies` strips out.
    /// Keyed by `SolvableId` index.
    removal_deps: HashMap<u32, Vec<SolverDep>>,

    /// distro_name -> Vec<equivalent distro_name> for canonical cross-distro resolution.
    /// Pre-loaded as a HashMap for O(1) lookup in the hot path.
    canonical_equivalents: HashMap<String, Vec<String>>,

    /// Demand-driven capability-to-provider cache (modeled after libsolv's
    /// whatprovides). Initialized empty at resolution start via
    /// `build_provides_index()` and populated during pre-solve loading.
    provides_index: Option<ProvidesIndex<'db>>,

    /// Source-selection policy that should influence SAT candidate ordering.
    policy: ResolutionPolicy,

    /// Exact root request names. RequestScope applies only to these names.
    root_request_names: HashSet<String>,

    /// Exact target-native architecture used by source dependency qualifiers.
    native_architecture: String,

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
            solvable_ids: Vec::new(),
            version_sets: Vec::new(),
            version_set_cache: HashMap::new(),
            version_set_unions: Vec::new(),
            name_to_solvable_ids: HashMap::new(),
            loaded_repo_package_ids: HashSet::new(),
            union_id_index: HashMap::new(),
            strings: vec![
                "package candidate has no compiled dependency authority; candidate excluded"
                    .to_string(),
            ],
            missing_dependency_authority: StringId(0),
            dependencies: HashMap::new(),
            relations: HashMap::new(),
            compiled_dependencies: HashMap::new(),
            compiled_requirement_groups: HashMap::new(),
            conditions: Vec::new(),
            condition_cache: HashMap::new(),
            provider_expression_name_ids: HashSet::new(),
            removal_provides_index: HashMap::new(),
            trove_id_to_name: HashMap::new(),
            removal_deps: HashMap::new(),
            canonical_equivalents: HashMap::new(),
            provides_index: None,
            policy,
            root_request_names: HashSet::new(),
            native_architecture: crate::repository::registry::detect_system_arch(),
            conn,
        }
    }

    pub fn set_root_request_names(&mut self, names: impl IntoIterator<Item = String>) {
        self.root_request_names = names.into_iter().collect();
    }

    pub fn expand_root_request_names_with_canonical_equivalents(&mut self) {
        let equivalents = self
            .root_request_names
            .iter()
            .flat_map(|name| self.canonical_equivalents(name).iter().cloned())
            .collect::<Vec<_>>();
        self.root_request_names.extend(equivalents);
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

    /// Intern a package name, returning its `NameId`.
    pub fn intern_name(&mut self, name: &str) -> Result<NameId> {
        if let Some(&id) = self.name_to_id.get(name) {
            return Ok(id);
        }
        let id = NameId::from_raw(Self::pool_u32(self.names.len(), "name")?);
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
        let constraint = ConaryConstraint::Requested(constraint);
        self.intern_conary_version_set(name_id, constraint)
    }

    fn intern_conary_version_set(
        &mut self,
        name_id: NameId,
        constraint: ConaryConstraint,
    ) -> Result<VersionSetId> {
        if matches!(constraint, ConaryConstraint::RpmRuntime(_)) {
            return Err(Error::ResolutionError(
                "RPM runtime capability reached package version-set interning".to_string(),
            ));
        }
        if name_id.to_index() >= self.names.len() {
            return Err(Error::ResolutionError(format!(
                "cannot intern a version set for unknown resolver name ID {}",
                name_id.into_raw()
            )));
        }
        let cache_key = (name_id.into_raw(), constraint.clone());
        if let Some(&existing) = self.version_set_cache.get(&cache_key) {
            return Ok(existing);
        }
        if matches!(constraint, ConaryConstraint::ProviderExpression { .. }) {
            self.provider_expression_name_ids.insert(name_id.into_raw());
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
            capability_kind: None,
            raw,
            architecture_qualifier: Default::default(),
            depending_architecture: self.native_architecture.clone(),
        };
        self.intern_conary_version_set(name_id, constraint)
    }

    /// Intern a version set union (OR-group), returning its `VersionSetUnionId`.
    pub fn intern_version_set_union(
        &mut self,
        sets: Vec<VersionSetId>,
    ) -> Result<VersionSetUnionId> {
        for set in &sets {
            if set.to_index() >= self.version_sets.len() {
                return Err(Error::ResolutionError(format!(
                    "cannot intern a version-set union containing unknown version-set ID {}",
                    set.0
                )));
            }
        }
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
        if pkg.repo_package_id.is_some()
            && pkg
                .architecture
                .as_deref()
                .is_none_or(|architecture| architecture.is_empty())
        {
            return Err(Error::ConfigError(format!(
                "repository package '{}-{}' has no architecture authority",
                pkg.name, pkg.version
            )));
        }
        if pkg.installed_pinned && pkg.installed_trove_id.is_none() {
            return Err(Error::ResolutionError(format!(
                "resolver candidate '{}' is marked installed-pinned without an installed trove identity",
                pkg.name
            )));
        }
        validate_repo_version(pkg.version_scheme, &pkg.version)?;
        for capability in &pkg.provided_capabilities {
            if let Some(version) = capability.version.as_deref() {
                validate_repo_version(capability.version_scheme, version)?;
            }
        }
        let idx = self.solvables.len();
        let id = SolvableId::from_raw(Self::pool_u32(idx, "solvable")?);
        // Update name-to-solvable index for O(1) lookup by name.
        self.name_to_solvable_ids
            .entry(pkg.name.clone())
            .or_default()
            .push(id);
        // Track repo_package_id for O(1) duplicate detection.
        if let Some(repo_id) = pkg.repo_package_id {
            self.loaded_repo_package_ids.insert(repo_id);
        }
        self.solvables.push(pkg);
        self.solvable_ids.push(id);
        self.dependencies.insert(id.into_raw(), Vec::new());
        self.relations.insert(id.into_raw(), Vec::new());
        Ok(id)
    }

    /// Bulk-load all installed troves as solvables.
    pub fn load_installed_packages(&mut self) -> Result<()> {
        let packages = crate::resolver::requirements::load_installed_package_identities(self.conn)?;

        for pkg in packages {
            let package_architecture = pkg
                .architecture
                .clone()
                .unwrap_or_else(|| self.native_architecture.clone());
            // Intern name for side effect (ensures this name is known to the solver)
            let _name_id = self.intern_name(&pkg.name)?;
            let trove_id = pkg.installed_trove_id;
            let solvable_id = self.add_solvable(pkg)?;

            if let Some(tid) = trove_id {
                let mut dep_list =
                    load_installed_dependency_requests(self.conn, tid, &package_architecture)?;
                let relations = load_installed_relations(self.conn, tid)?;
                dep_list.extend(
                    relations
                        .iter()
                        .map(relation_to_solver_dep)
                        .collect::<Result<Vec<_>>>()?,
                );
                self.dependencies.insert(solvable_id.into_raw(), dep_list);
                self.relations.insert(solvable_id.into_raw(), relations);
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
        use crate::repository::selector::{ArchitectureScope, PackageSelector, SelectionOptions};

        if self.policy.primary_source_identity().is_none() {
            let primary_source_identity = policy_primary_source_identity(self.conn, &self.policy)?;
            self.policy
                .set_primary_source_identity(primary_source_identity);
        }
        for name in names {
            // Skip if we already have a repo package for this name (O(1) index lookup).
            let already_has_repo =
                self.name_to_solvable_ids
                    .get(name.as_str())
                    .is_some_and(|ids| {
                        ids.iter()
                            .any(|id| self.solvables[id.to_index()].repo_package_id.is_some())
                    });
            if already_has_repo {
                continue;
            }

            // Load ALL candidates (all versions from all repos), not just the
            // best one. The SAT solver needs multiple candidates to backtrack.
            let options = SelectionOptions {
                architecture_scope: ArchitectureScope::All,
                policy: Some(self.policy.clone()),
                is_root: self.root_request_names.contains(name),
                ..SelectionOptions::default()
            };
            let mut candidates = PackageSelector::search_packages(self.conn, name, &options)?;

            // Always include virtual-provide providers alongside exact-name
            // candidates so the solver can consider both. Filtering and ranking
            // determine which are preferred.
            let is_root = self.root_request_names.contains(name);
            let mut virtual_providers = Vec::new();
            for candidate in self.find_repo_providers(name)? {
                let candidate_identity = crate::repository::selector::candidate_source_identity(
                    &candidate.package,
                    &candidate.repository,
                )?;
                if self.policy.accepts_candidate(
                    &candidate.repository.name,
                    candidate_identity,
                    is_root,
                ) {
                    virtual_providers.push(candidate);
                }
            }
            candidates.extend(virtual_providers);

            for pkg_with_repo in candidates {
                let repository_package_id = pkg_with_repo.package.id.ok_or_else(|| {
                    Error::MissingId(format!(
                        "repository package '{}-{}' selected as a solver candidate",
                        pkg_with_repo.package.name, pkg_with_repo.package.version
                    ))
                })?;
                // O(1) duplicate check via loaded_repo_package_ids set.
                if self
                    .loaded_repo_package_ids
                    .contains(&repository_package_id)
                {
                    continue;
                }

                let scheme = pkg_with_repo.package.version_scheme;

                let _name_id = self.intern_name(&pkg_with_repo.package.name)?;

                let provided_capabilities = load_repo_provided_capabilities(
                    self.conn,
                    &pkg_with_repo.package,
                    &pkg_with_repo.repository,
                )?;
                let repository_id = pkg_with_repo.repository.id.ok_or_else(|| {
                    Error::MissingId(format!(
                        "repository '{}' selected as a solver candidate",
                        pkg_with_repo.repository.name
                    ))
                })?;

                let pkg = PackageIdentity {
                    repo_package_id: Some(repository_package_id),
                    name: pkg_with_repo.package.name.clone(),
                    version: pkg_with_repo.package.version.clone(),
                    package_release: (!pkg_with_repo.package.package_release.is_empty())
                        .then(|| pkg_with_repo.package.package_release.clone()),
                    architecture: pkg_with_repo.package.architecture.clone(),
                    debian_multi_arch: pkg_with_repo.package.debian_multi_arch,
                    version_scheme: scheme,
                    repository_id: Some(repository_id),
                    repository_name: pkg_with_repo.repository.name.clone(),
                    repository_profile: crate::repository::selector::candidate_source_profile(
                        &pkg_with_repo.package,
                        &pkg_with_repo.repository,
                    )?
                    .map(str::to_string),
                    repository_priority: pkg_with_repo.repository.priority,
                    canonical_id: pkg_with_repo.package.canonical_id,
                    canonical_name: None,
                    installed_trove_id: None,
                    installed_pinned: false,
                    provided_capabilities,
                };
                let solvable_id = self.add_solvable(pkg)?;

                let mut sub_deps = load_repo_dependency_requests(
                    self.conn,
                    &pkg_with_repo.package,
                    &pkg_with_repo.repository,
                )?;
                let relations = load_repo_relations(self.conn, &pkg_with_repo.package)?;
                sub_deps.extend(
                    relations
                        .iter()
                        .map(relation_to_solver_dep)
                        .collect::<Result<Vec<_>>>()?,
                );
                self.dependencies.insert(solvable_id.into_raw(), sub_deps);
                self.relations.insert(solvable_id.into_raw(), relations);
            }
        }

        Ok(())
    }

    /// Initialize the demand-driven `ProvidesIndex` from the database.
    ///
    /// Should be called once after `load_installed_packages()` and before
    /// resolution begins. Capability rows are fetched only when the
    /// transitive pre-solve loader names that capability; no provider table is
    /// traversed here.
    pub fn build_provides_index(&mut self) -> Result<()> {
        self.provides_index = Some(ProvidesIndex::build(self.conn)?);
        Ok(())
    }

    fn find_repo_providers(
        &mut self,
        capability: &str,
    ) -> Result<Vec<crate::repository::selector::PackageWithRepo>> {
        use crate::repository::selector::PackageWithRepo;

        let mut providers = Vec::<PackageWithRepo>::new();
        let entries = if let Some(index) = self.provides_index.as_mut() {
            index.find_providers(capability)?
        } else {
            // Keep direct provider construction useful for callers that do
            // not opt into the install builder. The normal SAT path always
            // initializes the demand-driven index above.
            RepositoryProvide::find_by_capability(self.conn, capability)?
                .into_iter()
                .map(|provide| ProviderEntry {
                    repo_package_id: Some(provide.repository_package_id),
                    installed_trove_id: None,
                    canonical_id: None,
                    provide_version: provide.version,
                    version_relation: provide.version_relation,
                    version_scheme: Some(provide.version_scheme),
                })
                .collect()
        };

        for entry in entries {
            if let Some(pkg_id) = entry.repo_package_id {
                let pkg = find_repo_package_by_id(self.conn, pkg_id)?.ok_or_else(|| {
                    Error::ConfigError(format!(
                        "repository provide '{capability}' references missing package row {pkg_id}"
                    ))
                })?;
                let repo = crate::db::models::Repository::find_by_id(self.conn, pkg.repository_id)?
                    .ok_or_else(|| {
                        Error::ConfigError(format!(
                            "repository package '{}' references missing repository row {}",
                            pkg.name, pkg.repository_id
                        ))
                    })?;
                if repo.enabled {
                    providers.push(PackageWithRepo {
                        package: pkg,
                        repository: repo,
                    });
                }
                continue;
            }

            // AppStream entries have canonical_id but no direct repo package
            // ID. Resolve the canonical package to enabled repository rows.
            let Some(cid) = entry.canonical_id else {
                continue;
            };
            let mut cid_stmt = self.conn.prepare(
                "SELECT rp.id FROM resolved_repository_packages rp
                 JOIN repositories r ON rp.repository_id = r.id
                 WHERE rp.canonical_id = ?1 AND r.enabled = 1",
            )?;
            let pkg_ids = cid_stmt
                .query_map([cid], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            for pkg_id in pkg_ids {
                let pkg = find_repo_package_by_id(self.conn, pkg_id)?.ok_or_else(|| {
                    Error::ConfigError(format!(
                        "AppStream provider for canonical package {cid} references missing package row {pkg_id}"
                    ))
                })?;
                let repo = crate::db::models::Repository::find_by_id(self.conn, pkg.repository_id)?
                    .ok_or_else(|| {
                        Error::ConfigError(format!(
                            "repository package '{}' references missing repository row {}",
                            pkg.name, pkg.repository_id
                        ))
                    })?;
                let already = providers.iter().any(|p| p.package.id == pkg.id);
                if !already && repo.enabled {
                    providers.push(PackageWithRepo {
                        package: pkg,
                        repository: repo,
                    });
                }
            }
        }

        Ok(providers)
    }

    /// Get the solvable package at a given index.
    pub fn get_solvable(&self, id: SolvableId) -> &PackageIdentity {
        &self.solvables[id.to_index()]
    }

    /// Get the total number of solvables.
    pub fn solvable_count(&self) -> usize {
        self.solvables.len()
    }

    /// Return every solver candidate ID minted by this provider.
    pub fn solvable_ids(&self) -> &[SolvableId] {
        &self.solvable_ids
    }

    /// Get the dependency list for a solvable (if loaded).
    pub fn get_dependency_list(&self, id: SolvableId) -> Option<&[SolverDep]> {
        self.dependencies.get(&id.into_raw()).map(Vec::as_slice)
    }

    pub fn get_relation_list(&self, id: SolvableId) -> Result<&[SolverRelation]> {
        self.relations
            .get(&id.into_raw())
            .map(Vec::as_slice)
            .ok_or_else(|| {
                Error::ResolutionError(format!(
                    "solver candidate {} has no registered relation authority",
                    id.into_raw()
                ))
            })
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
                for atom in dep.expression.atoms() {
                    match &atom.constraint {
                        ConaryConstraint::ProviderExpression { expression } => {
                            expression.collect_names(known, &mut seen);
                        }
                        ConaryConstraint::RpmRuntime(_) => {}
                        ConaryConstraint::ExactRepositoryPackage(_) => {}
                        ConaryConstraint::Requested(_) | ConaryConstraint::Repository { .. }
                            if !known.contains(atom.name.as_str()) =>
                        {
                            seen.insert(atom.name.clone());
                        }
                        ConaryConstraint::Requested(_) | ConaryConstraint::Repository { .. } => {}
                    }
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Intern version sets for all loaded dependencies so that `get_dependencies`
    /// can find them when the solver queries.
    pub fn intern_all_dependency_version_sets(&mut self) -> Result<()> {
        self.compile_dependency_requirements()?;
        self.validate_solver_invariants()
    }

    fn validate_solver_invariants(&self) -> Result<()> {
        if self.solvables.len() != self.solvable_ids.len() {
            return Err(Error::ResolutionError(format!(
                "resolver candidate pool has {} packages but {} validated IDs",
                self.solvables.len(),
                self.solvable_ids.len()
            )));
        }

        for (index, (package, id)) in self.solvables.iter().zip(&self.solvable_ids).enumerate() {
            let expected = Self::pool_u32(index, "solvable")?;
            if id.into_raw() != expected {
                return Err(Error::ResolutionError(format!(
                    "resolver candidate '{}' has ID {} at pool position {expected}",
                    package.name,
                    id.into_raw()
                )));
            }
            if !self.dependencies.contains_key(&id.into_raw())
                || !self.relations.contains_key(&id.into_raw())
                || !self.compiled_dependencies.contains_key(&id.into_raw())
            {
                return Err(Error::ResolutionError(format!(
                    "resolver candidate '{}' ({}) is missing registered dependency, relation, or compiled authority",
                    package.name,
                    id.into_raw()
                )));
            }
            let Some(ids) = self.name_to_solvable_ids.get(&package.name) else {
                return Err(Error::ResolutionError(format!(
                    "resolver candidate '{}' ({}) is absent from the name index",
                    package.name,
                    id.into_raw()
                )));
            };
            if !ids.contains(id) {
                return Err(Error::ResolutionError(format!(
                    "resolver candidate '{}' ({}) is absent from its name index entry",
                    package.name,
                    id.into_raw()
                )));
            }
        }

        for (name, ids) in &self.name_to_solvable_ids {
            for id in ids {
                let Some(package) = self.solvables.get(id.to_index()) else {
                    return Err(Error::ResolutionError(format!(
                        "resolver name index '{}' references unknown candidate ID {}",
                        name,
                        id.into_raw()
                    )));
                };
                if package.name != *name {
                    return Err(Error::ResolutionError(format!(
                        "resolver name index '{}' references candidate '{}' ({})",
                        name,
                        package.name,
                        id.into_raw()
                    )));
                }
            }
        }

        for (index, sets) in self.version_set_unions.iter().enumerate() {
            let expected = VersionSetUnionId(Self::pool_u32(index, "version_set_union")?);
            if self.union_id_index.get(sets) != Some(&expected) {
                return Err(Error::ResolutionError(format!(
                    "resolver version-set union {index} is absent from its reverse index"
                )));
            }
            for set in sets {
                if set.to_index() >= self.version_sets.len() {
                    return Err(Error::ResolutionError(format!(
                        "resolver version-set union {index} references unknown version-set ID {}",
                        set.0
                    )));
                }
            }
        }

        Ok(())
    }

    /// Intern a single constraint, creating a version set for it.
    fn intern_constraint(&mut self, dep_name: &str, constraint: &ConaryConstraint) -> Result<()> {
        let name_id = self.intern_name(dep_name)?;
        match constraint {
            ConaryConstraint::Requested(constraint) => {
                self.intern_version_set(name_id, constraint.clone())?;
            }
            ConaryConstraint::ExactRepositoryPackage(_) => {
                self.intern_conary_version_set(name_id, constraint.clone())?;
            }
            ConaryConstraint::Repository { .. } => {
                self.intern_conary_version_set(name_id, constraint.clone())?;
            }
            ConaryConstraint::RpmRuntime(_) => {
                return Err(Error::ResolutionError(
                    "RPM runtime capability reached package constraint interning".to_string(),
                ));
            }
            ConaryConstraint::ProviderExpression { .. } => {
                self.intern_conary_version_set(name_id, constraint.clone())?;
            }
        }
        Ok(())
    }

    /// Find a previously-interned union ID matching the given version set IDs.
    pub(super) fn find_union_id(&self, vs_ids: &[VersionSetId]) -> Option<VersionSetUnionId> {
        self.union_id_index.get(vs_ids).copied()
    }

    /// Find all solvables that match a given package name.
    pub(super) fn solvables_for_name(&self, name_id: NameId) -> Vec<SolvableId> {
        let name = &self.names[name_id.to_index()];
        self.name_to_solvable_ids
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn solvables_for_provide(&self, capability: &str) -> Vec<SolvableId> {
        self.solvables
            .iter()
            .zip(&self.solvable_ids)
            .filter(|(solvable, _)| {
                solvable
                    .provided_capabilities
                    .iter()
                    .any(|provided| provided.name == capability)
            })
            .map(|(_, id)| *id)
            .collect()
    }

    /// Find the installed solvable for a name, if any.
    pub(super) fn installed_solvable_for_name(&self, name_id: NameId) -> Option<SolvableId> {
        let name = &self.names[name_id.to_index()];
        self.name_to_solvable_ids.get(name).and_then(|ids| {
            ids.iter()
                .find(|id| self.solvables[id.to_index()].installed_trove_id.is_some())
                .copied()
        })
    }

    /// Select one stable installed witness from an already-bounded candidate
    /// set. The caller retains root-name and version filtering authority.
    pub(super) fn installed_solvable_for_candidates(
        &self,
        candidates: &[SolvableId],
    ) -> Option<SolvableId> {
        candidates
            .iter()
            .copied()
            .filter(|id| self.solvables[id.to_index()].installed_trove_id.is_some())
            .min_by(|left, right| {
                let left = &self.solvables[left.to_index()];
                let right = &self.solvables[right.to_index()];
                left.name
                    .cmp(&right.name)
                    .then_with(|| {
                        left.version_scheme
                            .as_str()
                            .cmp(right.version_scheme.as_str())
                    })
                    .then_with(|| left.version.cmp(&right.version))
                    .then_with(|| left.package_release.cmp(&right.package_release))
                    .then_with(|| left.architecture.cmp(&right.architecture))
                    .then_with(|| left.installed_trove_id.cmp(&right.installed_trove_id))
            })
    }

    /// Build the provides index, trove-id-to-name map, and dependency list used
    /// by `solve_removal()`.
    ///
    /// Must be called after `load_installed_packages()`. All data comes from
    /// already-loaded solvables and a single extra DB query per trove.
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
            for capability in &solvable.provided_capabilities {
                self.removal_provides_index
                    .entry(capability.name.clone())
                    .or_default()
                    .push((tid, capability.version.clone(), capability.version_relation));
            }
        }

        for (solvable, sid) in self.solvables.iter().zip(&self.solvable_ids) {
            let Some(tid) = solvable.installed_trove_id else {
                continue;
            };

            let package_architecture = solvable
                .architecture
                .as_deref()
                .unwrap_or(&self.native_architecture);
            let dep_list =
                load_installed_dependency_requests(self.conn, tid, package_architecture)?;

            self.removal_deps.insert(sid.into_raw(), dep_list);
        }

        Ok(())
    }

    /// Look up which troves provide a given capability.
    ///
    /// Returns exact installed provider range contracts.
    pub fn find_providers(
        &self,
        capability: &str,
    ) -> Vec<(i64, Option<String>, Option<ProvideVersionRelation>)> {
        let mut results = Vec::new();
        let mut seen_ids = HashSet::new();

        if let Some(providers) = self.removal_provides_index.get(capability) {
            for &(tid, ref ver, relation) in providers {
                if seen_ids.insert(tid) {
                    results.push((tid, ver.clone(), relation));
                }
            }
        }

        results
    }

    /// Map a trove id back to its package name.
    pub fn trove_name(&self, trove_id: i64) -> Option<&str> {
        self.trove_id_to_name.get(&trove_id).map(String::as_str)
    }

    pub(super) fn installed_package_for_trove(&self, trove_id: i64) -> Option<&PackageIdentity> {
        self.solvables
            .iter()
            .find(|package| package.installed_trove_id == Some(trove_id))
    }

    /// Get the unfiltered dependency list for a solvable (if loaded).
    ///
    /// Unlike `get_dependency_list()` this includes virtual provides such as
    /// soname deps, perl modules, etc. that the regular list strips out.
    pub fn get_removal_dependency_list(&self, id: SolvableId) -> Option<&[SolverDep]> {
        self.removal_deps.get(&id.into_raw()).map(Vec::as_slice)
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
             FROM resolved_package_implementations pi1
             JOIN resolved_package_implementations pi2 ON pi1.canonical_id = pi2.canonical_id
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

fn policy_primary_source_identity(
    conn: &rusqlite::Connection,
    policy: &ResolutionPolicy,
) -> Result<Option<String>> {
    match &policy.request_scope {
        RequestScope::SourceIdentity(source_identity) => Ok(Some(source_identity.clone())),
        RequestScope::Repository(name) => {
            let repository = crate::db::models::Repository::find_by_name(conn, name)?
                .ok_or_else(|| Error::ConfigError(format!("repository '{name}' does not exist")))?;
            Ok(repository.resolution_source_identity()?.map(str::to_string))
        }
        RequestScope::Any => Ok(None),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
