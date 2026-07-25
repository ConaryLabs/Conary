// conary-core/src/resolver/requirements.rs

//! Exact evaluation of typed native requirements against a bounded package set.

use std::collections::HashSet;

use crate::db::models::{ProvideEntry, RepositoryPackage, RepositoryProvide, Trove};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::selector::PackageSelector;
use crate::repository::versioning::{
    RepoVersionConstraint, VersionScheme, parse_repo_constraint, repo_version_satisfies,
};
use crate::resolver::identity::PackageIdentity;
use crate::resolver::identity::ProvidedCapability;

/// Load the exact installed package/provide facts used by bounded requirement
/// evaluation outside the SAT provider.
pub fn load_installed_package_identities(
    conn: &rusqlite::Connection,
) -> Result<Vec<PackageIdentity>> {
    Trove::list_all(conn)?
        .into_iter()
        .map(|trove| {
            let trove_id = trove.id.ok_or_else(|| {
                Error::MissingId(format!(
                    "installed package '{}' was loaded without a persisted trove ID",
                    trove.name
                ))
            })?;
            let version_scheme = trove.version_scheme;
            let mut provided_capabilities = Vec::new();
            for provide in ProvideEntry::find_by_trove(conn, trove_id)? {
                provided_capabilities.push(ProvidedCapability {
                    name: provide.capability.clone(),
                    version: provide.version.clone(),
                    version_scheme,
                });
                let typed = provide.to_typed_string();
                if typed != provide.capability {
                    provided_capabilities.push(ProvidedCapability {
                        name: typed,
                        version: provide.version,
                        version_scheme,
                    });
                }
            }
            Ok(PackageIdentity {
                repo_package_id: None,
                name: trove.name,
                version: trove.version,
                package_release: None,
                architecture: trove.architecture,
                version_scheme,
                repository_id: trove.installed_from_repository_id,
                repository_name: String::new(),
                repository_distro: trove.source_distro,
                repository_priority: 0,
                canonical_id: None,
                canonical_name: None,
                installed_trove_id: Some(trove_id),
                installed_pinned: trove.pinned,
                provided_capabilities,
            })
        })
        .collect()
}

/// Load the bounded installed and repository candidate facts referenced by an
/// exact requirement expression.
///
/// Atomic clause rows are discovery indexes only. The caller must evaluate the
/// original expression against the returned identities to retain Boolean and
/// same-provider semantics.
pub(crate) fn load_requirement_candidate_identities(
    conn: &rusqlite::Connection,
    expression: &RepositoryRequirementExpression,
    architecture: &str,
) -> Result<Vec<PackageIdentity>> {
    let mut identities = load_installed_package_identities(conn)?
        .into_iter()
        .filter(|identity| {
            PackageSelector::is_architecture_compatible(
                identity.architecture.as_deref(),
                architecture,
            )
        })
        .collect::<Vec<_>>();
    let mut seen_repository_packages = HashSet::new();

    for clause in expression.atoms() {
        if matches!(
            clause.capability_kind,
            None | Some(RepositoryCapabilityKind::PackageName)
        ) {
            for identity in PackageIdentity::find_all_by_name(conn, &clause.name)? {
                add_repository_identity(
                    conn,
                    identity,
                    architecture,
                    &mut seen_repository_packages,
                    &mut identities,
                )?;
            }
        }

        let provides = match clause.capability_kind {
            Some(kind) => RepositoryProvide::find_by_capability_and_kind(
                conn,
                &clause.name,
                repository_capability_kind(kind),
            )?,
            None => RepositoryProvide::find_by_capability(conn, &clause.name)?,
        };
        for provide in provides {
            let Some(identity) = repository_identity_by_id(conn, provide.repository_package_id)?
            else {
                continue;
            };
            add_repository_identity(
                conn,
                identity,
                architecture,
                &mut seen_repository_packages,
                &mut identities,
            )?;
        }
    }

    Ok(identities)
}

fn add_repository_identity(
    conn: &rusqlite::Connection,
    mut identity: PackageIdentity,
    architecture: &str,
    seen_repository_packages: &mut HashSet<i64>,
    identities: &mut Vec<PackageIdentity>,
) -> Result<()> {
    let Some(repository_package_id) = identity.repo_package_id else {
        return Ok(());
    };
    if !PackageSelector::is_architecture_compatible(identity.architecture.as_deref(), architecture)
        || !seen_repository_packages.insert(repository_package_id)
    {
        return Ok(());
    }

    identity.provided_capabilities = repository_provided_capabilities(conn, repository_package_id)?;
    identities.push(identity);
    Ok(())
}

fn repository_identity_by_id(
    conn: &rusqlite::Connection,
    repository_package_id: i64,
) -> Result<Option<PackageIdentity>> {
    let Some(package) = RepositoryPackage::find_by_id(conn, repository_package_id)? else {
        return Ok(None);
    };
    Ok(PackageIdentity::find_all_by_name(conn, &package.name)?
        .into_iter()
        .find(|identity| identity.repo_package_id == Some(repository_package_id)))
}

fn repository_provided_capabilities(
    conn: &rusqlite::Connection,
    repository_package_id: i64,
) -> Result<Vec<ProvidedCapability>> {
    let mut capabilities = Vec::new();
    for provide in RepositoryProvide::find_by_repository_package(conn, repository_package_id)? {
        capabilities.push(ProvidedCapability {
            name: provide.capability.clone(),
            version: provide.version.clone(),
            version_scheme: provide.version_scheme,
        });
        let typed = typed_capability_name(&provide.kind, &provide.capability);
        if typed != provide.capability {
            capabilities.push(ProvidedCapability {
                name: typed,
                version: provide.version,
                version_scheme: provide.version_scheme,
            });
        }
    }
    Ok(capabilities)
}

fn repository_capability_kind(kind: RepositoryCapabilityKind) -> &'static str {
    match kind {
        RepositoryCapabilityKind::PackageName => "package",
        RepositoryCapabilityKind::Virtual => "virtual",
        RepositoryCapabilityKind::Soname => "soname",
        RepositoryCapabilityKind::File => "file",
        RepositoryCapabilityKind::Generic => "generic",
    }
}

fn typed_capability_name(kind: &str, name: &str) -> String {
    if kind.is_empty() || kind == "package" {
        name.to_string()
    } else {
        format!("{kind}({name})")
    }
}

/// Evaluate a parsed requirement expression against the supplied package facts.
///
/// `With` and `Without` retain RPM's same-provider semantics by evaluating
/// both operands against each individual package rather than against the
/// package set as a whole.
pub fn requirement_expression_satisfied(
    expression: &RepositoryRequirementExpression,
    version_scheme: VersionScheme,
    packages: &[PackageIdentity],
) -> Result<bool> {
    match expression {
        RepositoryRequirementExpression::Atom(clause) => packages
            .iter()
            .map(|package| atom_satisfied(clause, version_scheme, package))
            .collect::<Result<Vec<_>>>()
            .map(|matches| matches.into_iter().any(|matched| matched)),
        RepositoryRequirementExpression::And(operands) => {
            for operand in operands {
                if !requirement_expression_satisfied(operand, version_scheme, packages)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        RepositoryRequirementExpression::Or(operands) => {
            for operand in operands {
                if requirement_expression_satisfied(operand, version_scheme, packages)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        RepositoryRequirementExpression::If {
            requirement,
            condition,
            otherwise,
        } => {
            if requirement_expression_satisfied(condition, version_scheme, packages)? {
                requirement_expression_satisfied(requirement, version_scheme, packages)
            } else if let Some(otherwise) = otherwise {
                requirement_expression_satisfied(otherwise, version_scheme, packages)
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            if !requirement_expression_satisfied(condition, version_scheme, packages)? {
                requirement_expression_satisfied(requirement, version_scheme, packages)
            } else if let Some(otherwise) = otherwise {
                requirement_expression_satisfied(otherwise, version_scheme, packages)
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::With { left, right } => {
            for package in packages {
                let package = std::slice::from_ref(package);
                if requirement_expression_satisfied(left, version_scheme, package)?
                    && requirement_expression_satisfied(right, version_scheme, package)?
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        RepositoryRequirementExpression::Without { left, right } => {
            for package in packages {
                let package = std::slice::from_ref(package);
                if requirement_expression_satisfied(left, version_scheme, package)?
                    && !requirement_expression_satisfied(right, version_scheme, package)?
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn atom_satisfied(
    clause: &RepositoryRequirementClause,
    version_scheme: VersionScheme,
    package: &PackageIdentity,
) -> Result<bool> {
    let constraint = clause
        .version_constraint
        .as_deref()
        .map(|raw| {
            parse_repo_constraint(version_scheme, raw).map_err(|error| {
                Error::ConfigError(format!(
                    "requirement '{}' has invalid {} constraint '{}': {error}",
                    clause.name,
                    version_scheme.as_str(),
                    raw
                ))
            })
        })
        .transpose()?
        .unwrap_or(RepoVersionConstraint::Any);

    if matches!(
        clause.capability_kind,
        None | Some(RepositoryCapabilityKind::PackageName)
    ) && package.name == clause.name
    {
        if matches!(constraint, RepoVersionConstraint::Any) {
            return Ok(true);
        }
        if package.version_scheme != version_scheme {
            return Ok(false);
        }
        return Ok(repo_version_satisfies(
            version_scheme,
            &package.version,
            &constraint,
        )?);
    }

    let capability_name = clause.capability_kind.map_or_else(
        || clause.name.clone(),
        |kind| typed_capability_name(repository_capability_kind(kind), &clause.name),
    );
    for provide in package
        .provided_capabilities
        .iter()
        .filter(|provide| provide.name == capability_name)
    {
        if matches!(constraint, RepoVersionConstraint::Any) {
            return Ok(true);
        }
        if let Some(version) = provide.version.as_deref()
            && provide.version_scheme == version_scheme
            && repo_version_satisfies(version_scheme, version, &constraint)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::rpm_dependency::parse_rpm_dependency;
    use crate::resolver::identity::ProvidedCapability;

    fn package(name: &str, provides: &[&str]) -> PackageIdentity {
        PackageIdentity {
            repo_package_id: None,
            name: name.to_string(),
            version: "1".to_string(),
            package_release: None,
            architecture: None,
            version_scheme: VersionScheme::Rpm,
            repository_id: None,
            repository_name: String::new(),
            repository_distro: None,
            repository_priority: 0,
            canonical_id: None,
            canonical_name: None,
            installed_trove_id: None,
            installed_pinned: false,
            provided_capabilities: provides
                .iter()
                .map(|name| ProvidedCapability {
                    name: (*name).to_string(),
                    version: None,
                    version_scheme: VersionScheme::Rpm,
                })
                .collect(),
        }
    }

    #[test]
    fn with_requires_one_provider_to_supply_both_atoms() {
        let expression = parse_rpm_dependency("(feature-a with feature-b)").unwrap();
        assert!(
            !requirement_expression_satisfied(
                &expression,
                VersionScheme::Rpm,
                &[
                    package("provider-a", &["feature-a"]),
                    package("provider-b", &["feature-b"]),
                ],
            )
            .unwrap()
        );
        assert!(
            requirement_expression_satisfied(
                &expression,
                VersionScheme::Rpm,
                &[package("provider-both", &["feature-a", "feature-b"])],
            )
            .unwrap()
        );
    }

    #[test]
    fn typed_capability_requirement_does_not_match_same_named_package() {
        let expression = RepositoryRequirementExpression::Atom(RepositoryRequirementClause {
            name: "libexample.so.1".to_string(),
            capability_kind: Some(RepositoryCapabilityKind::Soname),
            version_constraint: None,
            native_text: Some("libexample.so.1".to_string()),
        });

        assert!(
            !requirement_expression_satisfied(
                &expression,
                VersionScheme::Rpm,
                &[package("libexample.so.1", &[])],
            )
            .unwrap()
        );
        assert!(
            requirement_expression_satisfied(
                &expression,
                VersionScheme::Rpm,
                &[package("libexample", &["soname(libexample.so.1)"])],
            )
            .unwrap()
        );
    }
}
