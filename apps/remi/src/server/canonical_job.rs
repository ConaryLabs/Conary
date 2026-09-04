// apps/remi/src/server/canonical_job.rs

//! Canonical package-map rebuild from exact contracts and discovery metadata.
//! The publication scheduler runs it after the ordered startup fetch and on its
//! canonical clock; the confirmed MCP mutation can also run it on demand.
//!
//! The rebuild runs a 2-phase pipeline, each phase committing independently
//! for short write locks:
//!
//! 1. **Exact Contract** — versioned literal mappings from `config.rules_dir`
//! 2. **AppStream Metadata** — exact-ID enrichment of contract-owned mappings
//!
//! Repology, package-name similarity, provides, payload paths, and AppStream
//! co-occurrence are discovery metadata only. They never create equivalence or
//! rank a package selected for mutation.

use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use conary_core::canonical::rules::CanonicalMapContract;
use conary_core::db::models::{
    AppstreamCacheEntry, CanonicalMappingAuthority, CanonicalPackage, MetadataTable,
    PackageImplementation, get_metadata, set_metadata,
};
use rusqlite::Connection;
use tracing::{debug, info};

use crate::server::config::CanonicalSection;
use crate::server::database_writer::DatabaseWriter;

/// Record the current UTC time as the last rebuild timestamp.
pub fn record_rebuild_timestamp(conn: &Connection) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    set_metadata(conn, MetadataTable::Server, "last_canonical_rebuild", &now)?;
    Ok(())
}

/// Atomically increment and return the canonical map content revision.
pub fn bump_map_revision(conn: &Connection) -> Result<u64> {
    let current = match get_metadata(conn, MetadataTable::Server, "canonical_map_revision")? {
        Some(value) => value.parse::<u64>().map_err(|error| {
            anyhow::anyhow!("invalid canonical map revision '{value}': {error}")
        })?,
        None => anyhow::bail!("canonical map revision metadata is missing"),
    };

    let next = current + 1;
    set_metadata(
        conn,
        MetadataTable::Server,
        "canonical_map_revision",
        &next.to_string(),
    )?;
    Ok(next)
}

// ---------------------------------------------------------------------------
// Exact-contract rebuild
// ---------------------------------------------------------------------------

/// Rebuild the canonical map from all enabled repositories.
///
/// Opens the database at `db_path` and runs two phases. The exact mappings,
/// rebuild timestamp, and exchanged-map revision commit in one transaction;
/// AppStream enrichment commits separately because it does not alter that map.
///
/// Returns the total count of newly created canonical package entries.
pub(crate) fn rebuild_canonical_map(
    db_path: &Path,
    config: &CanonicalSection,
    database_writer: &DatabaseWriter,
) -> Result<u64> {
    let conn = crate::server::open_runtime_db(db_path)?;
    let rules_dir = Path::new(&config.rules_dir);
    let contract = load_exact_contract(rules_dir)?;
    let (total_new, revision) =
        database_writer.execute(|| persist_exact_contract(&conn, &contract))?;

    let appstream_entries = AppstreamCacheEntry::find_all(&conn)?;
    database_writer.execute(|| persist_appstream_enrichment(&conn, &appstream_entries))?;

    info!(
        "Canonical map rebuild complete: {} new mappings, map revision {}",
        total_new, revision
    );

    Ok(total_new)
}

// ---------------------------------------------------------------------------
// Phase 1 — Exact contract
// ---------------------------------------------------------------------------

/// Load versioned YAML contracts and persist their literal mappings.
fn load_exact_contract(rules_dir: &Path) -> Result<CanonicalMapContract> {
    if rules_dir.is_dir() {
        let contract = CanonicalMapContract::load_from_dir(rules_dir)?;
        info!(
            "Phase 1: loaded {} exact mappings from {}",
            contract.len(),
            rules_dir.display()
        );
        Ok(contract)
    } else {
        debug!(
            "Phase 1: no canonical contract directory at {}; replacing Contract authority with an empty map",
            rules_dir.display()
        );
        Ok(CanonicalMapContract::new(Vec::new())?)
    }
}

fn persist_exact_contract(
    conn: &Connection,
    contract: &CanonicalMapContract,
) -> Result<(u64, u64)> {
    let tx = conn.unchecked_transaction()?;
    let mut new_count: u64 = 0;
    tx.execute(
        "DELETE FROM package_implementations WHERE source = 'contract'",
        [],
    )?;

    for mapping in contract.mappings() {
        let mut canonical =
            CanonicalPackage::new(mapping.canonical.clone(), mapping.kind().to_string());
        canonical.category = mapping.category.clone();
        let already_exists = CanonicalPackage::find_by_name(&tx, &mapping.canonical)?.is_some();
        let canonical_id = canonical.insert_or_verify(&tx)?;

        for profile_id in mapping.profiles.iter() {
            let mut imp = PackageImplementation::new(
                canonical_id,
                profile_id.to_string(),
                mapping.package.clone(),
                CanonicalMappingAuthority::Contract,
            );
            imp.insert_or_verify(&tx)?;
        }

        if !already_exists {
            new_count += 1;
        }
    }

    tx.execute(
        "DELETE FROM canonical_packages
         WHERE NOT EXISTS (
             SELECT 1 FROM package_implementations
             WHERE package_implementations.canonical_id = canonical_packages.id
         )
         AND NOT EXISTS (
             SELECT 1 FROM repository_packages
             WHERE repository_packages.canonical_id = canonical_packages.id
         )
         AND NOT EXISTS (
             SELECT 1 FROM provides
             WHERE provides.canonical_id = canonical_packages.id
         )",
        [],
    )?;
    record_rebuild_timestamp(&tx)?;
    let revision = bump_map_revision(&tx)?;
    tx.commit()?;
    info!(
        "Phase 1: {} new canonical entries from exact contracts at revision {}",
        new_count, revision
    );
    Ok((new_count, revision))
}

#[cfg(test)]
fn phase_exact_contract(conn: &Connection, rules_dir: &Path) -> Result<(u64, u64)> {
    let contract = load_exact_contract(rules_dir)?;
    persist_exact_contract(conn, &contract)
}

// ---------------------------------------------------------------------------
// Phase 2 — AppStream metadata
// ---------------------------------------------------------------------------

/// Attach AppStream IDs only to an already-authorized exact implementation.
///
/// For each cached AppStream component, look for an existing
/// `package_implementations` row where both `distro` and `distro_name` match
/// the AppStream entry.
///
/// AppStream is useful metadata but is not package-equivalence authority. A
/// cache row without a contract-owned implementation is ignored.
fn persist_appstream_enrichment(conn: &Connection, entries: &[AppstreamCacheEntry]) -> Result<u64> {
    if entries.is_empty() {
        debug!("Phase 2: appstream_cache is empty, skipping");
        return Ok(0);
    }

    let tx = conn.unchecked_transaction()?;
    let mut enriched_count: u64 = 0;

    for entry in entries {
        let existing =
            PackageImplementation::find_by_distro_name(&tx, &entry.distro, &entry.pkgname)?;

        if let Some(impl_row) = existing {
            if let Some(owner) = CanonicalPackage::find_by_appstream_id(&tx, &entry.appstream_id)?
                && owner.id != Some(impl_row.canonical_id)
            {
                anyhow::bail!(
                    "AppStream ID '{}' is already attached to canonical package '{}'",
                    entry.appstream_id,
                    owner.name
                );
            }
            let canonical = CanonicalPackage::find_by_id(&tx, impl_row.canonical_id)?
                .ok_or_else(|| anyhow::anyhow!("canonical implementation owner disappeared"))?;
            match canonical.appstream_id.as_deref() {
                Some(current) if current != entry.appstream_id => {
                    anyhow::bail!(
                        "canonical package '{}' already has AppStream ID '{}' and cannot accept '{}'",
                        canonical.name,
                        current,
                        entry.appstream_id
                    );
                }
                Some(_) => {}
                None => {
                    enriched_count += tx.execute(
                        "UPDATE canonical_packages SET appstream_id = ?1 WHERE id = ?2",
                        rusqlite::params![entry.appstream_id, impl_row.canonical_id],
                    )? as u64;
                }
            }
        } else {
            debug!(
                "AppStream component '{}' for {}/{} has no exact canonical contract; retaining as discovery metadata",
                entry.appstream_id, entry.distro, entry.pkgname
            );
        }
    }

    tx.commit()?;
    info!(
        "Phase 2: enriched {} exact canonical entries from AppStream metadata ({} components examined)",
        enriched_count,
        entries.len()
    );
    Ok(enriched_count)
}

#[cfg(test)]
fn phase_appstream_enrichment(conn: &Connection) -> Result<u64> {
    let entries = AppstreamCacheEntry::find_all(conn)?;
    persist_appstream_enrichment(conn, &entries)
}

#[cfg(test)]
mod tests;
