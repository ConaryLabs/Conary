// conary-core/src/derived/builder/parent.rs

//! Exact installed-parent selection for derived builds.

use crate::db::models::{Trove, TroveType};
use crate::error::{Error, Result};
use crate::repository::versioning::{compare_mixed_repo_versions, validate_repo_version};
use rusqlite::Connection;
use std::cmp::Ordering;

pub(super) fn find_parent(
    conn: &Connection,
    parent_name: &str,
    parent_version: Option<&str>,
) -> Result<Trove> {
    let candidates = Trove::find_by_name(conn, parent_name)?
        .into_iter()
        .filter(|trove| trove.trove_type == TroveType::Package)
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return Err(Error::InitError(format!(
            "Parent package '{parent_name}' not found"
        )));
    }

    if let Some(version) = parent_version {
        return find_exact_parent(parent_name, version, candidates);
    }

    find_latest_parent(parent_name, candidates)
}

fn find_exact_parent(
    parent_name: &str,
    parent_version: &str,
    candidates: Vec<Trove>,
) -> Result<Trove> {
    let matching = candidates
        .into_iter()
        .filter(|trove| trove.version == parent_version)
        .collect::<Vec<_>>();

    match matching.as_slice() {
        [] => Err(Error::InitError(format!(
            "Parent package '{parent_name}' version '{parent_version}' not found"
        ))),
        [candidate] => {
            validate_repo_version(candidate.version_scheme, &candidate.version)?;
            Ok(candidate.clone())
        }
        _ => Err(ambiguous_parent(parent_name, &matching)),
    }
}

fn find_latest_parent(parent_name: &str, candidates: Vec<Trove>) -> Result<Trove> {
    let selected_architecture = candidates
        .first()
        .ok_or_else(|| {
            Error::InitError(format!(
                "Parent package '{parent_name}' unexpectedly had no package candidates"
            ))
        })?
        .architecture
        .as_deref();
    if candidates
        .iter()
        .any(|candidate| candidate.architecture.as_deref() != selected_architecture)
    {
        return Err(ambiguous_parent(parent_name, &candidates));
    }

    for candidate in &candidates {
        validate_repo_version(candidate.version_scheme, &candidate.version)?;
    }

    let mut candidates = candidates.into_iter();
    let mut best = candidates.next().ok_or_else(|| {
        Error::InitError(format!(
            "Parent package '{parent_name}' unexpectedly had no package candidates"
        ))
    })?;
    let mut equivalent_best = vec![best.clone()];

    for candidate in candidates {
        match compare_mixed_repo_versions(
            candidate.version_scheme,
            &candidate.version,
            best.version_scheme,
            &best.version,
        )? {
            Ordering::Greater => {
                best = candidate;
                equivalent_best.clear();
                equivalent_best.push(best.clone());
            }
            Ordering::Equal => equivalent_best.push(candidate),
            Ordering::Less => {}
        }
    }

    if equivalent_best.len() != 1 {
        return Err(ambiguous_parent(parent_name, &equivalent_best));
    }

    Ok(best)
}

fn ambiguous_parent(parent_name: &str, candidates: &[Trove]) -> Error {
    let mut identities = candidates
        .iter()
        .map(|candidate| {
            format!(
                "{} {} {} ({}, id={})",
                candidate.name,
                candidate.version,
                candidate.architecture.as_deref().unwrap_or("<unspecified>"),
                candidate.version_scheme.as_str(),
                candidate.id.unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    identities.sort();
    Error::ConflictError(format!(
        "derived parent '{parent_name}' is ambiguous among exact installed identities: {}",
        identities.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::versioning::VersionScheme;

    fn test_db() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("conary.db");
        crate::db::init(path.to_str().unwrap()).unwrap();
        let conn = crate::db::open(path.to_str().unwrap()).unwrap();
        (temp, conn)
    }

    fn insert_parent(
        conn: &Connection,
        version: &str,
        scheme: VersionScheme,
        architecture: Option<&str>,
    ) {
        let mut trove = Trove::new(
            "parent".to_string(),
            version.to_string(),
            TroveType::Package,
            scheme,
        );
        trove.architecture = architecture.map(str::to_string);
        trove.debian_multi_arch = (scheme == VersionScheme::Debian)
            .then_some(crate::repository::dependency_model::DebianMultiArch::No);
        trove.insert(conn).unwrap();
    }

    #[test]
    fn latest_parent_uses_owning_version_scheme() {
        let (_temp, conn) = test_db();
        insert_parent(&conn, "1.0-9", VersionScheme::Rpm, Some("x86_64"));
        insert_parent(&conn, "1.0-10", VersionScheme::Rpm, Some("x86_64"));

        let parent = find_parent(&conn, "parent", None).unwrap();

        assert_eq!(parent.version, "1.0-10");
        assert_eq!(parent.version_scheme, VersionScheme::Rpm);
    }

    #[test]
    fn latest_parent_rejects_cross_scheme_ordering() {
        let (_temp, conn) = test_db();
        insert_parent(&conn, "1.0-1", VersionScheme::Rpm, Some("x86_64"));
        insert_parent(&conn, "2.0-1", VersionScheme::Debian, Some("x86_64"));

        let error = find_parent(&conn, "parent", None).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("cannot compare debian and rpm version schemes"),
            "{error}"
        );
    }

    #[test]
    fn exact_parent_rejects_architecture_tie() {
        let (_temp, conn) = test_db();
        insert_parent(&conn, "1.0-1", VersionScheme::Rpm, Some("x86_64"));
        insert_parent(&conn, "1.0-1", VersionScheme::Rpm, Some("aarch64"));

        let error = find_parent(&conn, "parent", Some("1.0-1")).unwrap_err();

        assert!(error.to_string().contains("ambiguous"), "{error}");
        assert!(error.to_string().contains("aarch64"), "{error}");
        assert!(error.to_string().contains("x86_64"), "{error}");
    }

    #[test]
    fn latest_parent_rejects_implicit_architecture_selection() {
        let (_temp, conn) = test_db();
        insert_parent(&conn, "1.0-1", VersionScheme::Rpm, Some("x86_64"));
        insert_parent(&conn, "2.0-1", VersionScheme::Rpm, Some("aarch64"));

        let error = find_parent(&conn, "parent", None).unwrap_err();

        assert!(error.to_string().contains("ambiguous"), "{error}");
        assert!(error.to_string().contains("aarch64"), "{error}");
        assert!(error.to_string().contains("x86_64"), "{error}");
    }
}
