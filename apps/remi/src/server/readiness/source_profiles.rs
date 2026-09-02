// apps/remi/src/server/readiness/source_profiles.rs

//! Exact configured-profile and active-catalog readiness inspection.

use std::collections::BTreeSet;
use std::path::Path;

use super::ProbeOutcome;
use crate::server::catalog_authority::{CatalogAuthority, ProfileRevisionInspection};

/// Require the current exact member contract and at least one package in every
/// public profile's active catalog.
///
/// Operational SQLite owns which exact profiles are enabled. The verified,
/// pinned immutable catalog alone owns whether an activated profile contains
/// packages; mutable package projections are deliberately irrelevant here.
pub(super) fn probe(
    db_path: &Path,
    catalog_authority: &CatalogAuthority,
    required_profiles: &[String],
) -> ProbeOutcome {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = match rusqlite::Connection::open_with_flags(db_path, flags) {
        Ok(conn) => conn,
        Err(error) => {
            return ProbeOutcome::unavailable(format!(
                "could not inspect source-profile population in {}: {error}",
                db_path.display()
            ));
        }
    };

    let mut statement = match conn.prepare(
        "SELECT DISTINCT source_profile
         FROM repositories
         WHERE enabled = 1 AND source_profile IS NOT NULL
         ORDER BY source_profile",
    ) {
        Ok(statement) => statement,
        Err(error) => {
            return ProbeOutcome::unavailable(format!(
                "could not prepare source-profile population query in {}: {error}",
                db_path.display()
            ));
        }
    };
    let configured = match statement.query_map([], |row| row.get::<_, String>(0)) {
        Ok(rows) => match rows.collect::<Result<Vec<_>, _>>() {
            Ok(configured) => configured.into_iter().collect::<BTreeSet<_>>(),
            Err(error) => {
                return ProbeOutcome::unavailable(format!(
                    "could not read source-profile population in {}: {error}",
                    db_path.display()
                ));
            }
        },
        Err(error) => {
            return ProbeOutcome::unavailable(format!(
                "could not query source-profile population in {}: {error}",
                db_path.display()
            ));
        }
    };

    if configured.is_empty() {
        return ProbeOutcome::not_ready("no enabled exact source profiles are configured");
    }

    let required = if required_profiles.is_empty() {
        configured.clone()
    } else {
        required_profiles.iter().cloned().collect::<BTreeSet<_>>()
    };
    let missing_configuration = required
        .difference(&configured)
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !missing_configuration.is_empty() {
        return ProbeOutcome::not_ready(format!(
            "required source profiles are not enabled: {}",
            missing_configuration.join(", ")
        ));
    }

    let mut missing = Vec::new();
    for profile in required {
        match active_profile_is_populated(catalog_authority, &profile) {
            Ok(true) => {}
            Ok(false) => missing.push(profile),
            Err(error) => {
                return ProbeOutcome::unavailable(format!(
                    "could not verify active immutable catalog for source profile '{profile}': {error}"
                ));
            }
        }
    }

    if missing.is_empty() {
        ProbeOutcome::Ready
    } else {
        ProbeOutcome::not_ready(format!(
            "public source profiles lack their exact current members or populated active immutable catalogs: {}",
            missing.join(", ")
        ))
    }
}

pub(super) fn active_profile_is_populated(
    catalog_authority: &CatalogAuthority,
    profile: &str,
) -> anyhow::Result<bool> {
    let inspection = match catalog_authority.inspect_active_profile_for_upgrade(profile)? {
        ProfileRevisionInspection::Current(inspection) => inspection,
        ProfileRevisionInspection::ObsoleteSchema { .. } => return Ok(false),
    };
    conary_core::repository::supported_profiles::profile_by_public_id(profile)
        .ok_or_else(|| anyhow::anyhow!("profile '{profile}' has no public support contract"))?;
    if inspection.manifest.validate_member_contract().is_err() {
        return Ok(false);
    }
    Ok(inspection.manifest.projection_version
        == crate::server::catalog_refresh::PROFILE_CATALOG_PROJECTION_VERSION
        && inspection.manifest.counts.packages > 0)
}
