// crates/conary-core/src/resolver/provider/repository.rs

//! Repository-backed SAT candidate discovery and admission.

use crate::db::models::{Repository, RepositoryProvide};
use crate::error::{Error, Result};
use crate::repository::architecture::{
    NativeResolutionArchitectureDecisionV1, native_resolution_architecture_decision,
    require_profile_host_architecture_token,
};
use crate::repository::selector::{PackageSelector, PackageWithRepo, candidate_source_profile};
use crate::repository::versioning::VersionScheme;
use crate::resolver::identity::PackageIdentity;
use crate::resolver::provides_index::ProviderEntry;
use resolvo::SolvableId;

use super::ConaryProvider;
use super::loading::{
    find_repo_package_by_id, load_repo_dependency_requests, load_repo_provided_capabilities,
    load_repo_relations, relation_to_solver_dep,
};

impl ConaryProvider<'_> {
    /// Admit and intern one repository row as a SAT solvable.
    ///
    /// Every repository discovery path converges here. A row excluded by its
    /// source profile or scheme-owned machine contract never receives a
    /// `SolvableId` and never enters any provider index.
    pub(super) fn load_repository_solvable(
        &mut self,
        pkg_with_repo: PackageWithRepo,
    ) -> Result<Option<SolvableId>> {
        if !self.admit_repository_row(&pkg_with_repo)? {
            return Ok(None);
        }

        let repository_package_id = pkg_with_repo.package.id.ok_or_else(|| {
            Error::MissingId(format!(
                "repository package '{}-{}' selected as a solver candidate",
                pkg_with_repo.package.name, pkg_with_repo.package.version
            ))
        })?;
        if self
            .loaded_repo_package_ids
            .contains(&repository_package_id)
        {
            return Ok(None);
        }

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
        let repository_profile =
            candidate_source_profile(&pkg_with_repo.package, &pkg_with_repo.repository)?
                .map(str::to_string);
        let pkg = PackageIdentity {
            repo_package_id: Some(repository_package_id),
            name: pkg_with_repo.package.name.clone(),
            version: pkg_with_repo.package.version.clone(),
            package_release: (!pkg_with_repo.package.package_release.is_empty())
                .then(|| pkg_with_repo.package.package_release.clone()),
            architecture: pkg_with_repo.package.architecture.clone(),
            debian_multi_arch: pkg_with_repo.package.debian_multi_arch,
            version_scheme: pkg_with_repo.package.version_scheme,
            repository_id: Some(repository_id),
            repository_name: pkg_with_repo.repository.name.clone(),
            repository_profile,
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
        Ok(Some(solvable_id))
    }

    /// Apply the sole repository-row architecture admission decision used by
    /// the SAT provider.
    fn admit_repository_row(&self, candidate: &PackageWithRepo) -> Result<bool> {
        let architecture = candidate.package.architecture.as_deref().ok_or_else(|| {
            Error::ConfigError(format!(
                "repository package '{}-{}' has no architecture authority",
                candidate.package.name, candidate.package.version
            ))
        })?;
        match candidate.package.version_scheme {
            VersionScheme::Rpm | VersionScheme::Debian | VersionScheme::Arch => {
                let profile_id = candidate_source_profile(
                    &candidate.package,
                    &candidate.repository,
                )?
                .ok_or_else(|| {
                    Error::ConfigError(format!(
                        "repository '{}' has no source profile for native package admission",
                        candidate.repository.name
                    ))
                })?;
                let profile =
                    crate::repository::supported_profiles::profile_by_public_id(profile_id)
                        .ok_or_else(|| {
                            Error::ConfigError(format!(
                                "repository '{}' declares unsupported source profile '{}'",
                                candidate.repository.name, profile_id
                            ))
                        })?;
                if profile.version_scheme() != candidate.package.version_scheme {
                    return Err(Error::ConfigError(format!(
                        "repository package '{}-{}' scheme '{}' conflicts with source profile '{}' scheme '{}'",
                        candidate.package.name,
                        candidate.package.version,
                        candidate.package.version_scheme.as_str(),
                        profile.id(),
                        profile.version_scheme().as_str()
                    )));
                }
                require_profile_host_architecture_token(profile, &self.native_architecture)?;
                Ok(matches!(
                    native_resolution_architecture_decision(profile, architecture).into_result()?,
                    NativeResolutionArchitectureDecisionV1::Admitted
                ))
            }
            VersionScheme::Conary | VersionScheme::Eopkg => {
                Ok(PackageSelector::is_machine_architecture_compatible(
                    candidate.package.version_scheme,
                    Some(architecture),
                    &self.native_architecture,
                ))
            }
        }
    }

    pub(super) fn find_repo_providers(&mut self, capability: &str) -> Result<Vec<PackageWithRepo>> {
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
                let repo =
                    Repository::find_by_id(self.conn, pkg.repository_id)?.ok_or_else(|| {
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
                let repo =
                    Repository::find_by_id(self.conn, pkg.repository_id)?.ok_or_else(|| {
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
}
