// src/commands/ccs/install/dependency.rs

//! Exact installed-dependent validation for CCS replacement installs.

use anyhow::Result;
use conary_core::db::models::InstalledRequirementGroup;
use conary_core::packages::traits::PackageFormat;
use conary_core::repository::dependency_model::RepositoryRequirementKind;
use conary_core::resolver::identity::PackageIdentity;

/// Project the signed/source package facts used by the end-state validator.
pub(super) fn incoming_package_identity(incoming: &dyn PackageFormat) -> Result<PackageIdentity> {
    Ok(PackageIdentity {
        repo_package_id: None,
        name: incoming.name().to_string(),
        version: incoming.version().to_string(),
        package_release: incoming.package_release().map(str::to_string),
        architecture: incoming.architecture().map(str::to_string),
        debian_multi_arch: incoming.debian_multi_arch(),
        version_scheme: incoming.version_scheme(),
        repository_id: None,
        repository_name: String::new(),
        repository_profile: None,
        repository_priority: 0,
        canonical_id: None,
        canonical_name: None,
        installed_trove_id: None,
        installed_pinned: false,
        provided_capabilities: incoming.resolution_capabilities()?,
    })
}

/// Validate installed dependents against the transaction's exact end state.
///
/// The admitted universe is `(installed - outgoing) + incoming`. Outgoing
/// identities are selected by exact trove ID by the caller; package names are
/// not identity, because parallel-installed same-name packages that remain in
/// place must keep their original versions and capabilities.
pub(super) fn validate_incoming_version_against_dependents(
    conn: &rusqlite::Connection,
    outgoing_trove_ids: &[i64],
    incoming: &PackageIdentity,
) -> Result<()> {
    let before = conary_core::resolver::load_installed_package_identities(conn)?;
    let outgoing_trove_ids = outgoing_trove_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let mut replaced = false;
    let mut after = Vec::with_capacity(before.len() + 1);
    for package in &before {
        if package
            .installed_trove_id
            .is_some_and(|trove_id| outgoing_trove_ids.contains(&trove_id))
        {
            replaced = true;
        } else {
            after.push(package.clone());
        }
    }
    if !replaced {
        return Ok(());
    }
    after.push(incoming.clone());

    let native_architecture = conary_core::repository::registry::detect_system_arch();
    let mut violations = Vec::new();
    for dependent in &before {
        let Some(trove_id) = dependent.installed_trove_id else {
            continue;
        };
        let depending_architecture = dependent
            .architecture
            .as_deref()
            .filter(|architecture| !architecture.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "installed dependent '{}' has no architecture authority",
                    dependent.name
                )
            })?;
        for stored in InstalledRequirementGroup::find_by_trove(conn, trove_id)? {
            if !matches!(
                stored.kind,
                RepositoryRequirementKind::Depends | RepositoryRequirementKind::PreDepends
            ) {
                continue;
            }
            let was_satisfied = conary_core::resolver::requirement_expression_satisfied(
                &stored.requirement.expression,
                stored.version_scheme,
                depending_architecture,
                &native_architecture,
                &before,
            )?;
            let remains_satisfied = conary_core::resolver::requirement_expression_satisfied(
                &stored.requirement.expression,
                stored.version_scheme,
                depending_architecture,
                &native_architecture,
                &after,
            )?;
            if was_satisfied && !remains_satisfied {
                let requirement = stored
                    .requirement
                    .native_text
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", stored.requirement.expression));
                violations.push(format!("{} requires {}", dependent.name, requirement));
            }
        }
    }

    if violations.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "dependency version mismatch: {} {} would break {}",
        incoming.name,
        incoming.version,
        violations.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::db::models::{ProvideEntry, Trove, TroveType};
    use conary_core::repository::dependency_model::{
        CapabilityProvenance, ProvideArchitectureQualifier, ProvideVersionRelation,
        RepositoryCapabilityKind, RepositoryRequirementClause, RepositoryRequirementExpression,
        RepositoryRequirementGroup,
    };
    use conary_core::repository::versioning::VersionScheme;
    use conary_core::resolver::identity::ProvidedCapability;

    fn database() -> (tempfile::TempDir, rusqlite::Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("conary.db");
        conary_core::db::init(&path).unwrap();
        (temp, conary_core::db::open(&path).unwrap())
    }

    fn package(
        conn: &rusqlite::Connection,
        name: &str,
        version: &str,
        scheme: VersionScheme,
    ) -> i64 {
        let mut trove = Trove::new(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            scheme,
        );
        trove.architecture = Some(conary_core::repository::registry::detect_system_arch());
        trove.insert(conn).unwrap()
    }

    fn package_with_capability(
        conn: &rusqlite::Connection,
        name: &str,
        version: &str,
        architecture: &str,
        capability: &str,
    ) -> i64 {
        let mut trove = Trove::new(
            name.to_string(),
            version.to_string(),
            TroveType::Package,
            VersionScheme::Rpm,
        );
        trove.architecture = Some(architecture.to_string());
        let trove_id = trove.insert(conn).unwrap();
        let exact_identity = exact_identity_capability(name, version);
        let declared = ProvidedCapability {
            kind: RepositoryCapabilityKind::Generic,
            name: capability.to_string(),
            version: None,
            version_relation: None,
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::AuthorDeclared,
        };
        ProvideEntry::insert_package_capabilities(
            conn,
            trove_id,
            name,
            version,
            VersionScheme::Rpm,
            &[exact_identity, declared],
        )
        .unwrap();
        trove_id
    }

    fn exact_identity_capability(name: &str, version: &str) -> ProvidedCapability {
        ProvidedCapability {
            kind: RepositoryCapabilityKind::PackageName,
            name: name.to_string(),
            version: Some(version.to_string()),
            version_relation: Some(ProvideVersionRelation::Equal),
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }
    }

    fn incoming_identity(
        name: &str,
        version: &str,
        architecture: &str,
        version_scheme: VersionScheme,
        provided_capabilities: Vec<ProvidedCapability>,
    ) -> PackageIdentity {
        PackageIdentity {
            repo_package_id: None,
            name: name.to_string(),
            version: version.to_string(),
            package_release: None,
            architecture: Some(architecture.to_string()),
            debian_multi_arch: None,
            version_scheme,
            repository_id: None,
            repository_name: String::new(),
            repository_profile: None,
            repository_priority: 0,
            canonical_id: None,
            canonical_name: None,
            installed_trove_id: None,
            installed_pinned: false,
            provided_capabilities,
        }
    }

    fn depends_on(capability: &str) -> RepositoryRequirementGroup {
        RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            RepositoryRequirementClause::name_only(capability.to_string()),
        )
    }

    fn alternate_architecture(native: &str) -> &'static str {
        if native == "aarch64" {
            "x86_64"
        } else {
            "aarch64"
        }
    }

    #[test]
    fn incoming_version_is_checked_against_typed_installed_groups() {
        let (_temp, conn) = database();
        package(&conn, "dep-liba", "1.5.0", VersionScheme::Conary);
        let app = package(&conn, "dep-app", "1.0.0", VersionScheme::Conary);
        let requirement = conary_core::repository::requirement::parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Conary,
            "dep-liba < 2.0.0",
        )
        .unwrap();
        InstalledRequirementGroup::insert_groups(&conn, app, VersionScheme::Conary, &[requirement])
            .unwrap();

        let old_id = Trove::find_by_name(&conn, "dep-liba")
            .unwrap()
            .first()
            .and_then(|trove| trove.id)
            .unwrap();
        let native_architecture = conary_core::repository::registry::detect_system_arch();
        let incoming = incoming_identity(
            "dep-liba",
            "2.0.0",
            &native_architecture,
            VersionScheme::Conary,
            Vec::new(),
        );
        let error =
            validate_incoming_version_against_dependents(&conn, &[old_id], &incoming).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("dep-app requires dep-liba < 2.0.0")
        );
        let incoming = incoming_identity(
            "dep-liba",
            "1.9.0",
            &native_architecture,
            VersionScheme::Conary,
            Vec::new(),
        );
        validate_incoming_version_against_dependents(&conn, &[old_id], &incoming).unwrap();
    }

    #[test]
    fn exact_alternative_prevents_false_upgrade_breakage() {
        let (_temp, conn) = database();
        package(&conn, "dep-liba", "1.5", VersionScheme::Rpm);
        package(&conn, "dep-libb", "3", VersionScheme::Rpm);
        let app = package(&conn, "dep-app", "1", VersionScheme::Rpm);
        let left =
            RepositoryRequirementClause::versioned("dep-liba".to_string(), "< 2".to_string());
        let right =
            RepositoryRequirementClause::versioned("dep-libb".to_string(), ">= 3".to_string());
        let mut requirement =
            RepositoryRequirementGroup::simple(RepositoryRequirementKind::Depends, left.clone())
                .with_expression(RepositoryRequirementExpression::Or(vec![
                    RepositoryRequirementExpression::Atom(left.clone()),
                    RepositoryRequirementExpression::Atom(right.clone()),
                ]));
        requirement.alternatives = vec![left, right];
        InstalledRequirementGroup::insert_groups(&conn, app, VersionScheme::Rpm, &[requirement])
            .unwrap();

        let old_id = Trove::find_by_name(&conn, "dep-liba")
            .unwrap()
            .first()
            .and_then(|trove| trove.id)
            .unwrap();
        let native_architecture = conary_core::repository::registry::detect_system_arch();
        let incoming = incoming_identity(
            "dep-liba",
            "2.0",
            &native_architecture,
            VersionScheme::Rpm,
            Vec::new(),
        );
        validate_incoming_version_against_dependents(&conn, &[old_id], &incoming).unwrap();
    }

    #[test]
    fn incoming_ccs_that_drops_an_old_capability_breaks_its_installed_dependent() {
        let (_temp, conn) = database();
        let native_architecture = conary_core::repository::registry::detect_system_arch();
        let old_id = package_with_capability(
            &conn,
            "parallel-provider",
            "1",
            &native_architecture,
            "feature-dropped",
        );
        let dependent_id = package(&conn, "dependent", "1", VersionScheme::Rpm);
        InstalledRequirementGroup::insert_groups(
            &conn,
            dependent_id,
            VersionScheme::Rpm,
            &[depends_on("feature-dropped")],
        )
        .unwrap();

        // The incoming CCS identity carries its exact self-provider but
        // deliberately drops the capability its outgoing version provided.
        let incoming = incoming_identity(
            "parallel-provider",
            "2",
            &native_architecture,
            VersionScheme::Rpm,
            vec![exact_identity_capability("parallel-provider", "2")],
        );
        let error = validate_incoming_version_against_dependents(&conn, &[old_id], &incoming)
            .expect_err("dropping the only provider must fail dependent validation");
        assert!(
            error.to_string().contains("dependent requires")
                && error.to_string().contains("feature-dropped"),
            "{error:#}"
        );
    }

    #[test]
    fn untouched_parallel_same_name_capability_survives_an_exact_replacement() {
        let (_temp, conn) = database();
        let native_architecture = conary_core::repository::registry::detect_system_arch();
        let parallel_architecture = alternate_architecture(&native_architecture);
        let outgoing_id = package_with_capability(
            &conn,
            "parallel-provider",
            "1",
            parallel_architecture,
            "feature-kept-by-other-identity",
        );
        package_with_capability(
            &conn,
            "parallel-provider",
            "9",
            &native_architecture,
            "feature-kept-by-other-identity",
        );
        let dependent_id = package(&conn, "dependent", "1", VersionScheme::Rpm);
        InstalledRequirementGroup::insert_groups(
            &conn,
            dependent_id,
            VersionScheme::Rpm,
            &[depends_on("feature-kept-by-other-identity")],
        )
        .unwrap();

        // Only the non-native identity is outgoing. The incoming CCS drops the
        // capability, so success proves the untouched native identity remains
        // in the end-state universe with its own persisted capabilities.
        let incoming = incoming_identity(
            "parallel-provider",
            "2",
            parallel_architecture,
            VersionScheme::Rpm,
            vec![exact_identity_capability("parallel-provider", "2")],
        );
        validate_incoming_version_against_dependents(&conn, &[outgoing_id], &incoming)
            .expect("the untouched same-name provider must still satisfy the dependent");
    }

    #[test]
    fn installed_schema_has_no_flat_dependency_rows() {
        let (_temp, conn) = database();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'dependencies'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
