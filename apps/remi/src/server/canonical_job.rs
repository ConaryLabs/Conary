// apps/remi/src/server/canonical_job.rs

//! Scheduled job that builds the canonical package mapping from all
//! indexed distros. Runs after mirror sync or on demand.
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

// ---------------------------------------------------------------------------
// Debounce helpers
// ---------------------------------------------------------------------------

/// Check whether enough time has elapsed since the last rebuild.
///
/// Reads `last_canonical_rebuild` from `server_metadata`. Returns `true`
/// if the timestamp is missing or older than `cooldown_minutes`.
pub fn should_rebuild(conn: &Connection, cooldown_minutes: u64) -> bool {
    let value = match get_metadata(conn, MetadataTable::Server, "last_canonical_rebuild") {
        Ok(Some(v)) => v,
        _ => return true,
    };

    let last = match chrono::DateTime::parse_from_rfc3339(&value) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(_) => return true,
    };

    let elapsed = Utc::now().signed_duration_since(last);
    elapsed.num_minutes() >= cooldown_minutes as i64
}

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
pub fn rebuild_canonical_map(db_path: &Path, config: &CanonicalSection) -> Result<u64> {
    let conn = crate::server::open_runtime_db(db_path)?;
    let rules_dir = Path::new(&config.rules_dir);
    let (total_new, revision) = phase_exact_contract(&conn, rules_dir)?;

    phase_appstream_enrichment(&conn)?;

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
fn phase_exact_contract(conn: &Connection, rules_dir: &Path) -> Result<(u64, u64)> {
    let contract = if rules_dir.is_dir() {
        let contract = CanonicalMapContract::load_from_dir(rules_dir)?;
        info!(
            "Phase 1: loaded {} exact mappings from {}",
            contract.len(),
            rules_dir.display()
        );
        contract
    } else {
        debug!(
            "Phase 1: no canonical contract directory at {}; replacing Contract authority with an empty map",
            rules_dir.display()
        );
        CanonicalMapContract::new(Vec::new())?
    };

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
fn phase_appstream_enrichment(conn: &Connection) -> Result<u64> {
    let entries = AppstreamCacheEntry::find_all(conn)?;

    if entries.is_empty() {
        debug!("Phase 2: appstream_cache is empty, skipping");
        return Ok(0);
    }

    let tx = conn.unchecked_transaction()?;
    let mut enriched_count: u64 = 0;

    for entry in &entries {
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
mod tests {
    use super::*;
    use conary_core::db::schema;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn create_test_db(dir: &TempDir) -> std::path::PathBuf {
        let db_path = dir.path().join("conary.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();

        // Insert a test repository
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, source_profile)
             VALUES ('fedora-44', 'https://example.com/fedora', 1, 'fedora-44')",
            [],
        )
        .unwrap();

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE name = 'fedora-44'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Insert test packages
        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'curl', '8.5.0', 'abc123', 1000, 'https://example.com/curl.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'wget', '1.21', 'def456', 2000, 'https://example.com/wget.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();

        // Insert a second repository with the same package names. Similarity
        // must never create canonical mapping authority.
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, source_profile)
             VALUES ('arch', 'https://example.com/arch', 1, 'arch')",
            [],
        )
        .unwrap();

        let repo2_id: i64 = conn
            .query_row("SELECT id FROM repositories WHERE name = 'arch'", [], |r| {
                r.get(0)
            })
            .unwrap();

        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'curl', '8.5.0', 'abc124', 1000, 'https://example.com/curl.pkg.tar.zst', 'arch')",
            [repo2_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO repository_packages
                (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES
                (?1, 'wget', '1.21', 'def457', 2000, 'https://example.com/wget.pkg.tar.zst', 'arch')",
            [repo2_id],
        )
        .unwrap();

        db_path
    }

    #[test]
    fn test_should_rebuild_respects_cooldown() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();

        // No previous rebuild -- should proceed
        assert!(should_rebuild(&conn, 5));

        // Record a rebuild
        record_rebuild_timestamp(&conn).unwrap();

        // Immediately after -- should skip (within 5 min cooldown)
        assert!(!should_rebuild(&conn, 5));
    }

    #[test]
    fn test_bump_map_revision() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();

        let v1 = bump_map_revision(&conn).unwrap();
        assert_eq!(v1, 1);

        let v2 = bump_map_revision(&conn).unwrap();
        assert_eq!(v2, 2);
    }

    #[test]
    fn bump_map_revision_rejects_corrupt_persisted_state() {
        let conn = Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        set_metadata(
            &conn,
            MetadataTable::Server,
            "canonical_map_revision",
            "not-a-version",
        )
        .unwrap();

        let error = bump_map_revision(&conn).unwrap_err();
        assert!(error.to_string().contains("invalid canonical map revision"));
    }

    #[test]
    fn test_rebuild_canonical_map_empty_db() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("conary.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
        schema::ensure_current(&conn).unwrap();
        drop(conn);

        let config = CanonicalSection {
            rules_dir: dir.path().join("rules").to_string_lossy().to_string(),
            ..Default::default()
        };
        let count = rebuild_canonical_map(&db_path, &config).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn repository_similarity_does_not_create_canonical_authority() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);
        let config = CanonicalSection {
            rules_dir: dir.path().join("rules").to_string_lossy().to_string(),
            ..Default::default()
        };

        let count = rebuild_canonical_map(&db_path, &config).unwrap();
        assert_eq!(count, 0);

        let conn = conary_core::db::open(&db_path).unwrap();
        assert!(
            CanonicalPackage::find_by_name(&conn, "curl")
                .unwrap()
                .is_none(),
            "matching package names across repositories are discovery evidence, not mapping authority"
        );
        assert!(
            CanonicalPackage::find_by_name(&conn, "wget")
                .unwrap()
                .is_none(),
            "matching package names across repositories are discovery evidence, not mapping authority"
        );
    }

    #[test]
    fn test_rebuild_canonical_map_with_exact_contract() {
        let dir = TempDir::new().unwrap();
        let db_path = create_test_db(&dir);

        let rules_dir = dir.path().join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(
            rules_dir.join("01-rename.yaml"),
            "version: 1\nmappings:\n  - canonical: curl-tools\n    package: curl\n    profiles: fedora-44\n",
        )
        .unwrap();

        let config = CanonicalSection {
            rules_dir: rules_dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        let count = rebuild_canonical_map(&db_path, &config).unwrap();
        assert!(count > 0);

        // Verify the exact contract took effect
        let conn = conary_core::db::open(&db_path).unwrap();
        let pkg =
            conary_core::db::models::CanonicalPackage::find_by_name(&conn, "curl-tools").unwrap();
        assert!(
            pkg.is_some(),
            "exact contract should create 'curl-tools' canonical entry"
        );
    }

    #[test]
    fn configured_invalid_contract_is_a_hard_error() {
        let dir = TempDir::new().unwrap();
        let rules_dir = dir.path().join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(rules_dir.join("invalid.yaml"), "mappings: [").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        let error = phase_exact_contract(&conn, &rules_dir).unwrap_err();
        assert!(
            error.to_string().contains("YAML parse error"),
            "configured mappings are an authority contract and parse failures must abort: {error}"
        );
    }

    #[test]
    fn mapping_and_revision_failure_roll_back_together() {
        let dir = TempDir::new().unwrap();
        let rules_dir = dir.path().join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(
            rules_dir.join("contract.yaml"),
            "version: 1\nmappings:\n  - canonical: curl\n    package: curl\n    profiles: fedora-44\n",
        )
        .unwrap();
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();
        set_metadata(
            &conn,
            MetadataTable::Server,
            "canonical_map_revision",
            "invalid",
        )
        .unwrap();

        let error = phase_exact_contract(&conn, &rules_dir).unwrap_err();
        assert!(error.to_string().contains("invalid canonical map revision"));
        assert!(
            CanonicalPackage::find_by_name(&conn, "curl")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            get_metadata(&conn, MetadataTable::Server, "last_canonical_rebuild").unwrap(),
            None
        );
    }

    #[test]
    fn exact_contract_rebuild_removes_retired_contract_authority() {
        let dir = TempDir::new().unwrap();
        let rules_dir = dir.path().join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        let contract_path = rules_dir.join("contract.yaml");
        std::fs::write(
            &contract_path,
            "version: 1\nmappings:\n  - canonical: curl\n    package: curl\n    profiles: fedora-44\n",
        )
        .unwrap();
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        phase_exact_contract(&conn, &rules_dir).unwrap();
        let curl = CanonicalPackage::find_by_name(&conn, "curl")
            .unwrap()
            .unwrap();
        assert!(
            PackageImplementation::find_for_distro(&conn, curl.id.unwrap(), "fedora-44")
                .unwrap()
                .is_some()
        );

        std::fs::write(&contract_path, "version: 1\nmappings: []\n").unwrap();
        phase_exact_contract(&conn, &rules_dir).unwrap();
        assert!(
            CanonicalPackage::find_by_name(&conn, "curl")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn repology_cache_never_creates_canonical_authority() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        conary_core::db::models::RepologyCacheEntry::insert_or_replace(
            &conn,
            &conary_core::db::models::RepologyCacheEntry {
                project_name: "python".into(),
                distro: "fedora-44".into(),
                distro_name: "python3".into(),
                version: Some("3.12.0".into()),
                status: Some("newest".into()),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();
        conary_core::db::models::RepologyCacheEntry::insert_or_replace(
            &conn,
            &conary_core::db::models::RepologyCacheEntry {
                project_name: "python".into(),
                distro: "arch".into(),
                distro_name: "python".into(),
                version: Some("3.12.0".into()),
                status: Some("newest".into()),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        assert!(
            CanonicalPackage::find_by_name(&conn, "python")
                .unwrap()
                .is_none(),
            "Repology co-occurrence is discovery metadata, not equivalence authority"
        );
    }

    #[test]
    fn test_phase_appstream_enriches_existing() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        // Create a canonical package with an implementation
        let mut pkg = CanonicalPackage::new("firefox".into(), "package".into());
        let can_id = pkg.insert(&conn).unwrap();
        let mut imp = PackageImplementation::new(
            can_id,
            "fedora-44".into(),
            "firefox".into(),
            CanonicalMappingAuthority::Contract,
        );
        imp.insert(&conn).unwrap();

        // Insert AppStream entry matching the implementation
        AppstreamCacheEntry::insert_or_replace(
            &conn,
            &AppstreamCacheEntry {
                appstream_id: "org.mozilla.firefox".into(),
                pkgname: "firefox".into(),
                display_name: Some("Firefox".into()),
                summary: Some("Web Browser".into()),
                distro: "fedora-44".into(),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        let count = phase_appstream_enrichment(&conn).unwrap();
        assert_eq!(count, 1);

        let updated = CanonicalPackage::find_by_name(&conn, "firefox")
            .unwrap()
            .unwrap();
        assert_eq!(updated.appstream_id.as_deref(), Some("org.mozilla.firefox"));
    }

    #[test]
    fn appstream_enrichment_uses_exact_distro_identity() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();

        let mut fedora_pkg = CanonicalPackage::new("fedora-editor".into(), "package".into());
        let fedora_id = fedora_pkg.insert(&conn).unwrap();
        let mut fedora_imp = PackageImplementation::new(
            fedora_id,
            "fedora-44".into(),
            "editor".into(),
            CanonicalMappingAuthority::Contract,
        );
        fedora_imp.insert(&conn).unwrap();

        let mut arch_pkg = CanonicalPackage::new("arch-editor".into(), "package".into());
        let arch_id = arch_pkg.insert(&conn).unwrap();
        let mut arch_imp = PackageImplementation::new(
            arch_id,
            "arch".into(),
            "editor".into(),
            CanonicalMappingAuthority::Contract,
        );
        arch_imp.insert(&conn).unwrap();

        AppstreamCacheEntry::insert_or_replace(
            &conn,
            &AppstreamCacheEntry {
                appstream_id: "org.example.Editor".into(),
                pkgname: "editor".into(),
                display_name: Some("Editor".into()),
                summary: None,
                distro: "fedora-44".into(),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        let count = phase_appstream_enrichment(&conn).unwrap();
        assert_eq!(count, 1);

        let fedora = CanonicalPackage::find_by_name(&conn, "fedora-editor")
            .unwrap()
            .unwrap();
        let arch = CanonicalPackage::find_by_name(&conn, "arch-editor")
            .unwrap()
            .unwrap();
        assert_eq!(fedora.appstream_id.as_deref(), Some("org.example.Editor"));
        assert_eq!(
            arch.appstream_id, None,
            "same package name in another distro is not AppStream identity authority"
        );
    }

    #[test]
    fn appstream_without_exact_mapping_remains_discovery_only() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();
        AppstreamCacheEntry::insert_or_replace(
            &conn,
            &AppstreamCacheEntry {
                appstream_id: "org.example.Unmapped".into(),
                pkgname: "unmapped".into(),
                display_name: Some("Unmapped".into()),
                summary: None,
                distro: "fedora-44".into(),
                fetched_at: "2026-03-19T00:00:00Z".into(),
            },
        )
        .unwrap();

        assert_eq!(phase_appstream_enrichment(&conn).unwrap(), 0);
        assert!(
            CanonicalPackage::find_by_name(&conn, "unmapped")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn appstream_cannot_silently_replace_existing_exact_id() {
        let conn = Connection::open_in_memory().unwrap();
        schema::ensure_current(&conn).unwrap();
        let mut package = CanonicalPackage::new("firefox".into(), "package".into());
        package.appstream_id = Some("org.mozilla.Firefox".into());
        let canonical_id = package.insert(&conn).unwrap();
        PackageImplementation::new(
            canonical_id,
            "fedora-44".into(),
            "firefox".into(),
            CanonicalMappingAuthority::Contract,
        )
        .insert(&conn)
        .unwrap();
        AppstreamCacheEntry::insert_or_replace(
            &conn,
            &AppstreamCacheEntry {
                appstream_id: "org.example.NotFirefox".into(),
                pkgname: "firefox".into(),
                display_name: None,
                summary: None,
                distro: "fedora-44".into(),
                fetched_at: "2026-07-26T00:00:00Z".into(),
            },
        )
        .unwrap();

        let error = phase_appstream_enrichment(&conn).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("already has AppStream ID 'org.mozilla.Firefox'")
        );
    }
}
