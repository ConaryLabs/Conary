// conary-core/src/resolver/requirements.rs

//! Exact evaluation of typed native requirements against a bounded package set.

use std::collections::HashSet;

use crate::db::models::{ProvideEntry, RepositoryPackage, RepositoryProvide, Trove};
use crate::error::{Error, Result};
use crate::repository::dependency_model::{
    RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::versioning::{RepoVersionConstraint, VersionScheme, parse_repo_constraint};
use crate::resolver::identity::PackageIdentity;
use crate::resolver::identity::ProvidedCapability;
use crate::resolver::provider::matching::{
    constraint_architecture_matches_package, constraint_architecture_matches_provide,
    constraint_matches_package, constraint_matches_provide,
};
use crate::resolver::provider::types::ConaryConstraint;

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
                    kind: provide.kind,
                    name: provide.capability,
                    version: provide.version,
                    version_relation: provide.version_relation,
                    version_scheme: provide.version_scheme,
                    architecture_qualifier: provide.architecture_qualifier,
                });
            }
            Ok(PackageIdentity {
                repo_package_id: None,
                name: trove.name,
                version: trove.version,
                package_release: trove.package_release,
                architecture: trove.architecture,
                debian_multi_arch: trove.debian_multi_arch,
                version_scheme,
                repository_id: trove.installed_from_repository_id,
                repository_name: String::new(),
                repository_profile: trove.source_profile,
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
    version_scheme: VersionScheme,
) -> Result<Vec<PackageIdentity>> {
    let mut identities = load_installed_package_identities(conn)?;
    let mut seen_repository_packages = HashSet::new();

    for clause in expression.atoms() {
        if let Some(runtime) = crate::repository::rpm_runtime::RpmRuntimeRequirement::from_clause(
            clause,
            version_scheme,
        )
        .map_err(|error| Error::ConfigError(error.to_string()))?
        {
            runtime
                .ensure_supported()
                .map_err(|error| Error::ConfigError(error.to_string()))?;
            continue;
        }
        if matches!(
            clause.capability_kind,
            None | Some(RepositoryCapabilityKind::PackageName)
        ) {
            for identity in PackageIdentity::find_all_by_name(conn, &clause.name)? {
                add_repository_identity(
                    conn,
                    identity,
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
    seen_repository_packages: &mut HashSet<i64>,
    identities: &mut Vec<PackageIdentity>,
) -> Result<()> {
    let Some(repository_package_id) = identity.repo_package_id else {
        return Ok(());
    };
    if !seen_repository_packages.insert(repository_package_id) {
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
            kind: repository_capability_kind_from_db(&provide.kind)?,
            name: provide.capability,
            version: provide.version,
            version_relation: provide.version_relation,
            version_scheme: provide.version_scheme,
            architecture_qualifier: provide.architecture_qualifier,
        });
    }
    Ok(capabilities)
}

fn repository_capability_kind(kind: RepositoryCapabilityKind) -> &'static str {
    match kind {
        RepositoryCapabilityKind::PackageName => "package",
        RepositoryCapabilityKind::Virtual => "virtual",
        RepositoryCapabilityKind::Soname => "soname",
        RepositoryCapabilityKind::File => "file",
        RepositoryCapabilityKind::Path => "path",
        RepositoryCapabilityKind::Binary => "binary",
        RepositoryCapabilityKind::PkgConfig => "pkgconfig",
        RepositoryCapabilityKind::Generic => "generic",
    }
}

fn repository_capability_kind_from_db(kind: &str) -> Result<RepositoryCapabilityKind> {
    match kind {
        "package" => Ok(RepositoryCapabilityKind::PackageName),
        "virtual" => Ok(RepositoryCapabilityKind::Virtual),
        "soname" => Ok(RepositoryCapabilityKind::Soname),
        "file" => Ok(RepositoryCapabilityKind::File),
        "path" => Ok(RepositoryCapabilityKind::Path),
        "binary" => Ok(RepositoryCapabilityKind::Binary),
        "pkgconfig" => Ok(RepositoryCapabilityKind::PkgConfig),
        "generic" => Ok(RepositoryCapabilityKind::Generic),
        other => Err(Error::ConfigError(format!(
            "unsupported persisted repository capability kind '{other}'"
        ))),
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
    depending_architecture: &str,
    native_architecture: &str,
    packages: &[PackageIdentity],
) -> Result<bool> {
    if depending_architecture.is_empty() || native_architecture.is_empty() {
        return Err(Error::ConfigError(
            "requirement evaluation requires explicit depending and native architectures"
                .to_string(),
        ));
    }
    if let Some(package) = packages.iter().find(|package| {
        package
            .architecture
            .as_deref()
            .is_none_or(|architecture| architecture.is_empty())
    }) {
        return Err(Error::ConfigError(format!(
            "package '{}' has no architecture authority during requirement evaluation",
            package.name
        )));
    }
    requirement_expression_satisfied_validated(
        expression,
        version_scheme,
        depending_architecture,
        native_architecture,
        packages,
    )
}

fn requirement_expression_satisfied_validated(
    expression: &RepositoryRequirementExpression,
    version_scheme: VersionScheme,
    depending_architecture: &str,
    native_architecture: &str,
    packages: &[PackageIdentity],
) -> Result<bool> {
    match expression {
        RepositoryRequirementExpression::Atom(clause) => packages
            .iter()
            .map(|package| {
                atom_satisfied(
                    clause,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    package,
                )
            })
            .collect::<Result<Vec<_>>>()
            .map(|matches| matches.into_iter().any(|matched| matched)),
        RepositoryRequirementExpression::And(operands) => {
            for operand in operands {
                if !requirement_expression_satisfied_validated(
                    operand,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    packages,
                )? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        RepositoryRequirementExpression::Or(operands) => {
            for operand in operands {
                if requirement_expression_satisfied_validated(
                    operand,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    packages,
                )? {
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
            if requirement_expression_satisfied_validated(
                condition,
                version_scheme,
                depending_architecture,
                native_architecture,
                packages,
            )? {
                requirement_expression_satisfied_validated(
                    requirement,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    packages,
                )
            } else if let Some(otherwise) = otherwise {
                requirement_expression_satisfied_validated(
                    otherwise,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    packages,
                )
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::Unless {
            requirement,
            condition,
            otherwise,
        } => {
            if !requirement_expression_satisfied_validated(
                condition,
                version_scheme,
                depending_architecture,
                native_architecture,
                packages,
            )? {
                requirement_expression_satisfied_validated(
                    requirement,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    packages,
                )
            } else if let Some(otherwise) = otherwise {
                requirement_expression_satisfied_validated(
                    otherwise,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    packages,
                )
            } else {
                Ok(true)
            }
        }
        RepositoryRequirementExpression::With { left, right } => {
            for package in packages {
                let package = std::slice::from_ref(package);
                if requirement_expression_satisfied_validated(
                    left,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    package,
                )? && requirement_expression_satisfied_validated(
                    right,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    package,
                )? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        RepositoryRequirementExpression::Without { left, right } => {
            for package in packages {
                let package = std::slice::from_ref(package);
                if requirement_expression_satisfied_validated(
                    left,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    package,
                )? && !requirement_expression_satisfied_validated(
                    right,
                    version_scheme,
                    depending_architecture,
                    native_architecture,
                    package,
                )? {
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
    depending_architecture: &str,
    native_architecture: &str,
    package: &PackageIdentity,
) -> Result<bool> {
    if let Some(runtime) =
        crate::repository::rpm_runtime::RpmRuntimeRequirement::from_clause(clause, version_scheme)
            .map_err(|error| Error::ConfigError(error.to_string()))?
    {
        runtime
            .ensure_supported()
            .map_err(|error| Error::ConfigError(error.to_string()))?;
        return Ok(true);
    }
    let raw = clause.version_constraint.clone();
    let native_constraint = clause
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
    let constraint = ConaryConstraint::Repository {
        scheme: version_scheme,
        constraint: native_constraint,
        capability_kind: clause.capability_kind,
        raw,
        architecture_qualifier: clause.architecture_qualifier.clone(),
        depending_architecture: depending_architecture.to_string(),
    };

    if matches!(
        clause.capability_kind,
        None | Some(RepositoryCapabilityKind::PackageName)
    ) && package.name == clause.name
    {
        if !constraint_architecture_matches_package(&constraint, package, native_architecture) {
            return Ok(false);
        }
        return Ok(constraint_matches_package(
            &constraint,
            &package.version,
            package.version_scheme,
        )?);
    }

    for provide in package.provided_capabilities.iter().filter(|provide| {
        provide.name == clause.name
            && clause
                .capability_kind
                .is_none_or(|kind| provide.kind == kind)
    }) {
        if constraint_architecture_matches_provide(
            &constraint,
            package,
            &provide.architecture_qualifier,
            provide.version_scheme,
            native_architecture,
        ) && constraint_matches_provide(
            &constraint,
            provide.version.as_deref(),
            provide.version_relation,
            provide.version_scheme,
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::dependency_model::{
        DebianMultiArch, ProvideArchitectureQualifier, RepositoryRequirementKind,
        RequirementArchitectureQualifier,
    };
    use crate::repository::rpm_dependency::parse_rpm_dependency;
    use crate::resolver::identity::ProvidedCapability;

    fn package(name: &str, provides: &[&str]) -> PackageIdentity {
        PackageIdentity {
            repo_package_id: None,
            name: name.to_string(),
            version: "1".to_string(),
            package_release: None,
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            version_scheme: VersionScheme::Rpm,
            repository_id: None,
            repository_name: String::new(),
            repository_profile: None,
            repository_priority: 0,
            canonical_id: None,
            canonical_name: None,
            installed_trove_id: None,
            installed_pinned: false,
            provided_capabilities: provides
                .iter()
                .map(|name| ProvidedCapability {
                    kind: RepositoryCapabilityKind::Generic,
                    name: (*name).to_string(),
                    version: None,
                    version_relation: None,
                    version_scheme: VersionScheme::Rpm,
                    architecture_qualifier: Default::default(),
                })
                .collect(),
        }
    }

    fn debian_provider(
        owner_architecture: &str,
        qualifier: ProvideArchitectureQualifier,
    ) -> PackageIdentity {
        let mut package = package("mail-provider", &[]);
        package.architecture = Some(owner_architecture.to_string());
        package.debian_multi_arch = Some(DebianMultiArch::No);
        package.version_scheme = VersionScheme::Debian;
        package.provided_capabilities = vec![ProvidedCapability {
            kind: RepositoryCapabilityKind::Virtual,
            name: "mail-api".to_string(),
            version: None,
            version_relation: None,
            version_scheme: VersionScheme::Debian,
            architecture_qualifier: qualifier,
        }];
        package
    }

    #[test]
    fn with_requires_one_provider_to_supply_both_atoms() {
        let expression = parse_rpm_dependency(
            RepositoryRequirementKind::Depends,
            "(feature-a with feature-b)",
        )
        .unwrap();
        assert!(
            !requirement_expression_satisfied(
                &expression,
                VersionScheme::Rpm,
                "x86_64",
                "x86_64",
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
                "x86_64",
                "x86_64",
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
            architecture_qualifier: Default::default(),
            native_text: Some("libexample.so.1".to_string()),
        });

        assert!(
            !requirement_expression_satisfied(
                &expression,
                VersionScheme::Rpm,
                "x86_64",
                "x86_64",
                &[package("libexample.so.1", &[])],
            )
            .unwrap()
        );
        let mut provider = package("libexample", &[]);
        provider.provided_capabilities = vec![ProvidedCapability {
            kind: RepositoryCapabilityKind::Soname,
            name: "libexample.so.1".to_string(),
            version: None,
            version_relation: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: Default::default(),
        }];
        assert!(
            requirement_expression_satisfied(
                &expression,
                VersionScheme::Rpm,
                "x86_64",
                "x86_64",
                &[provider],
            )
            .unwrap()
        );
    }

    #[test]
    fn bounded_evaluator_uses_exact_debian_provide_architecture() {
        let any = RepositoryRequirementExpression::Atom(RepositoryRequirementClause {
            name: "mail-api".to_string(),
            capability_kind: None,
            version_constraint: None,
            architecture_qualifier: RequirementArchitectureQualifier::Any,
            native_text: Some("mail-api:any".to_string()),
        });
        let unqualified = RepositoryRequirementExpression::Atom(
            RepositoryRequirementClause::name_only("mail-api".to_string()),
        );
        let explicit_any = debian_provider("arm64", ProvideArchitectureQualifier::Any);

        assert!(
            requirement_expression_satisfied(
                &any,
                VersionScheme::Debian,
                "amd64",
                "amd64",
                std::slice::from_ref(&explicit_any),
            )
            .unwrap()
        );
        assert!(
            !requirement_expression_satisfied(
                &unqualified,
                VersionScheme::Debian,
                "amd64",
                "amd64",
                std::slice::from_ref(&explicit_any),
            )
            .unwrap()
        );

        let exact_amd64 = debian_provider(
            "arm64",
            ProvideArchitectureQualifier::Exact("amd64".to_string()),
        );
        assert!(
            requirement_expression_satisfied(
                &unqualified,
                VersionScheme::Debian,
                "amd64",
                "amd64",
                &[exact_amd64],
            )
            .unwrap()
        );
    }

    #[test]
    fn bounded_evaluator_rejects_missing_architecture_authority() {
        let expression = RepositoryRequirementExpression::Atom(
            RepositoryRequirementClause::name_only("feature-a".to_string()),
        );
        let mut provider = package("provider", &["feature-a"]);
        provider.architecture = None;

        let error = requirement_expression_satisfied(
            &expression,
            VersionScheme::Rpm,
            "x86_64",
            "x86_64",
            &[provider],
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("has no architecture authority"),
            "{error}"
        );
    }
}
