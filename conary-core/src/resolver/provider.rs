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
    Trove,
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
    Installed(RpmVersion),
    Repository {
        raw: String,
        scheme: Option<VersionScheme>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConaryProvidedVersion {
    Installed(RpmVersion),
    Repository {
        raw: String,
        scheme: VersionScheme,
    },
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

    strings: Vec<String>,

    /// Pre-loaded dependencies for each solvable, keyed by `SolvableId` index.
    dependencies: HashMap<u32, Vec<(String, ConaryConstraint)>>,

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
        let id = NameId(u32::try_from(self.names.len()).expect("resolver name pool overflow"));
        self.names.push(name.to_string());
        self.name_to_id.insert(name.to_string(), id);
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
        self.version_sets.push((name_id, constraint.clone()));
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
        for trove in troves {
            let trove_id = trove.id;
            let version = RpmVersion::parse(&trove.version)?;
            let provided_capabilities = if let Some(tid) = trove_id {
                ProvideEntry::find_by_trove(self.conn, tid)?
                    .into_iter()
                    .map(|provide| {
                        let version = provide
                            .version
                            .as_deref()
                            .and_then(|value| RpmVersion::parse(value).ok());
                        (
                            provide.capability,
                            version.map(ConaryProvidedVersion::Installed),
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            };
            // Intern name for side effect (ensures this name is known to the solver)
            let _name_id = self.intern_name(&trove.name);

            let pkg = ConaryPackage {
                name: trove.name.clone(),
                version: ConaryPackageVersion::Installed(version),
                trove_id,
                repo_package_id: None,
                provided_capabilities,
            };
            let solvable_id = self.add_solvable(pkg);

            // Load dependencies for this installed trove
            if let Some(tid) = trove_id {
                let deps = DependencyEntry::find_by_trove(self.conn, tid)?;
                let dep_list: Vec<(String, ConaryConstraint)> = deps
                    .into_iter()
                    .filter(|d| !ProvideEntry::is_virtual_provide(&d.depends_on_name))
                    .map(|d| {
                        let constraint = d
                            .version_constraint
                            .as_deref()
                            .and_then(|s| VersionConstraint::parse(s).ok())
                            .unwrap_or(VersionConstraint::Any);
                        (d.depends_on_name, ConaryConstraint::Legacy(constraint))
                    })
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
                if self.solvables.iter().any(|s| s.repo_package_id == pkg_with_repo.package.id) {
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
                let Some(pkg) = find_repo_package_by_id(self.conn, provide.repository_package_id)? else {
                    continue;
                };
                let Some(repo) = crate::db::models::Repository::find_by_id(self.conn, pkg.repository_id)?
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

        let pattern = format!("%{capability}%");
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT name FROM repository_packages
             WHERE metadata LIKE ?1
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
    pub fn get_dependency_list(&self, id: SolvableId) -> Option<&[(String, ConaryConstraint)]> {
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
        let all_deps: Vec<(u32, Vec<(String, ConaryConstraint)>)> = self
            .dependencies
            .iter()
            .map(|(&sid, deps)| (sid, deps.clone()))
            .collect();

        for (_sid, deps) in &all_deps {
            for (dep_name, constraint) in deps {
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
                        self.intern_repo_version_set(
                            name_id,
                            *scheme,
                            constraint.clone(),
                            raw.clone(),
                        );
                    }
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
}

fn load_repo_dependency_requests(
    conn: &rusqlite::Connection,
    pkg: &RepositoryPackage,
    repo: &crate::db::models::Repository,
) -> Result<Vec<(String, ConaryConstraint)>> {
    let repo_scheme = infer_version_scheme(repo);
    let Some(repository_package_id) = pkg.id else {
        return Ok(pkg
            .parse_dependency_requests()?
            .into_iter()
            .map(|(name, constraint)| (name, ConaryConstraint::Legacy(constraint)))
            .collect());
    };

    let rows = RepositoryRequirement::find_by_repository_package(conn, repository_package_id)?;
    if rows.is_empty() {
        return Ok(pkg
            .parse_dependency_requests()?
            .into_iter()
            .map(|(name, constraint)| (name, ConaryConstraint::Legacy(constraint)))
            .collect());
    }

    Ok(rows
        .into_iter()
        .map(|row| {
            let raw = row.version_constraint.clone();
            let constraint = match (repo_scheme, raw.as_deref()) {
                (Some(scheme), Some(value)) => ConaryConstraint::Repository {
                    scheme,
                    constraint: parse_repo_constraint(scheme, value)
                        .unwrap_or(RepoVersionConstraint::Any),
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
        })
        .collect())
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
                (Some(scheme), Some(raw)) => Some(ConaryProvidedVersion::Repository { raw, scheme }),
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
    Ok(RepositoryPackage::list_all(conn)?
        .into_iter()
        .find(|pkg| pkg.id == Some(repository_package_id)))
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
        _version_set_union: VersionSetUnionId,
    ) -> impl Iterator<Item = VersionSetId> {
        // We don't use unions — return empty iterator
        std::iter::empty()
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

            if let Some(version_cmp) =
                compare_package_versions_desc(&pkg_a.version, &pkg_b.version)
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
            for (dep_name, constraint) in dep_list {
                // Find or create the name id (we can't mutate self in async,
                // so we look up existing names only)
                if let Some(&dep_name_id) = self.name_to_id.get(dep_name) {
                    // O(1) lookup via version_set_cache instead of linear scan
                    let cache_key = (dep_name_id.0, constraint.clone());
                    if let Some(&vs_id) = self.version_set_cache.get(&cache_key) {
                        requirements.push(ConditionalRequirement::from(vs_id));
                    } else {
                        tracing::warn!(
                            "Version set not interned for dependency '{}' -- skipping",
                            dep_name
                        );
                    }
                } else {
                    tracing::warn!(
                        "Dependency '{}' not interned during resolution -- skipping",
                        dep_name
                    );
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
            Self::Repository { raw, .. } => write!(f, "{}", raw),
        }
    }
}

impl fmt::Display for ConaryConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(constraint) => write!(f, "{}", constraint),
            Self::Repository { raw, constraint, .. } => {
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
        (ConaryConstraint::Legacy(constraint), ConaryPackageVersion::Installed(version)) => {
            constraint.satisfies(version)
        }
        (
            ConaryConstraint::Legacy(VersionConstraint::Any),
            ConaryPackageVersion::Repository { .. },
        ) => true,
        (
            ConaryConstraint::Legacy(constraint),
            ConaryPackageVersion::Repository {
                raw,
                scheme: Some(VersionScheme::Rpm),
            },
        ) => RpmVersion::parse(raw)
            .map(|version| constraint.satisfies(&version))
            .unwrap_or(false),
        (
            ConaryConstraint::Repository { scheme, constraint, .. },
            ConaryPackageVersion::Repository {
                raw,
                scheme: Some(version_scheme),
            },
        ) if scheme == version_scheme => repo_version_satisfies(*scheme, raw, constraint),
        (
            ConaryConstraint::Repository {
                constraint: RepoVersionConstraint::Any,
                ..
            },
            ConaryPackageVersion::Repository { .. },
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
                ConaryConstraint::Repository { scheme, constraint, .. },
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
            .find(|deps| deps.iter().any(|(name, _)| name == "kernel-core-uname-r"))
            .cloned()
            .unwrap();

        assert!(deps.iter().any(|(name, constraint)| {
            name == "kernel-core-uname-r"
                && *constraint
                    == ConaryConstraint::Legacy(
                        VersionConstraint::parse("= 6.19.6-200.fc43.x86_64").unwrap(),
                    )
        }));
        assert!(deps.iter().any(|(name, constraint)| {
            name == "coreutils"
                && *constraint
                    == ConaryConstraint::Legacy(VersionConstraint::parse(">= 9.7").unwrap())
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
            .find(|deps| deps.iter().any(|(name, _)| name == "glibc"))
            .cloned()
            .unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].0, "glibc");
        assert_eq!(
            deps[0].1,
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
            version: ConaryPackageVersion::Installed(
                RpmVersion::parse("6.19.6-200.fc43").unwrap(),
            ),
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
    let Some(provides) = metadata.get("rpm_provides").and_then(|value| value.as_array()) else {
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
