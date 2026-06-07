// src/commands/ccs/install/dependency.rs

use anyhow::Result;
use conary_core::ccs::CcsPackage;
use conary_core::packages::traits::PackageFormat;
use conary_core::repository::versioning::{
    RepoVersionConstraint, VersionScheme, parse_repo_constraint, repo_version_satisfies,
};

fn package_provided_names(ccs_pkg: &CcsPackage) -> std::collections::HashSet<String> {
    let mut provided = std::collections::HashSet::new();
    provided.insert(ccs_pkg.name().to_string());
    provided.extend(ccs_pkg.manifest().provides.capabilities.iter().cloned());
    for soname in &ccs_pkg.manifest().provides.sonames {
        provided.insert(soname.clone());
        provided.insert(format!("soname({soname})"));
    }
    for binary in &ccs_pkg.manifest().provides.binaries {
        provided.insert(binary.clone());
        provided.insert(format!("binary({binary})"));
    }
    for pkgconfig in &ccs_pkg.manifest().provides.pkgconfig {
        provided.insert(pkgconfig.clone());
        provided.insert(format!("pkgconfig({pkgconfig})"));
    }
    provided
}

pub(super) fn package_self_provides(ccs_pkg: &CcsPackage, dep_name: &str) -> bool {
    package_provided_names(ccs_pkg).contains(dep_name)
}

fn installed_versions_satisfying_constraint(
    conn: &rusqlite::Connection,
    package_name: &str,
    version_constraint: Option<&str>,
) -> Result<Vec<String>> {
    let installed = conary_core::db::models::Trove::find_by_name(conn, package_name)?;
    if installed.is_empty() {
        return Ok(Vec::new());
    }

    let Some(version_constraint) = version_constraint.filter(|v| !v.trim().is_empty()) else {
        return Ok(installed.into_iter().map(|trove| trove.version).collect());
    };

    let matches = installed
        .into_iter()
        .filter_map(|trove| {
            version_satisfies_constraint(
                &trove.version,
                trove.version_scheme.as_deref(),
                version_constraint,
            )
            .then_some(trove.version)
        })
        .collect();

    Ok(matches)
}

pub(super) fn validate_package_dependency(
    conn: &rusqlite::Connection,
    package_name: &str,
    version_constraint: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let matching_versions =
        installed_versions_satisfying_constraint(conn, package_name, version_constraint)?;
    if !matching_versions.is_empty() {
        return Ok(());
    }

    let installed_versions = conary_core::db::models::Trove::find_by_name(conn, package_name)?
        .into_iter()
        .map(|trove| trove.version)
        .collect::<Vec<_>>();
    if installed_versions.is_empty()
        && conary_core::db::models::ProvideEntry::is_declared_capability_satisfied(
            conn,
            package_name,
        )?
    {
        return Ok(());
    }

    if dry_run {
        println!("  Missing dependency: {package_name} (would fail)");
        return Ok(());
    }

    if installed_versions.is_empty() {
        anyhow::bail!(
            "Missing dependency: {}{}",
            package_name,
            version_constraint
                .map(|v| format!(" {v}"))
                .unwrap_or_default()
        );
    }

    anyhow::bail!(
        "dependency version mismatch: {} requires {} but installed versions are {}",
        package_name,
        version_constraint.unwrap_or("*"),
        installed_versions.join(", ")
    );
}

pub(super) fn validate_incoming_version_against_dependents(
    conn: &rusqlite::Connection,
    package_name: &str,
    incoming_version: &str,
) -> Result<()> {
    let scheme =
        installed_package_version_scheme(conn, package_name)?.unwrap_or(VersionScheme::Rpm);
    let dependents = conary_core::db::models::DependencyEntry::find_dependents(conn, package_name)?;
    let mut violations = Vec::new();

    for dep in dependents {
        let Some(constraint_str) = dep.version_constraint.as_deref() else {
            continue;
        };
        if repo_constraint_set_satisfied(scheme, incoming_version, constraint_str)? {
            continue;
        }
        let dependent_name = conary_core::db::models::Trove::find_by_id(conn, dep.trove_id)?
            .map(|trove| trove.name)
            .unwrap_or_else(|| format!("trove-{}", dep.trove_id));
        violations.push(format!("{dependent_name} requires {constraint_str}"));
    }

    if violations.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "dependency version mismatch: {} {} would break {}",
        package_name,
        incoming_version,
        violations.join(", ")
    );
}

fn version_satisfies_constraint(
    version: &str,
    version_scheme: Option<&str>,
    constraint: &str,
) -> bool {
    repo_constraint_set_satisfied(
        conary_core::repository::distro::version_scheme_or_rpm(version_scheme),
        version,
        constraint,
    )
    .unwrap_or(false)
}

fn installed_package_version_scheme(
    conn: &rusqlite::Connection,
    package_name: &str,
) -> Result<Option<VersionScheme>> {
    Ok(
        conary_core::db::models::Trove::find_by_name(conn, package_name)?
            .into_iter()
            .find_map(|trove| {
                conary_core::repository::distro::version_scheme_from_db(
                    trove.version_scheme.as_deref(),
                )
            }),
    )
}

fn repo_constraint_set_satisfied(scheme: VersionScheme, version: &str, raw: &str) -> Result<bool> {
    for part in split_constraint_parts(raw) {
        let constraint = parse_repo_constraint(scheme, part)
            .ok_or_else(|| anyhow::anyhow!("invalid version constraint: {raw}"))?;
        if !repo_constraint_satisfies(scheme, version, &constraint) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn split_constraint_parts(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

fn repo_constraint_satisfies(
    scheme: VersionScheme,
    version: &str,
    constraint: &RepoVersionConstraint,
) -> bool {
    repo_version_satisfies(scheme, version, constraint)
}

#[cfg(test)]
mod tests {
    use super::{
        installed_versions_satisfying_constraint, validate_incoming_version_against_dependents,
        validate_package_dependency,
    };

    #[test]
    fn installed_versions_respect_version_constraints() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut trove_v1 = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        trove_v1.insert(&conn).unwrap();

        let mut trove_v2 = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "2.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        trove_v2.insert(&conn).unwrap();

        let matching =
            installed_versions_satisfying_constraint(&conn, "dep-base", Some(">=1.0, <2.0"))
                .unwrap();
        assert_eq!(matching, vec!["1.0.0".to_string()]);

        let not_matching =
            installed_versions_satisfying_constraint(&conn, "dep-base", Some(">=3.0")).unwrap();
        assert!(not_matching.is_empty());
    }

    #[test]
    fn installed_versions_respect_debian_version_constraints() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut prerelease = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0~beta1".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        prerelease.version_scheme = Some("debian".to_string());
        prerelease.insert(&conn).unwrap();

        let mut stable = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        stable.version_scheme = Some("debian".to_string());
        stable.insert(&conn).unwrap();

        let matching =
            installed_versions_satisfying_constraint(&conn, "dep-base", Some(">= 1.0")).unwrap();
        assert_eq!(matching, vec!["1.0".to_string()]);
    }

    #[test]
    fn incoming_version_uses_arch_constraints_for_dependents() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut liba = conary_core::db::models::Trove::new(
            "dep-liba".to_string(),
            "1.0-1".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        liba.version_scheme = Some("arch".to_string());
        liba.insert(&conn).unwrap();

        let mut app = conary_core::db::models::Trove::new(
            "dep-app".to_string(),
            "1.0-1".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        app.version_scheme = Some("arch".to_string());
        let app_id = app.insert(&conn).unwrap();

        let mut dep = conary_core::db::models::DependencyEntry::new(
            app_id,
            "dep-liba".to_string(),
            None,
            "runtime".to_string(),
            Some(">= 1.0-2".to_string()),
        );
        dep.insert(&conn).unwrap();

        let error =
            validate_incoming_version_against_dependents(&conn, "dep-liba", "1.0-1").unwrap_err();
        let error_text = error.to_string();
        assert!(error_text.contains("dependency version mismatch"));
        assert!(error_text.contains("dep-app requires >= 1.0-2"));

        validate_incoming_version_against_dependents(&conn, "dep-liba", "1.0-2").unwrap();
    }

    #[test]
    fn incoming_version_cannot_break_installed_dependents() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut liba = conary_core::db::models::Trove::new(
            "dep-liba".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        liba.insert(&conn).unwrap();

        let mut app = conary_core::db::models::Trove::new(
            "dep-app".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let app_id = app.insert(&conn).unwrap();

        let mut dep = conary_core::db::models::DependencyEntry::new(
            app_id,
            "dep-liba".to_string(),
            None,
            "runtime".to_string(),
            Some(">=1.0, <2.0".to_string()),
        );
        dep.insert(&conn).unwrap();

        let error =
            validate_incoming_version_against_dependents(&conn, "dep-liba", "2.0.0").unwrap_err();
        let error_text = error.to_string();
        assert!(error_text.contains("dependency version mismatch"));
        assert!(error_text.contains("dep-app requires >=1.0, <2.0"));

        validate_incoming_version_against_dependents(&conn, "dep-liba", "1.5.0").unwrap();
    }

    #[test]
    fn package_dependency_rejects_undeclared_capability_guess() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut glibc = conary_core::db::models::Trove::new(
            "glibc".to_string(),
            "2.41.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let trove_id = glibc.insert(&conn).unwrap();

        let mut provide = conary_core::db::models::ProvideEntry::new_typed(
            trove_id,
            "soname",
            "libc.so.6(GLIBC_2.41)(64bit)".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        let err = validate_package_dependency(&conn, "libc.so.6", None, false).unwrap_err();
        assert!(err.to_string().contains("Missing dependency: libc.so.6"));
    }

    #[test]
    fn package_dependency_accepts_declared_capability_when_no_exact_package_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut glibc = conary_core::db::models::Trove::new(
            "glibc".to_string(),
            "2.41.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let trove_id = glibc.insert(&conn).unwrap();

        let mut provide = conary_core::db::models::ProvideEntry::new_typed(
            trove_id,
            "soname",
            "libc.so.6".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        validate_package_dependency(&conn, "libc.so.6", None, false).unwrap();
    }

    #[test]
    fn package_dependency_does_not_hide_exact_package_version_mismatch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("conary.db");
        let db_path_str = db_path.to_str().unwrap();
        conary_core::db::init(db_path_str).unwrap();
        let conn = conary_core::db::open(db_path_str).unwrap();

        let mut package = conary_core::db::models::Trove::new(
            "dep-base".to_string(),
            "1.0.0".to_string(),
            conary_core::db::models::TroveType::Package,
        );
        let trove_id = package.insert(&conn).unwrap();

        let mut provide = conary_core::db::models::ProvideEntry::new_typed(
            trove_id,
            "soname",
            "dep-base.so.1".to_string(),
            None,
        );
        provide.insert(&conn).unwrap();

        let error =
            validate_package_dependency(&conn, "dep-base", Some(">=2.0"), false).unwrap_err();
        assert!(error.to_string().contains("dependency version mismatch"));
    }
}
