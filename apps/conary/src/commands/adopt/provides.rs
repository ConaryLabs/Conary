// apps/conary/src/commands/adopt/provides.rs

//! Typed installed-provider acquisition and persistence for native adoption.

use anyhow::{Context, Result, bail};
use conary_core::db::models::ProvideEntry;
use conary_core::packages::{
    InstalledPackageIdentity, SystemPackageManager, dpkg_query, pacman_query, rpm_query,
};
use conary_core::repository::dependency_model::{
    CapabilityProvenance, ProvideArchitectureQualifier, ProvidedCapability,
    RepositoryCapabilityKind, SourcePackageFormat,
};
use rusqlite::Connection;
use std::collections::BTreeSet;

pub(super) fn query_package_provides(
    manager: SystemPackageManager,
    identity: &InstalledPackageIdentity,
) -> Result<Vec<ProvidedCapability>> {
    let provides = match manager {
        SystemPackageManager::Rpm => rpm_query::query_package_provides(identity),
        SystemPackageManager::Dpkg => dpkg_query::query_package_provides(identity),
        SystemPackageManager::Pacman => pacman_query::query_package_provides(identity),
        SystemPackageManager::Unknown => bail!("unsupported native package manager"),
    }
    .with_context(|| {
        format!(
            "could not inspect exact provides for '{}'",
            identity.selector()
        )
    })?;

    Ok(provides)
}

pub(super) fn insert_package_provides(
    conn: &Connection,
    trove_id: i64,
    identity: &InstalledPackageIdentity,
    provides: &[ProvidedCapability],
) -> conary_core::Result<()> {
    ProvideEntry::insert_package_capabilities(
        conn,
        trove_id,
        identity.name(),
        &identity.version(),
        identity.version_scheme(),
        provides,
    )
}

/// Add exact materialized package paths as source-derived file providers.
///
/// Native solvers admit file dependencies through package file ownership, not
/// only through declared `Provides` header fields. Adoption has already
/// captured those paths from the owning package database and live root, so the
/// same authority must reach Conary's installed provider index.
pub(super) fn extend_materialized_file_provides<'a>(
    provides: &mut Vec<ProvidedCapability>,
    identity: &InstalledPackageIdentity,
    paths: impl IntoIterator<Item = &'a str>,
) -> conary_core::Result<()> {
    let format = match identity {
        InstalledPackageIdentity::Rpm { .. } => SourcePackageFormat::Rpm,
        InstalledPackageIdentity::Dpkg { .. } => SourcePackageFormat::Debian,
        InstalledPackageIdentity::Pacman { .. } => SourcePackageFormat::Alpm,
    };
    let mut existing = provides
        .iter()
        .filter(|provide| provide.kind == RepositoryCapabilityKind::File)
        .map(|provide| provide.name.clone())
        .collect::<BTreeSet<_>>();
    let paths = paths.into_iter().collect::<BTreeSet<_>>();

    for path in paths {
        if path == "/" {
            continue;
        }
        if !path.starts_with('/') {
            return Err(conary_core::Error::ParseError(format!(
                "installed package file provider is not an absolute path: {path:?}"
            )));
        }
        if !existing.insert(path.to_string()) {
            continue;
        }
        let provide = ProvidedCapability {
            kind: RepositoryCapabilityKind::File,
            name: path.to_string(),
            version: None,
            version_relation: None,
            version_scheme: identity.version_scheme(),
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::SourceDerivedFile {
                format,
                source_path: path.to_string(),
            },
        };
        provide.validate()?;
        provides.push(provide);
    }
    Ok(())
}
